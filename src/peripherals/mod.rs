// SPDX-License-Identifier: GPL-3.0-or-later

pub mod adc;
pub mod dma;
pub mod dwt;
pub mod flash;
pub mod fsmc;
pub mod gpio;
pub mod i2c;
pub mod nvic;
pub mod otg_fs;
pub mod pwr;
pub mod rcc;
pub mod rtc;
pub mod scb;
pub mod spi;
pub mod sw_spi;
pub mod systick;
pub mod tim11;
pub mod usart;

use adc::*;
use dma::*;
use dwt::*;
use flash::*;
use fsmc::*;
use gpio::*;
use i2c::*;
use nvic::*;
use otg_fs::*;
use pwr::*;
use rcc::*;
use rtc::*;
use scb::*;
use serde::Deserialize;
use spi::*;
use sw_spi::*;
use systick::*;
use tim11::*;
use usart::*;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
};
use svd_parser::svd::{Device as SvdDevice, RegisterInfo};

use crate::{ext_devices::ExtDevices, system::System};

#[derive(Debug, Deserialize, Default)]
pub struct PeripheralsConfig {
    pub software_spi: Option<Vec<SoftwareSpiConfig>>,
}

#[derive(Default)]
pub struct Peripherals {
    debug_peripherals: Vec<PeripheralSlot<GenericPeripheral>>,
    peripherals: Vec<PeripheralSlot<RefCell<Box<dyn Peripheral>>>>,
    pub nvic: RefCell<Nvic>,
    pub gpio: RefCell<GpioPorts>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn pwr_reports_voltage_scaling_ready_after_configuration() {
        assert_eq!(
            crate::peripherals::pwr::Pwr::csr1_after_cr1_write(0x0000_c000) & 0x0000_4000,
            0x0000_4000
        );
    }

    #[test]
    fn pwr_reports_overdrive_ready_after_overdrive_is_enabled() {
        assert_eq!(
            crate::peripherals::pwr::Pwr::csr1_after_cr1_write(0x0001_c000) & 0x0001_0000,
            0x0001_0000
        );
    }

    #[test]
    fn pwr_csr1_reports_standby_ready_after_observed_enable() {
        assert_eq!(
            crate::peripherals::pwr::Pwr::csr1_after_csr1_write(0x0000_0200) & 0x0000_0008,
            0x0000_0008
        );
    }

    #[test]
    fn rtc_isr_reports_init_ready_after_init_request() {
        assert_eq!(
            crate::peripherals::rtc::Rtc::isr_after_write(0x0000_0080) & 0x0000_0040,
            0x0000_0040
        );
    }

    #[test]
    fn masked_usb_reset_interrupt_does_not_become_pending() {
        let mut otg = crate::peripherals::otg_fs::OtgFs::for_test();
        otg.set_global_interrupt_status(crate::peripherals::otg_fs::OtgFs::USB_RESET);
        assert!(!otg.interrupt_pending());
        otg.write_global_interrupt_mask(crate::peripherals::otg_fs::OtgFs::USB_RESET);
        assert!(otg.interrupt_pending());
    }

    #[test]
    fn virtual_host_reset_sets_the_masked_reset_interrupt() {
        let mut otg = crate::peripherals::otg_fs::OtgFs::for_test();
        otg.write_global_interrupt_mask(crate::peripherals::otg_fs::OtgFs::USB_RESET);
        otg.virtual_host_reset();
        assert!(otg.interrupt_pending());
    }

    #[test]
    fn flash_acr_retains_latency_and_cache_bits() {
        assert_eq!(
            crate::peripherals::flash::Flash::acr_after_write(0x0000_0707),
            0x0000_0707
        );
    }

    #[test]
    fn tim11_reports_capture_ready_after_observed_configuration() {
        assert_eq!(
            crate::peripherals::tim11::Tim11::sr_after_setup(0x0000_0001, 0x0000_0001, 0x0000_0001)
                & 0x0000_0002,
            0x0000_0002
        );
    }

