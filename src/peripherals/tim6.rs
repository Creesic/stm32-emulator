// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::Ordering;

use super::Peripheral;
use crate::system::System;

/// TIM6 update interrupt on STM32F767 (shared with the DAC).
const TIM6_DAC_IRQ: i32 = 54;

/// Minimal STM32F7 basic-timer model.
///
/// EpicEFI uses GPTD6 as the 10 kHz trigger for its fast ADC (MAP/MAF).
/// Without this timer ADC2 is configured but never starts a conversion.
#[derive(Default)]
pub struct Tim6 {
    cr1: u32,
    cr2: u32,
    dier: u32,
    sr: u32,
    psc: u32,
    arr: u32,
    enabled_at: u64,
}

impl Tim6 {
    const CR1_CEN: u32 = 1;
    const DIER_UIE: u32 = 1;
    const SR_UIF: u32 = 1;

    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        (name == "TIM6").then(|| Box::new(Self::default()) as Box<dyn Peripheral>)
    }

    fn now() -> u64 {
        crate::emulator::NUM_INSTRUCTIONS.load(Ordering::Relaxed)
    }

    /// TIM6 is clocked at 108 MHz while the Cortex-M7 core runs at 216 MHz.
    /// PSC and ARR are both programmed as value-minus-one by ChibiOS.
    fn period_instructions(psc: u32, arr: u32) -> u64 {
        2_u64
            .saturating_mul(psc as u64 + 1)
            .saturating_mul(arr as u64 + 1)
            .max(1)
    }

    fn enabled(&self) -> bool {
        self.cr1 & Self::CR1_CEN != 0
    }

    fn counter(&self) -> u32 {
        if !self.enabled() {
            return 0;
        }
        let timer_instructions = 2_u64.saturating_mul(self.psc as u64 + 1);
        ((Self::now().saturating_sub(self.enabled_at) / timer_instructions)
            % (self.arr as u64 + 1).max(1)) as u32
    }
}

impl Peripheral for Tim6 {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x00 => self.cr1,
            0x04 => self.cr2,
            0x0c => self.dier,
            0x10 => self.sr,
            0x24 => self.counter(),
            0x28 => self.psc,
            0x2c => self.arr,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x00 => {
                let was_enabled = self.enabled();
                self.cr1 = value;
                if self.enabled() && !was_enabled {
                    self.enabled_at = Self::now();
                }
            }
            0x04 => self.cr2 = value,
            0x0c => self.dier = value,
            0x10 => self.sr &= value, // rc_w0: writing zero clears UIF
            0x14 => {
                // EGR.UG reloads PSC/ARR and restarts the period.
                if value & 1 != 0 {
                    self.enabled_at = Self::now();
                }
            }
            0x24 => {
                // GPT only writes CNT=0; treating every write as a restart is
                // sufficient and preserves the expected phase.
                self.enabled_at = Self::now();
            }
            0x28 => self.psc = value & 0xffff,
            0x2c => self.arr = value,
            _ => {}
        }
    }

    fn poll(&mut self, sys: &System) {
        if self.enabled()
            && Self::now().saturating_sub(self.enabled_at)
                >= Self::period_instructions(self.psc, self.arr)
        {
            self.sr |= Self::SR_UIF;
            self.enabled_at = Self::now();
        }

        if self.sr & Self::SR_UIF != 0 && self.dier & Self::DIER_UIE != 0 {
            sys.p.nvic.borrow_mut().set_intr_pending(TIM6_DAC_IRQ);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Tim6;

    #[test]
    fn epic_efi_fast_adc_configuration_is_ten_khz() {
        // GPT frequency 100 kHz: PSC=(108 MHz / 100 kHz)-1.
        // GPT period 10: ARR=10-1. At a 216 MHz core that is 21,600
        // instructions per interrupt, or exactly 10 kHz.
        assert_eq!(Tim6::period_instructions(1_079, 9), 21_600);
    }
}
