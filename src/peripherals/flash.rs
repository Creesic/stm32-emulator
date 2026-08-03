// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{
    config::{InternalFlashConfig, InternalFlashModel, Region},
    system::System,
    util::UniErr,
};

use super::Peripheral;
use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File},
    io::{ErrorKind, Write},
};
use unicorn_engine::Unicorn;

const ACR: u32 = 0x00;
const KEYR: u32 = 0x04;
const OPTKEYR: u32 = 0x08;
const SR: u32 = 0x0c;
const CR: u32 = 0x10;
const OPTCR: u32 = 0x14;

const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xcdef_89ab;
const CR_SER: u32 = 1 << 1;
const CR_SNB_MASK: u32 = 0x1f << 3;
const CR_STRT: u32 = 1 << 16;
const CR_LOCK: u32 = 1 << 31;
const SR_OPERR: u32 = 1 << 1;
const OPTCR_OPTLOCK: u32 = 1;
const ERASED: u8 = 0xff;
const IO_CHUNK: usize = 64 * 1024;

pub struct Flash {
    acr: u32,
    sr: u32,
    cr: u32,
    optcr: u32,
    key_stage: u8,
    internal: Option<InternalFlashConfig>,
}

impl Flash {
    pub fn new(name: &str, internal: Option<InternalFlashConfig>) -> Option<Box<dyn Peripheral>> {
        if name == "FLASH" {
            Some(Box::new(Self {
                acr: 0,
                sr: 0,
                cr: CR_LOCK,
                // RDP 0xAA plus the reset-state write-protection bits. nDBANK
                // (bit 29) intentionally remains clear for dual-bank mode.
                optcr: 0x0fff_aaec | OPTCR_OPTLOCK,
                key_stage: 0,
                internal,
            }))
        } else {
            None
        }
    }

    pub(crate) fn acr_after_write(value: u32) -> u32 {
        value
    }

    pub(crate) fn initialize_memory(
        uc: &mut Unicorn<()>,
        config: &InternalFlashConfig,
        regions: &[Region],
    ) -> Result<()> {
        Self::validate_config(config)?;

        for region in regions {
            if Self::is_flash_mapping(config, region.start, region.size) {
                Self::fill(uc, region.start, region.size, ERASED)?;
            }
        }

        Ok(())
    }

