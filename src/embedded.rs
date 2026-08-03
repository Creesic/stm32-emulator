// SPDX-License-Identifier: GPL-3.0-or-later

use crate::config::Config;
use crate::ext_devices::embedded_cdc::{
    clear_embedded_link, install_embedded_link, EmbeddedCdcLink,
};
use crate::ext_devices::embedded_ecu_io::{
    clear_embedded_link as clear_embedded_ecu_io_link,
    install_embedded_link as install_embedded_ecu_io_link, EmbeddedEcuIoLink,
};
use crate::{emulator, set_verbose, util::read_file_str, Args};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static EMBEDDED_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct EmbeddedEmulatorOptions {
    pub config_path: PathBuf,
    /// Optional host-selected backing file for the configured persistent
    /// internal-flash range. This keeps writable simulator state outside a
    /// read-only application or submodule installation.
    pub flash_backing_path: Option<PathBuf>,
    pub verbose: u8,
    pub max_instructions: Option<u64>,
    pub stop_addr: Option<u32>,
    pub busy_loop_stop: bool,
    pub interrupt_period: u32,
    pub dump_stack: Option<usize>,
    pub cdc_link: Option<EmbeddedCdcLink>,
    pub ecu_io_link: Option<EmbeddedEcuIoLink>,
}

impl EmbeddedEmulatorOptions {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
            flash_backing_path: None,
            verbose: 0,
            max_instructions: None,
            stop_addr: None,
            busy_loop_stop: false,
            interrupt_period: 1,
            dump_stack: None,
            cdc_link: None,
            ecu_io_link: None,
        }
    }
}

/// Loads a board configuration and runs its firmware on the calling thread.
///
/// Hosts should dedicate a worker thread to this function. All relative paths
/// in the CPU and memory-region configuration are resolved against the YAML
/// file's directory, so embedding does not depend on the host process's
/// current working directory.
pub fn run_embedded(options: EmbeddedEmulatorOptions) -> Result<()> {
    let _run_guard = EmbeddedRunGuard::acquire()?;
    let config_path = options
        .config_path
        .canonicalize()
        .with_context(|| format!("Config file not found: {}", options.config_path.display()))?;
    let base = config_path
        .parent()
        .expect("a canonical config path always has a parent");

    let config_path_text = config_path
        .to_str()
        .with_context(|| format!("Config path is not valid UTF-8: {}", config_path.display()))?;
    let mut config: Config = serde_yaml::from_str(&read_file_str(config_path_text)?)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;
    resolve_config_paths(&mut config, base);
    if let Some(backing_file) = options.flash_backing_path {
        let persistent = config
            .internal_flash
            .as_mut()
            .and_then(|flash| flash.persistent.as_mut())
            .context("flash_backing_path requires an internal_flash.persistent configuration")?;
        persistent.backing_file = Some(backing_file);
    }

    let device = svd_parser::parse(&read_file_str(&config.cpu.svd)?)
        .with_context(|| format!("Failed to parse {}", config.cpu.svd))?;

    set_verbose(options.verbose);
    let cdc_guard = options.cdc_link.map(|link| {
        install_embedded_link(link.clone());
        EmbeddedLinkGuard(link)
    });
    let ecu_io_guard = options.ecu_io_link.map(|link| {
        install_embedded_ecu_io_link(link.clone());
        EmbeddedEcuIoLinkGuard(link)
    });
    let result = emulator::run_emulator(
        config,
        device,
        Args {
            max_instructions: options.max_instructions,
            stop_addr: options.stop_addr,
            busy_loop_stop: options.busy_loop_stop,
            interrupt_period: options.interrupt_period.max(1),
            dump_stack: options.dump_stack,
        },
    );
    drop(ecu_io_guard);
    drop(cdc_guard);
    result
}

struct EmbeddedRunGuard;

impl EmbeddedRunGuard {
    fn acquire() -> Result<Self> {
        if EMBEDDED_RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            bail!("only one embedded STM32 emulator can run in this process");
        }
        Ok(Self)
    }
}

impl Drop for EmbeddedRunGuard {
    fn drop(&mut self) {
        EMBEDDED_RUNNING.store(false, Ordering::Release);
    }
}

struct EmbeddedEcuIoLinkGuard(EmbeddedEcuIoLink);

impl Drop for EmbeddedEcuIoLinkGuard {
    fn drop(&mut self) {
        clear_embedded_ecu_io_link(&self.0);
    }
}

struct EmbeddedLinkGuard(EmbeddedCdcLink);

impl Drop for EmbeddedLinkGuard {
    fn drop(&mut self) {
        clear_embedded_link(&self.0);
    }
}

fn resolve_config_paths(config: &mut Config, base: &Path) {
    config.cpu.svd = resolve_path(base, &config.cpu.svd).display().to_string();
    for region in &mut config.regions {
        if let Some(load) = region.load.as_mut() {
            *load = resolve_path(base, &*load).display().to_string();
        }
    }
    if let Some(backing_file) = config
        .internal_flash
        .as_mut()
        .and_then(|flash| flash.persistent.as_mut())
        .and_then(|persistent| persistent.backing_file.as_mut())
    {
        *backing_file = resolve_path(base, &*backing_file).to_path_buf();
    }
}

fn resolve_path(base: &Path, value: impl AsRef<Path>) -> PathBuf {
    let path = value.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_defaults_are_unbounded_and_interrupt_accurate() {
        let options = EmbeddedEmulatorOptions::new("board.yaml");
        assert_eq!(options.interrupt_period, 1);
        assert_eq!(options.max_instructions, None);
        assert!(!options.busy_loop_stop);
        assert_eq!(options.flash_backing_path, None);
    }

    #[test]
    fn relative_paths_resolve_against_config_directory() {
        assert_eq!(
            resolve_path(Path::new(r"C:\boards\proteus"), "chip.svd"),
            PathBuf::from(r"C:\boards\proteus\chip.svd")
        );
        assert_eq!(
            resolve_path(Path::new(r"C:\boards\proteus"), r"D:\fw\image.bin"),
            PathBuf::from(r"D:\fw\image.bin")
        );
    }

    #[test]
    fn embedded_run_guard_rejects_overlapping_instances_and_releases_on_drop() {
        let first = EmbeddedRunGuard::acquire().unwrap();
        assert!(EmbeddedRunGuard::acquire().is_err());
        drop(first);
        assert!(EmbeddedRunGuard::acquire().is_ok());
    }
}
