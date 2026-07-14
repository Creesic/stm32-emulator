// SPDX-License-Identifier: GPL-3.0-or-later

use super::{nvic::irq, Peripheral};
use crate::system::System;

#[derive(Default)]
pub struct Scb {
    cpacr: u32,
}

impl Scb {
    // ICSR bit 11 (RETTOBASE): set when no other exception is active, so the
    // current one is returning to base (thread) level. ChibiOS's ARMv7-M port
    // epilogue only performs its post-IRQ thread switch when it observes this
    // bit set; since our NVIC never nests interrupts, it's always set.
    pub const ICSR_RETTOBASE: u32 = 1 << 11;

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

    fn read_icsr(&self, sys: &System) -> u32 {
        // VECTACTIVE, bits [8:0]: the exception number currently running
        // (0 in thread mode). rusEFI's assertInterruptPriority() derives
        // the IRQ whose NVIC_IPRn it should check from this field, via
        // Nvic -- the same struct that actually tracks which interrupt is
        // active, since nothing else here does.
        Self::ICSR_RETTOBASE | sys.p.nvic.borrow().active_exception_number()
    }
}

impl Peripheral for Scb {
    fn read(&mut self, sys: &System, offset: u32) -> u32 {
        match offset {
            0x0004 => self.read_icsr(sys),
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
    use std::{cell::RefCell, rc::Rc};

    use unicorn_engine::{
        unicorn_const::{Arch, Mode},
        ArmCpuModel, Unicorn,
    };

    use super::Scb;
    use crate::{ext_devices::ExtDevices, peripherals::Peripherals, system::System};

    fn test_parts() -> (Unicorn<'static, ()>, Rc<Peripherals>, Rc<ExtDevices>) {
        let mut uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        uc.ctl_set_cpu_model(ArmCpuModel::CORTEX_M4 as i32).unwrap();
        (uc, Rc::new(Peripherals::default()), Rc::new(ExtDevices::default()))
    }

    #[test]
    fn cpacr_retains_the_firmware_fpu_enable_value() {
        let mut scb = Scb::default();
        scb.write_cpacr(0x00f0_0000);

        assert_eq!(scb.read_cpacr(), 0x00f0_0000);
    }

    #[test]
    fn icsr_read_reports_rettobase_since_nested_interrupts_are_unsupported() {
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };
        let scb = Scb::default();
        assert_eq!(scb.read_icsr(&sys) & Scb::ICSR_RETTOBASE, Scb::ICSR_RETTOBASE);
    }

    #[test]
    fn icsr_read_reports_vectactive_from_nvics_currently_running_exception() {
        // rusEFI's assertInterruptPriority() derives which NVIC_IPRn to
        // check from ICSR.VECTACTIVE; without this, it always read back
        // "thread mode" (0) regardless of which interrupt was actually
        // running, silently checking the wrong (always-zero) priority byte.
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };
        let scb = Scb::default();

        assert_eq!(sys.p.nvic.borrow().active_exception_number(), 0);
        assert_eq!(scb.read_icsr(&sys) & 0x1ff, 0);

        // TIM5's exception number is IRQ_OFFSET (16) + 50 = 66.
        sys.p.nvic.borrow_mut().set_active_exception_number_for_test(66);
        assert_eq!(scb.read_icsr(&sys) & 0x1ff, 66);
    }
}