    pub(crate) fn load_persistent(
        uc: &mut Unicorn<()>,
        config: &InternalFlashConfig,
    ) -> Result<()> {
        let Some(persistent) = config.persistent.as_ref() else {
            return Ok(());
        };
        let Some(backing_file) = persistent.backing_file.as_ref() else {
            return Ok(());
        };

        let bytes = match fs::read(backing_file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to read internal-flash backing file {}",
                        backing_file.display()
                    )
                })
            }
        };
        if bytes.len() != persistent.size as usize {
            bail!(
                "Internal-flash backing file {} has {} bytes; expected {}",
                backing_file.display(),
                bytes.len(),
                persistent.size
            );
        }

        let offset = persistent.start - config.start;
        uc.mem_write(persistent.start.into(), &bytes)
            .map_err(UniErr)?;
        for alias in &config.aliases {
            uc.mem_write((*alias + offset).into(), &bytes)
                .map_err(UniErr)?;
        }

        info!(
            "Loaded {} bytes of persistent internal flash from {}",
            bytes.len(),
            backing_file.display()
        );
        Ok(())
    }

    fn validate_config(config: &InternalFlashConfig) -> Result<()> {
        if config.model != InternalFlashModel::Stm32F7DualBank {
            bail!("Unsupported internal flash model");
        }
        if config.size != 0x20_0000 {
            bail!(
                "STM32F7 dual-bank flash requires 0x200000 bytes, got 0x{:x}",
                config.size
            );
        }
        if let Some(persistent) = config.persistent.as_ref() {
            let flash_end = config
                .start
                .checked_add(config.size)
                .context("Internal-flash address range overflows")?;
            let persistent_end = persistent
                .start
                .checked_add(persistent.size)
                .context("Persistent internal-flash address range overflows")?;
            if persistent.start < config.start || persistent_end > flash_end {
                bail!("Persistent internal-flash range lies outside internal flash");
            }
        }
        Ok(())
    }

    fn is_flash_mapping(config: &InternalFlashConfig, start: u32, size: u32) -> bool {
        let end = start.checked_add(size);
        let contains = |base: u32| {
            end.is_some_and(|end| start >= base && end <= base.saturating_add(config.size))
        };
        contains(config.start) || config.aliases.iter().copied().any(contains)
    }

    fn fill(uc: &mut Unicorn<()>, start: u32, size: u32, value: u8) -> Result<()> {
        let chunk = vec![value; IO_CHUNK.min(size as usize)];
        let mut offset = 0usize;
        while offset < size as usize {
            let count = chunk.len().min(size as usize - offset);
            uc.mem_write((start as usize + offset) as u64, &chunk[..count])
                .map_err(UniErr)?;
            offset += count;
        }
        Ok(())
    }

    fn write_key(&mut self, value: u32) {
        match (self.key_stage, value) {
            (0, KEY1) => self.key_stage = 1,
            (1, KEY2) => {
                self.key_stage = 0;
                self.cr &= !CR_LOCK;
            }
            _ => self.key_stage = 0,
        }
    }

    fn write_control(&mut self, sys: &System, value: u32) {
        let was_locked = self.cr & CR_LOCK != 0;
        if was_locked && value & CR_LOCK == 0 {
            return;
        }

        self.cr = value & !CR_STRT;
        if !was_locked && value & CR_STRT != 0 && value & CR_SER != 0 {
            let register_sector = (value & CR_SNB_MASK) >> 3;
            if let Err(error) = self.erase_sector(sys, register_sector as u8) {
                error!("Internal flash sector erase failed: {error:#}");
                self.sr |= SR_OPERR;
            }
        }

        if !was_locked && value & CR_LOCK != 0 {
            if let Err(error) = self.persist(sys) {
                error!("Internal flash persistence failed: {error:#}");
                self.sr |= SR_OPERR;
            }
        }
    }

    fn sector_range(register_sector: u8) -> Option<(u32, u32)> {
        let (bank_offset, bank_sector) = match register_sector {
            0..=11 => (0, register_sector),
            0x10..=0x1b => (0x10_0000, register_sector - 0x10),
            _ => return None,
        };
        let sector_size = match bank_sector {
            0..=3 => 16 * 1024,
            4 => 64 * 1024,
            5..=11 => 128 * 1024,
            _ => return None,
        };
        let sector_offset = match bank_sector {
            0..=3 => u32::from(bank_sector) * 16 * 1024,
            4 => 64 * 1024,
            5..=11 => 128 * 1024 + u32::from(bank_sector - 5) * 128 * 1024,
            _ => return None,
        };
        Some((bank_offset + sector_offset, sector_size))
    }

    fn erase_sector(&mut self, sys: &System, register_sector: u8) -> Result<()> {
        let config = self
            .internal
            .as_ref()
            .context("FLASH erase requested without internal_flash configuration")?;
        let (offset, size) = Self::sector_range(register_sector)
            .with_context(|| format!("Invalid STM32F7 sector register value {register_sector}"))?;
        let mut uc = sys.uc.borrow_mut();
        Self::fill(&mut uc, config.start + offset, size, ERASED)?;
        for alias in &config.aliases {
            Self::fill(&mut uc, *alias + offset, size, ERASED)?;
        }
        info!(
            "Erased STM32F7 flash sector register={} address=0x{:08x} size=0x{:x}",
            register_sector,
            config.start + offset,
            size
        );
        Ok(())
    }

    fn persist(&self, sys: &System) -> Result<()> {
        let Some(config) = self.internal.as_ref() else {
            return Ok(());
        };
        let Some(persistent) = config.persistent.as_ref() else {
            return Ok(());
        };
        let Some(backing_file) = persistent.backing_file.as_ref() else {
            return Ok(());
        };

        if let Some(parent) = backing_file.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create internal-flash state directory {}",
                    parent.display()
                )
            })?;
        }
        let mut bytes = vec![0; persistent.size as usize];
        sys.uc
            .borrow_mut()
            .mem_read(persistent.start.into(), &mut bytes)
            .map_err(UniErr)?;

        let mut file = File::create(backing_file).with_context(|| {
            format!(
                "Failed to create internal-flash backing file {}",
                backing_file.display()
            )
        })?;
        file.write_all(&bytes).with_context(|| {
            format!(
                "Failed to write internal-flash backing file {}",
                backing_file.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "Failed to flush internal-flash backing file {}",
                backing_file.display()
            )
        })?;
        info!(
            "Persisted {} bytes of internal flash to {}",
            bytes.len(),
            backing_file.display()
        );
        Ok(())
    }
}

