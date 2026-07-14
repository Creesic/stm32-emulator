// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::Ordering;

use super::Peripheral;
use crate::system::System;

/// TIM5's global interrupt, fixed at this NVIC position across the whole
/// STM32F4/F7 family. STM32F767.svd has no `<interrupt>` tag on TIM5's own
/// peripheral block -- ST's SVD misattaches a "TIM5" interrupt entry to the
/// TIM10 block instead -- but its value (50) is the correct one per the
/// reference manual's vector table, so it's hardcoded here rather than read
/// from the SVD.
const TIM5_IRQ: i32 = 50;

/// rusEFI's sole hardware timebase (`getTimeNowNt()`/`getTimeNowUs()` read
/// TIM5->CNT directly, see `microsecond_timer_stm32.cpp`). Firmware's own
/// scheduler (`EventQueue::executeOne()`) busy-waits on this counter
/// advancing for near-term scheduled events; without a free-running CNT,
/// that wait never completes and firmware never proceeds past it.
///
/// Also backs rusEFI's scheduled-interrupt path: `setHardwareSchedulerTimer()`
/// arms channel 1 as a one-shot output-compare alarm (CCR1 + DIER.CC1IE) and
/// waits for its ISR to run `portMicrosecondTimerCallback()`. Without a real
/// compare-match/interrupt here, rusEFI's boot-time self-test
/// (`validateHardwareTimer()`) never observes its test callback fire and
/// raises "hwTimer not alive".
#[derive(Default)]
pub struct Tim5 {
    cr1: u32,
    dier: u32,
    sr: u32,
    ccr1: u32,
    // Sticky like real SR.CCxIF: nothing arms compare-match detection until
    // firmware has actually written CCR1 once. Without this guard, CCR1's
    // reset value of 0 would spuriously "match" the moment CNT starts
    // counting from 0, latching SR before rusEFI ever uses the channel.
    ccr1_armed: bool,
    // CNT reads as (current instruction count + offset) while enabled (CEN
    // set), mirroring Dwt's CYCCNT model; held_cnt is the frozen value while
    // disabled, and the value a CNT write while disabled takes effect from.
    cnt_offset: u32,
    held_cnt: u32,
}

impl Tim5 {
    const DIER_CC1IE: u32 = 1 << 1;
    const SR_CC1IF: u32 = 1 << 1;

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

    // Wrapping-safe "has CNT reached or passed CCR1 yet", the same signed-
    // difference trick rusEFI itself uses (see `timeDeltaNt <= 0` in
    // microsecond_timer.cpp) to compare a free-running counter against a
    // target that may be behind it after a wraparound.
    fn has_passed_ccr1(&self) -> bool {
        (self.cnt().wrapping_sub(self.ccr1) as i32) >= 0
    }
}

impl Peripheral for Tim5 {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.cr1,
            0x000c => self.dier,
            0x0010 => self.sr,
            0x0024 => self.cnt(),
            0x0034 => self.ccr1,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x0000 => self.write_cr1(value),
            0x000c => self.dier = value,
            0x0010 => self.sr &= value, // rc_w0: writing 0 clears a flag bit
            0x0024 => self.write_cnt(value),
            0x0034 => {
                self.ccr1 = value;
                self.ccr1_armed = true;
            }
            _ => {}
        }
    }

    // DIEPINT.TXFE taught us the same lesson for OTG_FS's TX-empty flag:
    // this is a level condition (SR.CC1IF held high while DIER.CC1IE is
    // enabled), not a one-shot edge fired only at the moment of the
    // original compare match -- so re-assert to the NVIC every poll while
    // both are true, not just once.
    fn poll(&mut self, sys: &System) {
        if self.ccr1_armed && self.sr & Tim5::SR_CC1IF == 0 && self.has_passed_ccr1() {
            self.sr |= Tim5::SR_CC1IF;
        }
        if self.sr & Tim5::SR_CC1IF != 0 && self.dier & Self::DIER_CC1IE != 0 {
            sys.p.nvic.borrow_mut().set_intr_pending(TIM5_IRQ);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use unicorn_engine::{
        unicorn_const::{Arch, Mode},
        ArmCpuModel, Unicorn,
    };

    use super::{Peripheral, Tim5};
    use crate::{ext_devices::ExtDevices, peripherals::Peripherals, system::System};

    fn test_parts() -> (Unicorn<'static, ()>, Rc<Peripherals>, Rc<ExtDevices>) {
        let mut uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        uc.ctl_set_cpu_model(ArmCpuModel::CORTEX_M4 as i32).unwrap();
        (uc, Rc::new(Peripherals::default()), Rc::new(ExtDevices::default()))
    }

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

    #[test]
    fn ccr1_reset_value_of_zero_never_spuriously_matches_before_firmware_arms_it() {
        // CCR1's reset value is 0, and CNT also starts at 0 -- without the
        // ccr1_armed guard, enabling the timer would immediately look like
        // a compare match on channel 1, latching SR before rusEFI has ever
        // used the channel for real (see setHardwareSchedulerTimer()).
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };
        let mut tim5 = Tim5::default();

        tim5.write_cr1(1); // CEN
        tim5.write_cnt(0); // CNT == CCR1's reset value

        tim5.poll(&sys);

        assert_eq!(tim5.read(&sys, 0x0010) & Tim5::SR_CC1IF, 0);
    }

    #[test]
    fn compare_match_sets_status_flag_but_does_not_interrupt_while_masked() {
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };
        let mut tim5 = Tim5::default();

        tim5.write_cr1(1); // CEN
        tim5.write(&sys, 0x0034, 100); // CCR1 = 100, arms channel 1
        tim5.write_cnt(200); // CNT has passed CCR1

        tim5.poll(&sys);

        assert_ne!(
            tim5.read(&sys, 0x0010) & Tim5::SR_CC1IF,
            0,
            "SR.CC1IF must latch on compare match regardless of DIER"
        );
        assert_eq!(
            sys.p.nvic.borrow_mut().get_and_clear_next_intr_pending(),
            None,
            "must not reach the NVIC while DIER.CC1IE is disabled"
        );
    }

    #[test]
    fn compare_match_raises_tim5_interrupt_once_the_mask_is_enabled() {
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };
        let mut tim5 = Tim5::default();

        tim5.write_cr1(1); // CEN
        tim5.write(&sys, 0x0034, 100); // CCR1 = 100, arms channel 1
        tim5.write(&sys, 0x000c, Tim5::DIER_CC1IE);
        tim5.write_cnt(200); // CNT has passed CCR1

        tim5.poll(&sys);

        assert_eq!(
            sys.p.nvic.borrow_mut().get_and_clear_next_intr_pending(),
            Some(super::TIM5_IRQ)
        );
    }
}
