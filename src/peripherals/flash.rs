// SPDX-License-Identifier: GPL-3.0-or-later

use crate::system::System;

use super::Peripheral;

pub struct Flash {
    acr: u32,
}

impl Flash {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "FLASH" {
            Some(Box::new(Self { acr: 0 }))
        } else {
            None
        }
    }

    pub(crate) fn acr_after_write(value: u32) -> u32 {
        value
    }
}

impl Peripheral for Flash {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.acr,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        if offset == 0x0000 {
            self.acr = Self::acr_after_write(value);
        }
    }
}