    #[test]
    fn scb_model_range_includes_cpacr() {
        assert_eq!(
            super::Peripherals::modeled_range("SCB", 0xe000_ed00, 4),
            (0xe000_ed00, 0xe000_ed8f),
        );
    }

    #[test]
    fn core_systick_model_covers_control_register() {
        let mut peripherals = super::Peripherals::default();
        peripherals.register_core_peripherals();
        assert!(super::Peripherals::get_peripheral(&peripherals.peripherals, 0xe000_e010).is_some());
    }

    #[test]
    fn core_scb_model_covers_interrupt_control_register() {
        let mut peripherals = super::Peripherals::default();
        peripherals.register_core_peripherals();
        assert!(super::Peripherals::get_peripheral(&peripherals.peripherals, 0xe000_ed04).is_some());
    }
}

pub struct PeripheralSlot<T> {
    pub start: u32,
    pub end: u32,
    pub peripheral: T,
}

impl Peripherals {
    // start - end regions
    pub const MEMORY_MAPS: [(u32, u32); 2] =
        [(0x4000_0000, 0xB000_0000), (0xE000_0000, 0xE100_0000)];

    pub fn register_peripheral(
        &mut self,
        name: String,
        base: u32,
        registers: &[RegisterInfo],
        ext_devices: &ExtDevices,
    ) {
        let p = GenericPeripheral::new(name.clone(), registers);

        let (start, end) = (base, base + p.size());

        trace!(
            "Peripheral start=0x{:08x} end=0x{:08x} name={}",
            start,
            end,
            p.name()
        );

        self.debug_peripherals.push(PeripheralSlot {
            start,
            end,
            peripheral: p,
        });

        // The debug peripheral is just for to print registers right now. So we
        // change the (start, end) only for the real peripheral.
        let (start, end) = Self::modeled_range(&name, start, end - start);

        let p = None
            .or_else(|| NvicWrapper::new(&name))
            .or_else(|| SysTick::new(&name))
            .or_else(|| Scb::new(&name))
            .or_else(|| Gpio::new(&name))
            .or_else(|| Usart::new(&name, ext_devices))
            .or_else(|| Fsmc::new(&name, ext_devices))
            .or_else(|| Rcc::new(&name))
            .or_else(|| Pwr::new(&name))
            .or_else(|| OtgFs::new(&name, ext_devices))
            .or_else(|| Rtc::new(&name))
            .or_else(|| Flash::new(&name))
            .or_else(|| Tim11::new(&name))
            .or_else(|| I2c::new(&name))
            .or_else(|| Dma::new(&name))
            .or_else(|| Spi::new(&name, ext_devices))
            .or_else(|| Adc::new(&name, ext_devices));

        if let Some(p) = p {
            self.peripherals.push(PeripheralSlot {
                start,
                end,
                peripheral: RefCell::new(p),
            });
        }
    }

    fn register_core_peripherals(&mut self) {
        // Pushed in ascending start-address order: get_peripheral binary
        // searches this vec, and this function doesn't sort it itself.
        if let Some(peripheral) = Dwt::new("DWT") {
            self.peripherals.push(PeripheralSlot {
                start: 0xe000_1000,
                end: 0xe000_1008,
                peripheral: RefCell::new(peripheral),
            });
        }
        if let Some(peripheral) = SysTick::new("STK") {
            self.peripherals.push(PeripheralSlot {
                start: 0xe000_e010,
                end: 0xe000_e01c,
                peripheral: RefCell::new(peripheral),
            });
        }
        if let Some(peripheral) = Scb::new("SCB") {
            self.peripherals.push(PeripheralSlot {
                start: 0xe000_ed00,
                end: 0xe000_ed8f,
                peripheral: RefCell::new(peripheral),
            });
        }
    }

