// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::Ordering;

use unicorn_engine::{RegisterARM, Unicorn};

use super::Peripheral;
use crate::system::System;

#[derive(Default)]
pub struct Nvic {
    pub systick_period: Option<u32>,
    pub last_systick_trigger: u64,

    // 128 different interrupts. Good enough for now
    pending: u128,
    in_interrupt: bool,
}

const IRQ_OFFSET: i32 = 16;

// ITSTATE (IT-block execution/predication state) is split across two XPSR
// fields: bits [15:10] hold ITSTATE[7:2], bits [26:25] hold ITSTATE[1:0].
const XPSR_ITSTATE_MASK: u32 = 0x0600_fc00;

pub mod irq {
    pub const PENDSV: i32 = -2;
    pub const SYSTICK: i32 = -1;
    // SVCall is architecturally fixed exception #11 (vector table word 11);
    // -5 here makes read_vector_addr's (IRQ_OFFSET + irq) formula land on it.
    pub const SVCALL: i32 = -5;
}

// This is all poorly implemented. If this is not making much sense, it might be
// best to re-implement everything correctly. Right now, I'm just trying to get
// the saturn firmware to work just well enough.

impl Nvic {
    pub fn set_intr_pending(&mut self, irq: i32) {
        trace!("Set irq pending irq={}", irq);
        let bit = IRQ_OFFSET + irq;
        assert!(bit > 0);
        self.pending |= 1 << (IRQ_OFFSET + irq);
    }

    pub fn get_and_clear_next_intr_pending(&mut self) -> Option<i32> {
        if self.pending != 0 {
            let bit = self.pending.trailing_zeros();
            self.pending &= !(1 << bit);
            let irq = (bit as i32) - IRQ_OFFSET;
            Some(irq)
        } else {
            None
        }
    }

    pub fn maybe_set_systick_intr_pending(&mut self) {
        if let Some(systick_period) = self.systick_period {
            let n = crate::emulator::NUM_INSTRUCTIONS.load(Ordering::Relaxed);
            let delta_num_instructions = n - self.last_systick_trigger;
            if delta_num_instructions > (systick_period as u64) {
                self.last_systick_trigger = n;
                self.set_intr_pending(irq::SYSTICK);
            }
        }
    }

    fn are_interrupts_disabled(sys: &System) -> bool {
        let uc = sys.uc.borrow();
        let primask = uc.reg_read(RegisterARM::PRIMASK).unwrap();
        // ChibiOS's chSysLock()/chSysUnlock() (and this ARMv7-M port's own
        // __port_irq_epilogue) mask interrupts via BASEPRI, not PRIMASK. We
        // don't model per-interrupt priorities, so treat any nonzero BASEPRI
        // as masking everything -- SVCall is entered directly by emulator.rs
        // regardless (matching this port's SVCall priority being configured
        // above any BASEPRI it raises).
        let basepri = uc.reg_read(RegisterARM::BASEPRI).unwrap();
        primask != 0 || basepri != 0
    }

    pub fn run_pending_interrupts(&mut self, sys: &System, vector_table_addr: u32) {
        self.maybe_set_systick_intr_pending();

        if Self::are_interrupts_disabled(sys) || self.in_interrupt {
            return;
        }

        if let Some(irq) = self.get_and_clear_next_intr_pending() {
            self.run_interrupt(sys, vector_table_addr, irq);
        }
    }

    fn read_vector_addr(sys: &System, vector_table_addr: u32, irq: i32) -> u32 {
        // 4 because of ptr size
        let vaddr = vector_table_addr + 4 * (IRQ_OFFSET + irq) as u32;

        let mut vector = [0, 0, 0, 0];
        sys.uc.borrow().mem_read(vaddr as u64, &mut vector).unwrap();
        u32::from_le_bytes(vector)
    }

    // SPSEL, bit[1], 0 means we use MSP, 1 means we use PSP.
    // FPCA, bit[2], if the processor includes the FP extension.

