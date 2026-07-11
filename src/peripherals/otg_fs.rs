// SPDX-License-Identifier: GPL-3.0-or-later

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use crate::{ext_devices::{usb_cdc_tcp::UsbCdcTcp, ExtDevices}, system::System};

use super::Peripheral;

pub struct OtgFs {
    bridge: Option<Rc<RefCell<UsbCdcTcp>>>,
    registers: BTreeMap<u32, u32>,
    global_interrupt_status: u32,
    global_interrupt_mask: u32,
    host_attached: bool,
}

impl OtgFs {
    pub const USB_RESET: u32 = 1 << 12;
    const GINTSTS: u32 = 0x0014;
    const GINTMSK: u32 = 0x0018;
    const GRSTCTL: u32 = 0x0010;
    // GRSTCTL's W1-to-request, self-clearing-on-completion command bits:
    // CSRST (core soft reset), RXFFLSH (RX FIFO flush), TXFFLSH (TX FIFO
    // flush). Firmware sets one, then polls for hardware to clear it; we
    // complete every requested operation instantly, so none of these must
    // ever be observed as still set.
    const GRSTCTL_CSRST: u32 = 1 << 0;
    const GRSTCTL_RXFFLSH: u32 = 1 << 4;
    const GRSTCTL_TXFFLSH: u32 = 1 << 5;
    const GRSTCTL_SELF_CLEARING: u32 =
        Self::GRSTCTL_CSRST | Self::GRSTCTL_RXFFLSH | Self::GRSTCTL_TXFFLSH;

    pub fn new(name: &str, ext_devices: &ExtDevices) -> Option<Box<dyn Peripheral>> {
        if name == "OTG_FS_GLOBAL" {
            Some(Box::new(Self {
                bridge: ext_devices.find_usb_cdc_tcp(name),
                registers: BTreeMap::new(),
                global_interrupt_status: 0,
                global_interrupt_mask: 0,
                host_attached: false,
            }))
        } else {
            None
        }
    }

    pub fn for_test() -> Self {
        Self {
            bridge: None,
            registers: BTreeMap::new(),
            global_interrupt_status: 0,
            global_interrupt_mask: 0,
            host_attached: false,
        }
    }

    pub fn set_global_interrupt_status(&mut self, value: u32) {
        self.global_interrupt_status |= value;
    }

    pub fn write_global_interrupt_mask(&mut self, value: u32) {
        self.global_interrupt_mask = value;
    }

    pub fn interrupt_pending(&self) -> bool {
        self.global_interrupt_status & self.global_interrupt_mask != 0
    }

    pub fn virtual_host_reset(&mut self) {
        self.set_global_interrupt_status(Self::USB_RESET);
    }

    fn register_read(&self, offset: u32) -> u32 {
        match offset {
            Self::GRSTCTL => self.registers.get(&offset).copied().unwrap_or(0) | 0x8000_0000,
            Self::GINTSTS => self.global_interrupt_status,
            Self::GINTMSK => self.global_interrupt_mask,
            _ => self.registers.get(&offset).copied().unwrap_or(0),
        }
    }

    fn register_write(&mut self, offset: u32, value: u32) {
        match offset {
            Self::GINTSTS => self.global_interrupt_status &= !value,
            Self::GINTMSK => self.global_interrupt_mask = value,
            Self::GRSTCTL => {
                self.registers
                    .insert(offset, value & !Self::GRSTCTL_SELF_CLEARING);
            }
            _ => {
                self.registers.insert(offset, value);
            }
        }
    }
}

impl Peripheral for OtgFs {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        self.register_read(offset)
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        self.register_write(offset, value);
    }

    fn poll(&mut self, sys: &System) {
        let connected = self
            .bridge
            .as_ref()
            .is_some_and(|bridge| bridge.borrow().is_client_connected());
        if connected && !self.host_attached {
            info!("Virtual USB host attached");
            self.host_attached = true;
            self.virtual_host_reset();
        } else if !connected {
            self.host_attached = false;
        }

        if self.interrupt_pending() {
            sys.p.nvic.borrow_mut().set_intr_pending(67);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OtgFs;

    #[test]
    fn grstctl_core_soft_reset_clears_immediately() {
        let mut otg = OtgFs::for_test();
        otg.register_write(OtgFs::GRSTCTL, OtgFs::GRSTCTL_CSRST);
        assert_eq!(otg.register_read(OtgFs::GRSTCTL) & OtgFs::GRSTCTL_CSRST, 0);
    }

    #[test]
    fn grstctl_fifo_flush_requests_clear_immediately() {
        let mut otg = OtgFs::for_test();
        otg.register_write(
            OtgFs::GRSTCTL,
            OtgFs::GRSTCTL_RXFFLSH | OtgFs::GRSTCTL_TXFFLSH,
        );
        assert_eq!(
            otg.register_read(OtgFs::GRSTCTL) & OtgFs::GRSTCTL_SELF_CLEARING,
            0
        );
    }
}
