// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::Ordering;

use super::Peripheral;
use crate::system::System;

/// ARM CoreSight DWT unit. Firmware uses DWT->CYCCNT for microsecond-precision
/// polling delays (e.g. ChibiOS's chSysPolledDelayX); without an actually
/// incrementing counter here, such delays never complete.
#[derive(Default)]
pub struct Dwt {
    ctrl: u32,
    // cyccnt reads as (current instruction count + offset); a CYCCNT write
    // sets offset so the next read returns the written value.
    cyccnt_offset: u32,
}

impl Dwt {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "DWT" {
            Some(Box::new(Self::default()))
        } else {
            None
        }
    }

    fn cyccnt(&self) -> u32 {
        (crate::emulator::NUM_INSTRUCTIONS.load(Ordering::Relaxed) as u32).wrapping_add(self.cyccnt_offset)
    }

    fn set_cyccnt(&mut self, value: u32) {
        self.cyccnt_offset = value.wrapping_sub(crate::emulator::NUM_INSTRUCTIONS.load(Ordering::Relaxed) as u32);
    }
}

impl Peripheral for Dwt {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.ctrl,
            0x0004 => self.cyccnt(),
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x0000 => self.ctrl = value,
            0x0004 => self.set_cyccnt(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dwt;

    #[test]
    fn cyccnt_write_is_observed_on_the_next_read() {
        let mut dwt = Dwt::default();
        dwt.set_cyccnt(0x1000);
        assert_eq!(dwt.cyccnt(), 0x1000);
    }
}
