// SPDX-License-Identifier: GPL-3.0-or-later

use crate::system::System;

use super::Peripheral;

pub struct Tim11 {
    cr1: u32,
    ccmr1: u32,
    ccer: u32,
    sr: u32,
}

impl Tim11 {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "TIM11" {
            Some(Box::new(Self {
                cr1: 0,
                ccmr1: 0,
                ccer: 0,
                sr: 0,
            }))
        } else {
            None
        }
    }

    pub(crate) fn sr_after_setup(cr1: u32, ccmr1: u32, ccer: u32) -> u32 {
        if cr1 & 1 != 0 && ccmr1 & 3 == 1 && ccer & 1 != 0 {
            1 << 1
        } else {
            0
        }
    }

    fn refresh_capture_status(&mut self) {
        self.sr |= Self::sr_after_setup(self.cr1, self.ccmr1, self.ccer);
    }
}

impl Peripheral for Tim11 {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.cr1,
            0x0010 => self.sr,
            0x0018 => self.ccmr1,
            0x0020 => self.ccer,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x0000 => self.cr1 = value,
            0x0010 => self.sr &= !value,
            0x0018 => self.ccmr1 = value,
            0x0020 => self.ccer = value,
            _ => return,
        }
        self.refresh_capture_status();
    }
}
