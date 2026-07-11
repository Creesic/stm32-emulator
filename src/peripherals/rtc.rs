// SPDX-License-Identifier: GPL-3.0-or-later

use super::Peripheral;
use crate::system::System;

pub struct Rtc {
    isr: u32,
}

impl Rtc {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "RTC" {
            Some(Box::new(Self { isr: 0x0000_0007 }))
        } else {
            None
        }
    }

    pub(crate) fn isr_after_write(value: u32) -> u32 {
        if value & 0x0000_0080 != 0 {
            value | 0x0000_0040
        } else {
            value & !0x0000_0040
        }
    }
}

impl Peripheral for Rtc {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x000c => self.isr,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        if offset == 0x000c {
            self.isr = Self::isr_after_write(value);
        }
    }
}