    pub fn finish_registration(&mut self) {
        // We sort because we do binary searches to find peripherals
        self.debug_peripherals.sort_by_key(|p| p.start);
        self.peripherals.sort_by_key(|p| p.start);

        {
            // Let's check that peripherals don't overlap
            let a = self.debug_peripherals.iter();
            let mut b = self.debug_peripherals.iter();
            b.next();

            for (p1, p2) in a.zip(b) {
                assert!(
                    p1.end < p2.start,
                    "Overlapping register blocks between {} and {}",
                    p1.peripheral.name(),
                    p2.peripheral.name()
                );
            }
        }
    }

    pub fn from_svd(
        mut svd_device: SvdDevice,
        config: PeripheralsConfig,
        gpio: GpioPorts,
        ext_devices: &ExtDevices,
    ) -> Self {
        let mut peripherals = Self {
            gpio: RefCell::new(gpio),
            ..Peripherals::default()
        };

        svd_device.peripherals.sort_by_key(|f| f.base_address);
        let svd_peripherals = svd_device
            .peripherals
            .iter()
            .map(|d| (d.name.to_string(), d))
            .collect::<HashMap<_, _>>();

        for p in &svd_device.peripherals {
            let name = &p.name;
            let base = p.base_address;

            let p = if let Some(derived_from) = p.derived_from.as_ref() {
                svd_peripherals
                    .get(derived_from)
                    .as_ref()
                    .unwrap_or_else(|| panic!("Cannot find peripheral {}", derived_from))
            } else {
                p
            };

            let regs = crate::util::extract_svd_registers(p);

            peripherals.register_peripheral(name.to_string(), base as u32, &regs, ext_devices);

            if crate::verbose() >= 3 {
                for r in &regs {
                    trace!(
                        "p={} addr=0x{:08x} reg_name={}",
                        p.name,
                        p.base_address as u32 + r.address_offset,
                        r.name
                    );
                }
            }
        }

        peripherals.register_core_peripherals();

        for sw_spi_config in config.software_spi.unwrap_or_default() {
            SoftwareSpi::register(
                sw_spi_config,
                &mut peripherals.gpio.borrow_mut(),
                ext_devices,
            );
        }

        peripherals.finish_registration();
        peripherals
    }

    /////////////////////////////////////////////////////////////////////////////////////////////////////////////

    pub fn get_peripheral<T>(
        peripherals: &Vec<PeripheralSlot<T>>,
        addr: u32,
    ) -> Option<&PeripheralSlot<T>> {
        let index = peripherals
            .binary_search_by_key(&addr, |p| p.start)
            .map_or_else(|e| e.checked_sub(1), |v| Some(v));

        index
            .map(|i| peripherals.get(i).filter(|p| addr <= p.end))
            .flatten()
    }

    pub fn modeled_range(name: &str, base: u32, size: u32) -> (u32, u32) {
        match name {
            "FSMC" => (0x6000_0000, 0xA000_1000),
            "OTG_FS_GLOBAL" => (base, base + 0x7000),
            "SCB" => (base, base + 0x008f),
            _ => (base, base + size),
        }
    }

    pub fn addr_desc(&self, addr: u32) -> String {
        if let Some(p) = Self::get_peripheral(&self.debug_peripherals, addr) {
            format!(
                "addr=0x{:08x} peri={} {}",
                addr,
                p.peripheral.name,
                p.peripheral.reg_name(addr - p.start)
            )
        } else {
            format!("addr=0x{:08x} peri=????", addr)
        }
    }

    fn bitbanding(addr: u32) -> Option<(u32, u8)> {
        if (0x4200_0000..0x4400_0000).contains(&addr) {
            //let old_addr = addr;
            let bit_number = (addr % 32) / 4;
            let addr = 0x4000_0000 + (addr - 0x4200_0000) / 32;
            //trace!("bitbanding: 0x{:08x} -> addr=0x{:08x} bit={}", old_addr, addr, bit_number);
            return Some((addr, bit_number as u8));
        } else {
            None
        }
    }

    fn is_register(addr: u32) -> bool {
        // this is avoiding the FSMC banks, essentially
        !(0x6000_0000..0xA000_0000).contains(&addr)
    }

