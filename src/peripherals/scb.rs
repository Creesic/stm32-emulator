// SPDX-License-Identifier: GPL-3.0-or-later

use super::{nvic::irq, Peripheral};
use crate::system::System;

#[derive(Default)]
pub struct Scb {
    cpacr: u32,
}

impl Scb {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "SCB" {
            Some(Box::new(Self::default()))
        } else {
            None
        }
    }

    fn write_cpacr(&mut self, value: u32) {
        self.cpacr = value;
    }

    fn read_cpacr(&self) -> u32 {
        self.cpacr
    }
}

impl Peripheral for Scb {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0088 => self.read_cpacr(),
            _ => 0,
        }
    }

    fn write(&mut self, sys: &System, offset: u32, value: u32) {
        match offset {
            0x0004 => {
                // ICSR register
                // bit 26: set systick pending
                // bit 28: set PendSV pending
                if value & (1 << 26) != 0 {
                    sys.p.nvic.borrow_mut().set_intr_pending(irq::SYSTICK);
                }
                if value & (1 << 28) != 0 {
                    sys.p.nvic.borrow_mut().set_intr_pending(irq::PENDSV);
                }
            }
            0x0088 => self.write_cpacr(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scb;

    #[test]
    fn cpacr_retains_the_firmware_fpu_enable_value() {
        let mut scb = Scb::default();
        scb.write_cpacr(0x00f0_0000);

        assert_eq!(scb.read_cpacr(), 0x00f0_0000);
    }
}
