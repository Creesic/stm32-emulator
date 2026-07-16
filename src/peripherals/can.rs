// SPDX-License-Identifier: GPL-3.0-or-later

//! Minimal bxCAN readiness model (trace-constrained, like `Pwr`/`Rtc`/`Flash`).
//!
//! ChibiOS's `can_lld_start` (hal_can_lld.c:704-706) writes `MCR = CAN_MCR_INRQ`
//! and then sleeps one tick at a time until `MSR.INAK` reads set; with CAN left
//! as a generic register-storage peripheral that never happens, so rusEFI's
//! init thread parks inside `initCan()` forever and nothing after it in
//! `initHardware()` (trigger-input EXTI setup, console command registration,
//! trigger emulator) ever runs. Mirror the mode-request bits into the
//! acknowledge bits and report the TX mailboxes as always empty; no frames are
//! ever transferred.

use super::Peripheral;
use crate::system::System;

pub struct Can {
    mcr: u32,
}

impl Can {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name.starts_with("CAN") {
            Some(Box::new(Can { mcr: 0 }))
        } else {
            None
        }
    }

    /// MSR.INAK (bit 0) acknowledges MCR.INRQ (bit 0); MSR.SLAK (bit 1)
    /// acknowledges MCR.SLEEP (bit 1). Both transitions are instantaneous.
    pub(crate) fn msr_after_mcr_write(mcr: u32) -> u32 {
        mcr & 0x0000_0003
    }

    /// TSR with TME0/TME1/TME2 (bits 26-28) permanently set: every transmit
    /// mailbox is always free, so `canTransmit` never blocks a firmware
    /// thread waiting for a mailbox that can never drain.
    const TSR_ALL_MAILBOXES_EMPTY: u32 = 0x1c00_0000;
}

impl Peripheral for Can {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => self.mcr,
            0x0004 => Self::msr_after_mcr_write(self.mcr),
            0x0008 => Self::TSR_ALL_MAILBOXES_EMPTY,
            _ => 0,
        }
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        match offset {
            0x0000 => self.mcr = value,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Can;

    #[test]
    fn init_mode_request_is_acknowledged_immediately() {
        // ChibiOS can_lld_start: MCR = CAN_MCR_INRQ, then wait for MSR.INAK.
        assert_eq!(Can::msr_after_mcr_write(0x0000_0001) & 0x0000_0001, 0x0000_0001);
    }

    #[test]
    fn leaving_init_mode_clears_the_acknowledge() {
        // can_lld_start then writes the config MCR (no INRQ), e.g. ABOM|AWUM.
        assert_eq!(Can::msr_after_mcr_write(0x0000_0060) & 0x0000_0001, 0);
    }

    #[test]
    fn sleep_request_is_acknowledged_immediately() {
        assert_eq!(Can::msr_after_mcr_write(0x0000_0002) & 0x0000_0002, 0x0000_0002);
    }

    #[test]
    fn transmit_mailboxes_always_report_empty() {
        assert_eq!(Can::TSR_ALL_MAILBOXES_EMPTY & 0x1c00_0000, 0x1c00_0000);
    }

    #[test]
    fn only_can_peripherals_match() {
        assert!(Can::new("CAN1").is_some());
        assert!(Can::new("CAN3").is_some());
        assert!(Can::new("PWR").is_none());
    }
}