    fn align_addr_4(addr: u32) -> (u32, u8) {
        let byte_offset = (addr % 4) as u8;
        let addr = addr - byte_offset as u32;
        (addr, byte_offset)
    }

    pub fn read(&self, sys: &System, addr: u32, size: u8) -> u32 {
        if let Some((addr, bit_number)) = Self::bitbanding(addr) {
            return (self.read(sys, addr, 1) >> bit_number) & 1;
        }

        let (addr, byte_offset) = if Self::is_register(addr) {
            // Reduce the access to 4 byte alignements to make things easier when dealing with registers
            Self::align_addr_4(addr)
        } else {
            (addr, 0)
        };

        assert!(byte_offset + size <= 4);

        let value = if let Some(p) = Self::get_peripheral(&self.peripherals, addr) {
            p.peripheral.borrow_mut().read(sys, addr - p.start) << (8 * byte_offset)
        } else {
            0
        };

        if crate::verbose() >= 3 {
            trace!("read:  {} read=0x{:08x}", self.addr_desc(addr), value);
        }

        value
    }

    pub fn write(&self, sys: &System, addr: u32, size: u8, mut value: u32) {
        if let Some((addr, bit_number)) = Self::bitbanding(addr) {
            let mut v = self.read(sys, addr, 1);
            v &= 1 << bit_number;
            v |= (value & 1) << bit_number;
            return self.write(sys, addr, 1, v);
        }

        let (addr, byte_offset) = if Self::is_register(addr) {
            // Reduce the access to 4 byte alignements to make things easier when dealing with registers
            Self::align_addr_4(addr)
        } else {
            (addr, 0)
        };

        assert!(byte_offset + size <= 4);

        if byte_offset != 0 {
            let v = self.read(sys, addr, 4);
            value = (value << 8 * byte_offset) | (v & (0xFFFF_FFFF >> (32 - 8 * byte_offset)));
        }

        if let Some(p) = Self::get_peripheral(&self.peripherals, addr) {
            p.peripheral.borrow_mut().write(sys, addr - p.start, value)
        }

        if crate::verbose() >= 3 {
            trace!("write: {} write=0x{:08x}", self.addr_desc(addr), value);
        }
    }

    pub fn poll(&self, sys: &System) {
        for peripheral in &self.peripherals {
            peripheral.peripheral.borrow_mut().poll(sys);
        }
    }
}

pub trait Peripheral {
    fn read(&mut self, sys: &System, offset: u32) -> u32;
    fn write(&mut self, sys: &System, offset: u32, value: u32);

    fn poll(&mut self, _sys: &System) {}

    fn read_dma(&mut self, sys: &System, offset: u32, size: usize) -> VecDeque<u8> {
        let mut v = VecDeque::with_capacity(size);
        for _ in 0..size {
            v.push_back(self.read(sys, offset) as u8);
        }
        v
    }
    fn write_dma(&mut self, sys: &System, offset: u32, value: VecDeque<u8>) {
        for v in value.into_iter() {
            self.write(sys, offset, v.into());
        }
    }
}

struct GenericPeripheral {
    pub name: String,
    // offset -> name
    pub registers: BTreeMap<u32, RegisterInfo>,
}

impl GenericPeripheral {
    pub fn new(name: String, registers: &[RegisterInfo]) -> Self {
        let registers = registers
            .iter()
            .map(|r| (r.address_offset, r.clone()))
            .collect();

        Self { name, registers }
    }

    pub fn reg_name(&self, offset: u32) -> String {
        assert!(offset % 4 == 0);
        let reg = self.registers.get(&offset);
        reg.map(|r| &r.name)
            .map(|r| format!("offset=0x{:04x} reg={}", offset, r))
            .unwrap_or_else(|| format!("offset=0x{:04x} reg=????", offset))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u32 {
        self.registers.keys().cloned().max().unwrap_or(0) + 4
    }
}
