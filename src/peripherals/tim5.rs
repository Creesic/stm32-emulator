// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::Ordering;

use super::Peripheral;
use crate::system::System;

/// rusEFI's sole hardware timebase (`getTimeNowNt()`/`getTimeNowUs()` read
/// TIM5->CNT directly, see `microsecond_timer_stm32.cpp`). Firmware's own
/// scheduler (`EventQueue::executeOne()`) busy-waits on this counter
/// advancing for near-term scheduled events; without a free-running CNT,
/// that wait never completes and firmware never proceeds past it.
#[derive(Default)]
pub struct Tim5 {
    cr1: u32,
    // CNT reads as (current instruction count + offset) while enabled (CEN
    // set), mirroring Dwt's CYCCNT model; held_cnt is the frozen value while
    // disabled, and the value a CNT write while disabled takes effect from.
    cnt_offset: u32,
    held_cnt: u32,
}

impl Tim5 {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "TIM5" {
            Some(Box::new(Self::default()))
        } else {
            None
        }
    }

    fn cen(&self) -> bool {
        self.cr1 & 1 != 0
    }

    fn num_instructions() -> u32 {
        crate::emulator::NUM_INSTRUCTIONS.load(Ordering::Relaxed) as u32
    }

    fn cnt(&self) -> u32 {
        if self.cen() {
            Self::num_instructions().wrapping_add(self.cnt_offset)
        } else {
            self.held_cnt
        }
    }

    fn write_cr1(&mut self, value: u32) {
        let was_enabled = self.cen();
        let now_enabled = value & 1 != 0;
        if now_enabled && !was_enabled {
            self.cnt_offset = self.held_cnt.wrapping_sub(Self::num_instructions());
        } else if !now_enabled && was_enabled {
            self.held_cnt = self.cnt();
        }
        self.cr1 = value;
    }

    fn write_cnt(&mut self, value: u32) {
        if self.cen() {
            self.cnt_offset = value.wrapping_sub(Self::num_instructions());
        } else {
            self.held_cnt = value;
        }
    }
}

impl Peripheral for Tim5 {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.cr1,
            0x0024 => self.cnt(),
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x0000 => self.write_cr1(value),
            0x0024 => self.write_cnt(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Tim5;

    // NUM_INSTRUCTIONS is a single process-wide static that other tests may
    // read concurrently (cargo test runs tests in parallel threads), so
    // these tests never mutate it directly — like Dwt's own test, they only
    // rely on a write being observed on the very next read, which holds
    // regardless of NUM_INSTRUCTIONS's actual value since no real
    // instructions execute between the two calls.

    #[test]
    fn cnt_reads_zero_by_default_while_disabled() {
        let tim5 = Tim5::default();
        assert_eq!(tim5.cnt(), 0);
    }

    #[test]
    fn writing_cnt_while_enabled_is_observed_on_the_next_read() {
        let mut tim5 = Tim5::default();
        tim5.write_cr1(1); // CEN
        tim5.write_cnt(0x1000);
        assert_eq!(tim5.cnt(), 0x1000);
    }

    #[test]
    fn disabling_freezes_cnt_at_its_current_value() {
        let mut tim5 = Tim5::default();
        tim5.write_cr1(1);
        tim5.write_cnt(0x2000);
        tim5.write_cr1(0); // disable
        assert_eq!(tim5.cnt(), 0x2000);
    }

    #[test]
    fn cnt_written_while_disabled_takes_effect_once_enabled() {
        let mut tim5 = Tim5::default();
        tim5.write_cnt(0x3000); // while disabled
        tim5.write_cr1(1); // enable
        assert_eq!(tim5.cnt(), 0x3000);
    }
}
