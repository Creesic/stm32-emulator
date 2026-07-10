// SPDX-License-Identifier: GPL-3.0-or-later

use crate::system::System;
use super::Peripheral;

pub struct Pwr {
    cr1: u32,
}

impl Pwr {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "PWR" {
            Some(Box::new(Pwr { cr1: 0 }))
        } else {
            None
        }
    }

    pub(crate) fn csr1_after_cr1_write(value: u32) -> u32 {
        (value & 0x0000_c000 != 0) as u32 * 0x0000_4000
            | (value & 0x0001_0000)
            | (value & 0x0002_0000)
    }
}

impl Peripheral for Pwr {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.cr1,
            0x0004 => Self::csr1_after_cr1_write(self.cr1),
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        if offset == 0x0000 {
            self.cr1 = value;
        }
    }
}