    fn run_interrupt(&mut self, sys: &System, vector_table_addr: u32, irq: i32) {
        let vector = Self::read_vector_addr(sys, vector_table_addr, irq);

        let mut uc = sys.uc.borrow_mut();

        // SPSEL, bit[1], 0 means we use MSP, 1 means we use PSP.
        // FPCA, bit[2], if the processor includes the FP extension.
        let control_reg = uc.reg_read(RegisterARM::CONTROL).unwrap();
        let spsel = control_reg & (1 << 1) != 0;
        let fpca = control_reg & (2 << 1) != 0;

        trace!(
            "Running interrupt irq={} spsel={} fpca={} vector={:#08x}",
            irq,
            spsel,
            fpca,
            vector
        );

        Self::push_regs(&mut uc, spsel, fpca);

        // Real hardware clears ITSTATE unconditionally on exception entry --
        // the interrupted context's IT state was just preserved in the xPSR
        // pushed above, and return_from_interrupt's pop_regs restores it.
        // Without this, an interrupt taken between an IT instruction and its
        // predicated followers leaves stale predication state active for
        // the handler's own first instructions.
        let xpsr = uc.reg_read(RegisterARM::XPSR).unwrap() as u32;
        uc.reg_write(RegisterARM::XPSR, (xpsr & !XPSR_ITSTATE_MASK) as u64)
            .unwrap();

        // LR meaning:
        //   EXC_RETURN    Return to      Return stack Frame type
        //   0xFFFF_FFE1   Handler mode   Main         Extended
        //   0xFFFF_FFE9   Thread mode    Main         Extended
        //   0xFFFF_FFED   Thread mode    Process      Extended
        //   0xFFFF_FFF1   Handler mode   Main         Basic
        //   0xFFFF_FFF9   Thread mode    Main         Basic
        //   0xFFFF_FFFD   Thread mode    Process      Basic

        // Right now, we don't supposed nested interrupts.
        let mut lr: u32 = 0xFFFF_FFE9;
        if spsel {
            lr |= 0b0000_0100;
        }
        if !fpca {
            lr |= 0b0001_0000;
        } // Yes, no fpca means the bit is set
        uc.reg_write(RegisterARM::LR, lr.into()).unwrap();

        uc.reg_write(RegisterARM::IPSR, irq as u64).unwrap();
        uc.reg_write(RegisterARM::PC, vector as u64).unwrap();

        self.in_interrupt = true;
    }

    /// Enters the SVCall exception. Unlike peripheral IRQs, an `svc`
    /// instruction is a synchronous, firmware-requested trap (used by
    /// ChibiOS's ARMv7-M port for its actual scheduler context switch), so
    /// it's entered directly rather than through the pending-IRQ queue.
    pub fn enter_svcall(&mut self, sys: &System, vector_table_addr: u32) {
        self.run_interrupt(sys, vector_table_addr, irq::SVCALL);
    }

    pub fn return_from_interrupt(&mut self, sys: &System) {
        let mut uc = sys.uc.borrow_mut();

        let lr = uc.reg_read(RegisterARM::LR).unwrap();
        if lr & 0xFFFF_FF00 == 0xFFFF_FF00 {
            let spsel = lr & 0b0000_0100 != 0;
            let fpca = lr & 0b0001_0000 == 0; // 0 means yes here

            Self::pop_regs(&mut uc, spsel, fpca);

            trace!(
                "Return from interrupt spsel={} fpca={} pc=0x{:08x}",
                spsel,
                fpca,
                uc.reg_read(RegisterARM::PC).unwrap()
            );

            // SPSEL, bit[1], 0 means we use MSP, 1 means we use PSP.
            // FPCA, bit[2], if the processor includes the FP extension.
            let mut control_reg = 0;
            if spsel {
                control_reg |= 1 << 1;
            }
            if fpca {
                control_reg |= 2 << 1;
            }
            uc.reg_write(RegisterARM::CONTROL, control_reg).unwrap();
        } else {
            let control_reg = uc.reg_read(RegisterARM::CONTROL).unwrap();
            let spsel = control_reg & (1 << 1) != 0;
            let fpca = control_reg & (2 << 1) != 0;
            Self::pop_regs(&mut uc, spsel, fpca);

            trace!(
                "Return from interrupt spsel={} fpca={} pc=0x{:08x} -- LR was not right",
                spsel,
                fpca,
                uc.reg_read(RegisterARM::PC).unwrap()
            );
        }

        self.in_interrupt = false;
    }

    const CONTEXT_REGS_EXTENDED: [RegisterARM; 17] = [
        RegisterARM::FPSCR,
        RegisterARM::S15,
        RegisterARM::S14,
        RegisterARM::S13,
        RegisterARM::S12,
        RegisterARM::S11,
        RegisterARM::S10,
        RegisterARM::S9,
        RegisterARM::S8,
        RegisterARM::S7,
        RegisterARM::S6,
        RegisterARM::S5,
        RegisterARM::S4,
        RegisterARM::S3,
        RegisterARM::S2,
        RegisterARM::S1,
        RegisterARM::S0,
    ];