impl Peripheral for Flash {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            ACR => self.acr,
            KEYR | OPTKEYR => 0,
            SR => self.sr,
            CR => self.cr,
            OPTCR => self.optcr,
            _ => 0,
        }
    }

    fn write(&mut self, sys: &System, offset: u32, value: u32) {
        match offset {
            ACR => self.acr = Self::acr_after_write(value),
            KEYR => self.write_key(value),
            SR => self.sr &= !value,
            CR => self.write_control(sys, value),
            OPTCR if self.optcr & OPTCR_OPTLOCK == 0 => self.optcr = value,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ext_devices::ExtDevices, peripherals::Peripherals, system::System};
    use std::{cell::RefCell, rc::Rc};
    use unicorn_engine::{
        unicorn_const::{Arch, Mode, Prot},
        Unicorn,
    };

    #[test]
    fn f7_dual_bank_sector_register_values_map_to_physical_ranges() {
        assert_eq!(Flash::sector_range(0), Some((0, 16 * 1024)));
        assert_eq!(Flash::sector_range(4), Some((64 * 1024, 64 * 1024)));
        assert_eq!(Flash::sector_range(11), Some((896 * 1024, 128 * 1024)));
        assert_eq!(Flash::sector_range(0x10), Some((1024 * 1024, 16 * 1024)));
        assert_eq!(Flash::sector_range(0x1b), Some((1920 * 1024, 128 * 1024)));
        assert_eq!(Flash::sector_range(12), None);
    }

    #[test]
    fn erase_program_lock_and_restart_preserve_persistent_bank() {
        let state_dir = tempfile::tempdir().unwrap();
        let backing_file = state_dir.path().join("proteus-f7.flash.bin");
        let config = InternalFlashConfig {
            model: InternalFlashModel::Stm32F7DualBank,
            start: 0x0800_0000,
            size: 0x0020_0000,
            aliases: vec![0x0020_0000],
            persistent: Some(crate::config::FlashPersistence {
                start: 0x0810_0000,
                size: 0x0010_0000,
                backing_file: Some(backing_file.clone()),
            }),
        };
        let regions = vec![
            Region {
                name: "ROM-ITCM-alias".to_string(),
                start: 0x0020_0000,
                size: 0x0020_0000,
                load: None,
            },
            Region {
                name: "ROM-AXI-alias".to_string(),
                start: 0x0800_0000,
                size: 0x0020_0000,
                load: None,
            },
        ];

        let mut uc = mapped_flash();
        Flash::initialize_memory(&mut uc, &config, &regions).unwrap();
        uc.mem_write(0x0818_0000, &[0x00; 4]).unwrap();
        uc.mem_write(0x0038_0000, &[0x00; 4]).unwrap();

        let peripherals = Rc::new(Peripherals::default());
        let devices = Rc::new(ExtDevices::default());
        let sys = System {
            uc: RefCell::new(&mut uc),
            p: peripherals,
            d: devices,
        };
        let mut flash = Flash::new("FLASH", Some(config.clone())).unwrap();
        assert_eq!(flash.read(&sys, CR) & CR_LOCK, CR_LOCK);
        flash.write(&sys, KEYR, KEY1);
        flash.write(&sys, KEYR, KEY2);
        assert_eq!(flash.read(&sys, CR) & CR_LOCK, 0);

        // EpicEFI uses logical sector 20 for its first configuration copy.
        // In the F767 dual-bank CR encoding that is bank-2 sector 8 (0x18).
        flash.write(&sys, CR, CR_SER | (0x18 << 3) | CR_STRT);
        let mut erased = [0; 4];
        sys.uc
            .borrow_mut()
            .mem_read(0x0818_0000, &mut erased)
            .unwrap();
        assert_eq!(erased, [0xff; 4]);
        sys.uc
            .borrow_mut()
            .mem_read(0x0038_0000, &mut erased)
            .unwrap();
        assert_eq!(erased, [0xff; 4]);

        let programmed = [0x45, 0x23, 0x01, 0x00];
        sys.uc
            .borrow_mut()
            .mem_write(0x0818_0000, &programmed)
            .unwrap();
        flash.write(&sys, CR, CR_LOCK);
        assert_eq!(fs::metadata(&backing_file).unwrap().len(), 0x0010_0000);
        drop(sys);

        let mut restarted = mapped_flash();
        Flash::initialize_memory(&mut restarted, &config, &regions).unwrap();
        Flash::load_persistent(&mut restarted, &config).unwrap();
        let mut restored = [0; 4];
        restarted.mem_read(0x0818_0000, &mut restored).unwrap();
        assert_eq!(restored, programmed);
        restarted.mem_read(0x0038_0000, &mut restored).unwrap();
        assert_eq!(restored, programmed);
    }

    fn mapped_flash() -> Unicorn<'static, ()> {
        let mut uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        uc.mem_map(0x0020_0000, 0x0020_0000, Prot::ALL).unwrap();
        uc.mem_map(0x0800_0000, 0x0020_0000, Prot::ALL).unwrap();
        uc
    }
}
