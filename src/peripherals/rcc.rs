// SPDX-License-Identifier: GPL-3.0-or-later

use super::Peripheral;
use crate::system::System;

pub struct Rcc {
    cfgr: u32,
    csr: u32,
    bdcr: u32,
}

impl Rcc {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "RCC" {
            Some(Box::new(Rcc { cfgr: 0, csr: 0, bdcr: 0 }))
        } else {
            None
        }
    }

    fn cfgr_after_write(value: u32) -> u32 {
        (value & !0x0000_000c) | ((value & 0x0000_0003) << 2)
    }

    fn csr_after_write(value: u32) -> u32 {
        if value & 0x0000_0001 != 0 {
            value | 0x0000_0002
        } else {
            value & !0x0000_0002
        }
    }

    fn bdcr_after_write(value: u32) -> u32 {
        if value & 0x0000_0001 != 0 {
            value | 0x0000_0002
        } else {
            value & !0x0000_0002
        }
    }
}

impl Peripheral for Rcc {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => {
                // CR register
                // Return all the r to true. This is where the PLL ready flags are.
                //0b0010_0000_0010_0000_0000_0000_0010
                0xFFFF_FFFF
            }
            0x0008 => {
                // CFGR register
                self.cfgr
            }
            0x0074 => self.csr,
            0x0070 => self.bdcr,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x0008 => self.cfgr = Self::cfgr_after_write(value),
            0x0074 => self.csr = Self::csr_after_write(value),
            0x0070 => self.bdcr = Self::bdcr_after_write(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rcc;

    #[test]
    fn cfgr_reports_hsi_status_when_hsi_is_selected() {
        assert_eq!(Rcc::cfgr_after_write(0) & 0x0000_000c, 0);
    }

    #[test]
    fn csr_reports_lsi_ready_when_lsi_is_enabled() {
        assert_eq!(Rcc::csr_after_write(0x0000_0001) & 0x0000_0002, 0x0000_0002);
    }

    #[test]
    fn bdcr_reports_lse_ready_when_lse_is_enabled() {
        assert_eq!(Rcc::bdcr_after_write(0x0000_0001) & 0x0000_0002, 0x0000_0002);
    }
}