    const CONTEXT_REGS: [RegisterARM; 8] = [
        RegisterARM::XPSR,
        RegisterARM::PC,
        RegisterARM::LR,
        RegisterARM::R12,
        RegisterARM::R3,
        RegisterARM::R2,
        RegisterARM::R1,
        RegisterARM::R0,
    ];

    fn push_regs(uc: &mut Unicorn<()>, spsel: bool, fpca: bool) {
        let sp_reg = if spsel {
            RegisterARM::PSP
        } else {
            RegisterARM::MSP
        };
        let mut sp = uc.reg_read(sp_reg).unwrap();

        let mut push_word = |uc: &mut Unicorn<()>, v: u32| {
            sp -= 4;
            uc.mem_write(sp, &v.to_le_bytes())
                .expect("Invalid SP pointer during interrupt");
        };

        if fpca {
            // Real Cortex-M7 extended frames are 104 bytes (26 words): a
            // reserved word for 8-byte stack alignment sits above FPSCR, at
            // the frame's highest address, so (sp only decreasing from here)
            // it's pushed first. Firmware that computes frame size from
            // this constant (e.g. ChibiOS's ARMv7-M port) depends on it.
            push_word(uc, 0);
            for reg in Self::CONTEXT_REGS_EXTENDED {
                let v = uc.reg_read(reg).unwrap() as u32;
                push_word(uc, v);
            }
        }
        for reg in Self::CONTEXT_REGS {
            let v = uc.reg_read(reg).unwrap() as u32;
            push_word(uc, v);
        }
        uc.reg_write(RegisterARM::SP, sp).unwrap();
    }

    fn pop_regs(uc: &mut Unicorn<()>, spsel: bool, fpca: bool) {
        let sp_reg = if spsel {
            RegisterARM::PSP
        } else {
            RegisterARM::MSP
        };
        let mut sp = uc.reg_read(sp_reg).unwrap();

        let mut pop_word = |uc: &mut Unicorn<()>| -> u32 {
            let mut v = [0, 0, 0, 0];
            uc.mem_read(sp, &mut v)
                .expect("Invalid SP pointer during interrupt return");
            sp += 4;
            u32::from_le_bytes(v)
        };

        for reg in Self::CONTEXT_REGS.iter().rev() {
            let v = pop_word(uc);
            uc.reg_write(*reg, v as u64).unwrap();
        }
        if fpca {
            for reg in Self::CONTEXT_REGS_EXTENDED.iter().rev() {
                let v = pop_word(uc);
                uc.reg_write(*reg, v as u64).unwrap();
            }
            // Discard the reserved alignment word pushed above FPSCR (see push_regs).
            sp += 4;
        }
        uc.reg_write(RegisterARM::SP, sp).unwrap();
    }
}

impl Peripheral for Nvic {
    fn read(&mut self, _sys: &System, _offset: u32) -> u32 {
        0
    }

    fn write(&mut self, _sys: &System, _offset: u32, _value: u32) {}
}

/// The next part is glue. Maybe we could have a better architecture.

pub struct NvicWrapper;

impl NvicWrapper {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name == "NVIC" {
            Some(Box::new(Self))
        } else {
            None
        }
    }
}

impl Peripheral for NvicWrapper {
    fn read(&mut self, sys: &System, offset: u32) -> u32 {
        sys.p.nvic.borrow_mut().read(sys, offset)
    }

    fn write(&mut self, sys: &System, offset: u32, value: u32) {
        sys.p.nvic.borrow_mut().write(sys, offset, value)
    }
}

/*
0xE000E100 B  REGISTER ISER0 (rw): Interrupt Set-Enable Register
0xE000E104 B  REGISTER ISER1 (rw): Interrupt Set-Enable Register
0xE000E108 B  REGISTER ISER2 (rw): Interrupt Set-Enable Register

0xE000E180 B  REGISTER ICER0 (rw): Interrupt Clear-Enable Register
0xE000E184 B  REGISTER ICER1 (rw): Interrupt Clear-Enable Register
0xE000E188 B  REGISTER ICER2 (rw): Interrupt Clear-Enable Register

0xE000E200 B  REGISTER ISPR0 (rw): Interrupt Set-Pending Register
0xE000E204 B  REGISTER ISPR1 (rw): Interrupt Set-Pending Register
0xE000E208 B  REGISTER ISPR2 (rw): Interrupt Set-Pending Register

0xE000E280 B  REGISTER ICPR0 (rw): Interrupt Clear-Pending Register
0xE000E284 B  REGISTER ICPR1 (rw): Interrupt Clear-Pending Register
0xE000E288 B  REGISTER ICPR2 (rw): Interrupt Clear-Pending Register

0xE000E300 B  REGISTER IABR0 (ro): Interrupt Active Bit Register
0xE000E304 B  REGISTER IABR1 (ro): Interrupt Active Bit Register
0xE000E308 B  REGISTER IABR2 (ro): Interrupt Active Bit Register

0xE000E400 B  REGISTER IPR0 (rw): Interrupt Priority Register
0xE000E404 B  REGISTER IPR1 (rw): Interrupt Priority Register
0xE000E408 B  REGISTER IPR2 (rw): Interrupt Priority Register
0xE000E40C B  REGISTER IPR3 (rw): Interrupt Priority Register
0xE000E410 B  REGISTER IPR4 (rw): Interrupt Priority Register
0xE000E414 B  REGISTER IPR5 (rw): Interrupt Priority Register
0xE000E418 B  REGISTER IPR6 (rw): Interrupt Priority Register
0xE000E41C B  REGISTER IPR7 (rw): Interrupt Priority Register
0xE000E420 B  REGISTER IPR8 (rw): Interrupt Priority Register
0xE000E424 B  REGISTER IPR9 (rw): Interrupt Priority Register
0xE000E428 B  REGISTER IPR10 (rw): Interrupt Priority Register
0xE000E42C B  REGISTER IPR11 (rw): Interrupt Priority Register
0xE000E430 B  REGISTER IPR12 (rw): Interrupt Priority Register
0xE000E434 B  REGISTER IPR13 (rw): Interrupt Priority Register
0xE000E438 B  REGISTER IPR14 (rw): Interrupt Priority Register
0xE000E43C B  REGISTER IPR15 (rw): Interrupt Priority Register
0xE000E440 B  REGISTER IPR16 (rw): Interrupt Priority Register
0xE000E444 B  REGISTER IPR17 (rw): Interrupt Priority Register
0xE000E448 B  REGISTER IPR18 (rw): Interrupt Priority Register
0xE000E44C B  REGISTER IPR19 (rw): Interrupt Priority Register
*/

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use unicorn_engine::{
        unicorn_const::{Arch, Mode, Prot},
        ArmCpuModel, RegisterARM, Unicorn,
    };

    use super::{Nvic, XPSR_ITSTATE_MASK};
    use crate::{ext_devices::ExtDevices, peripherals::Peripherals, system::System};

    fn test_parts() -> (Unicorn<'static, ()>, Rc<Peripherals>, Rc<ExtDevices>) {
        let mut uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        uc.ctl_set_cpu_model(ArmCpuModel::CORTEX_M4 as i32).unwrap();
        (uc, Rc::new(Peripherals::default()), Rc::new(ExtDevices::default()))
    }

    #[test]
    fn run_interrupt_clears_itstate_before_entering_the_handler() {
        let (mut uc, p, d) = test_parts();
        uc.mem_map(0x0000_0000, 0x1000, Prot::ALL).unwrap();
        uc.mem_map(0x2000_0000, 0x1000, Prot::ALL).unwrap();
        // Vector table word for IRQ 0 (vector index IRQ_OFFSET + 0 = 16).
        uc.mem_write(4 * 16, &0x0000_1001u32.to_le_bytes()).unwrap();
        uc.reg_write(RegisterARM::MSP, 0x2000_0100).unwrap();
        uc.reg_write(RegisterARM::CONTROL, 0).unwrap();
        // Simulate the interrupted context being mid-IT-block.
        uc.reg_write(RegisterARM::XPSR, 0x0600_fc00).unwrap();

        let sys = System { uc: RefCell::new(&mut uc), p, d };
        Nvic::default().run_interrupt(&sys, 0x0000_0000, 0);

        let xpsr = sys.uc.borrow_mut().reg_read(RegisterARM::XPSR).unwrap() as u32;
        assert_eq!(
            xpsr & XPSR_ITSTATE_MASK,
            0,
            "ITSTATE bits must be cleared on interrupt entry"
        );
    }
}
