// SPDX-License-Identifier: GPL-3.0-or-later

use std::{mem::MaybeUninit, sync::atomic::{AtomicU64, Ordering, AtomicBool}, cell::RefCell};
use svd_parser::svd::Device as SvdDevice;
use unicorn_engine::{unicorn_const::{Arch, Mode, HookType, MemType}, ArmCpuModel, Unicorn, RegisterARM};
use crate::{config::{Config, CpuModel}, util::UniErr, Args, system::System, framebuffers::sdl_engine::{PUMP_EVENT_INST_INTERVAL, SDL}};
use anyhow::{Context as _, Result, bail};
use capstone::prelude::*;

#[repr(C)]
struct VectorTable {
    pub sp: u32,
    pub reset: u32,
}

impl VectorTable {
    pub fn from_memory(uc: &Unicorn<()>, addr: u32) -> Result<Self> {
        unsafe {
            let mut self_ = MaybeUninit::<Self>::uninit();
            let buf = std::slice::from_raw_parts_mut(self_.as_mut_ptr() as *mut u8, std::mem::size_of::<Self>());
            uc.mem_read(addr.into(), buf).map_err(UniErr)?;
            Ok(self_.assume_init())
        }
    }
}

fn thumb(pc: u64) -> u64 {
    pc | 1
}

/// True when a fetch address can only be an EXC_RETURN magic value
/// (0xFFFFFFE1..=0xFFFFFFFD, i.e. an exception-return branch target).
/// Nothing is ever mapped in the top page, so a prefetch abort there is
/// an exception return whose branch ran in a translation block that was
/// compiled without the magic-address check -- see the intr_hook's
/// intno=3 arm below. Masked (not exact-matched) so both the raw magic
/// value and its Thumb-bit-cleared fetch address qualify.
fn is_exception_return_pc(pc: u32) -> bool {
    pc & 0xFFFF_FF00 == 0xFFFF_FF00
}

// Last translation-block start address + size in bytes (approximates the
// current PC for progress logging and detects self-looping blocks for -b).
pub static mut LAST_INSTRUCTION: (u32, u8) = (0,0);
pub static NUM_INSTRUCTIONS: AtomicU64 = AtomicU64::new(0);
static CONTINUE_EXECUTION: AtomicBool = AtomicBool::new(false);
static BUSY_LOOP_REACHED: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

fn initialize_arm_engine(model: CpuModel) -> Result<Unicorn<'static, ()>> {
    // MCLASS is deprecated by Unicorn's CPU-model control and overrides the
    // selected Cortex-M model with Cortex-M33 during CPU initialization.
    let mut uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN)
        .map_err(UniErr)
        .context("Failed to initialize Unicorn instance")?;
    let unicorn_model = match model {
        CpuModel::CortexM4 => ArmCpuModel::CORTEX_M4 as i32,
        CpuModel::CortexM7 => ArmCpuModel::CORTEX_M7 as i32,
    };
    debug!("Selecting ARM CPU model {:?}", model);

    uc.ctl_set_cpu_model(unicorn_model)
        .map_err(UniErr)
        .context("Failed to select configured ARM CPU model")?;

    Ok(uc)
}

fn disassemble_instruction(diassembler: &Capstone, uc: &Unicorn<()>, pc: u64) -> String {
    let mut instr = [0; 4];
    if uc.mem_read(pc, &mut instr).is_err() {
        return "failed to read memory at pc".to_string();
    }

    if let Ok(disasm) = diassembler.disasm_count(&instr, pc, 1) {
        if let Some(instr) = disasm.first() {
            return format!("{:5} {}", instr.mnemonic().unwrap(), instr.op_str().unwrap());
        }
    }

    return "??".to_string();
}

fn stop_address_state(pc: u32, r3: u32) -> String {
    format!("Stop address reached at pc=0x{pc:08x} r3=0x{r3:08x}")
}

pub fn dump_stack(uc: &mut Unicorn<()>, count: usize) {
    let mut sp = uc.reg_read(RegisterARM::SP).unwrap();

    for _ in 0..count {
        let mut v = [0,0,0,0];
        if uc.mem_read(sp, &mut v).is_err() {
            info!("stack dump finished due to mem read error");
            return;
        }
        let v = u32::from_le_bytes(v);

        if (0x0800_0000..0x0810_0000).contains(&v) {
            // Probably a return address
            info!("*** 0x{:08x} (sp=0x{:08x})", v, sp);
        } else {
            info!("    0x{:08x} (sp=0x{:08x})", v, sp);
        }

        sp += 4;
    }
}

pub fn run_emulator(config: Config, svd_device: SvdDevice, args: Args) -> Result<()> {
    let mut uc = initialize_arm_engine(config.cpu.model)?;

    let vector_table_addr = config.cpu.vector_table;

    let (sys, framebuffers) = crate::system::prepare(&mut uc, config, svd_device)?;

    // One hook per *translation block*, not per instruction. Unicorn injects
    // the hook call into the generated code, so a per-instruction code hook
    // costs an FFI round trip on every single instruction -- measured at
    // roughly half of total runtime (8.3M instr/s with the old hook doing
    // per-instruction NVIC bookkeeping, 15.8M with it skipped). Every job the
    // old hook had works at block granularity:
    //  - The emulated clock advances by the block's *halfword* count: Thumb
    //    instructions are 2 or 4 bytes, so halfwords approximate cycles at
    //    least as well as the old "1 instruction = 1 cycle" did, and every
    //    derived timebase (SysTick period, TIM5, DWT CYCCNT, DMA deadlines,
    //    the ecu_io trigger wheel) reads this same counter, so they all stay
    //    mutually consistent.
    //  - Interrupt dispatch happens at block entry, which is where
    //    run_interrupt's PC-write + emu_stop() redirect is well-defined
    //    anyway (the hook fires before the block's first instruction
    //    executes; pinned by the characterization tests below).
    //  - Busy-loop detection and interrupt delivery inside spin loops keep
    //    working because the injected hook call is part of the block body:
    //    a self-looping block re-fires it on every iteration.
    // Per-instruction disassembly tracing (-vvvv) installs its own dedicated
    // code hook below and keeps its old (slow) behavior.
    {
        let busy_loop_stop = args.busy_loop_stop;
        let p = sys.p.clone();
        let d = sys.d.clone();
        let interrupt_period = args.interrupt_period as u64;
        let mut last_interrupt_check: u64 = 0;
        let mut last_pump: u64 = 0;
        sys.uc.borrow_mut().add_block_hook(0, u64::MAX, move |uc, addr, size| {
            unsafe {
                // A block of <= 4 bytes jumping to itself is a busy loop
                // (`b .`, or a two-halfword self-loop). The old
                // per-instruction check only caught single-instruction
                // loops; block granularity catches the same and slightly
                // more, which serves -b's purpose (stop at an infinite
                // loop) equally well.
                if busy_loop_stop && size <= 4 && LAST_INSTRUCTION.0 == addr as u32 {
                    info!("Busy loop reached");
                    uc.emu_stop().unwrap();
                    BUSY_LOOP_REACHED.store(true, Ordering::Release);
                }
                LAST_INSTRUCTION = (addr as u32, size as u8);
            }

            let n = NUM_INSTRUCTIONS.fetch_add((size as u64 + 1) / 2, Ordering::Acquire);

            // The counter now advances in jumps, so both gates are
            // deadline-based rather than the old exact `n % period == 0` /
            // `n & mask == 0` forms (which a jump could step over).
            if n.wrapping_sub(last_interrupt_check) >= interrupt_period {
                last_interrupt_check = n;
                let sys = System { uc: RefCell::new(uc), p: p.clone(), d: d.clone() };
                p.nvic.borrow_mut().run_pending_interrupts(&sys, vector_table_addr);
            }

            if n.wrapping_sub(last_pump) >= PUMP_EVENT_INST_INTERVAL + 1 {
                last_pump = n;
                let sys = System { uc: RefCell::new(uc), p: p.clone(), d: d.clone() };
                d.poll(&sys);
                p.poll(&sys);
                for fb in &framebuffers.sdls {
                    fb.borrow_mut().maybe_redraw();
                }
                if !SDL.lock().unwrap().pump_events(&framebuffers.sdls) {
                    STOP_REQUESTED.store(true, Ordering::Relaxed);
                    uc.emu_stop().unwrap();
                }
            }
        }).expect("add_block_hook failed");
    }

    if crate::verbose() >= 4 {
        let diassembler = Capstone::new()
            .arm()
            .mode(arch::arm::ArchMode::Thumb)
            .build()
            .expect("failed to initialize capstone");
        sys.uc.borrow_mut().add_code_hook(0, u64::MAX, move |uc, pc, _size| {
            info!("{}", disassemble_instruction(&diassembler, uc, pc));
        }).expect("add_code_hook failed");
    }

    {
        let p = sys.p.clone();
        let d = sys.d.clone();
        sys.uc.borrow_mut().add_intr_hook(move |uc, exception| {
            match exception {
                /*
                    EXCP_UDEF            1   /* undefined instruction */
                    EXCP_SWI             2   /* software interrupt */
                    EXCP_PREFETCH_ABORT  3
                    EXCP_DATA_ABORT      4
                    EXCP_IRQ             5
                    EXCP_FIQ             6
                    EXCP_BKPT            7
                    EXCP_EXCEPTION_EXIT  8   /* Return from v7M exception.  */
                    EXCP_KERNEL_TRAP     9   /* Jumped to kernel code page.  */
                    EXCP_HVC            11   /* HyperVisor Call */
                    EXCP_HYP_TRAP       12
                    EXCP_SMC            13   /* Secure Monitor Call */
                    EXCP_VIRQ           14
                    EXCP_VFIQ           15
                    EXCP_SEMIHOST       16   /* semihosting call */
                    EXCP_NOCP           17   /* v7M NOCP UsageFault */
                    EXCP_INVSTATE       18   /* v7M INVSTATE UsageFault */
                    EXCP_STKOF          19   /* v8M STKOF UsageFault */
                    EXCP_LAZYFP         20   /* v7M fault during lazy FP stacking */
                    EXCP_LSERR          21   /* v8M LSERR SecureFault */
                    EXCP_UNALIGNED      22   /* v7M UNALIGNED UsageFault */
                    */
                8 => {
                    // Return from interrupt
                    let sys = System { uc: RefCell::new(uc), p: p.clone(), d: d.clone() };
                    p.nvic.borrow_mut().return_from_interrupt(&sys);
                    p.nvic.borrow_mut().run_pending_interrupts(&sys, vector_table_addr);
                }
                2 => {
                    // svc instruction. Some RTOS ports (e.g. ChibiOS's ARMv7-M
                    // port) use this instead of PendSV to perform their
                    // scheduler context switch from within an ISR epilogue.
                    let sys = System { uc: RefCell::new(uc), p: p.clone(), d: d.clone() };
                    p.nvic.borrow_mut().enter_svcall(&sys, vector_table_addr);
                }
                3 => {
                    // Unicorn compiles the EXC_RETURN magic-branch detection
                    // into a translation block only when the block was
                    // translated in handler mode (gen_bx_excret gates on the
                    // HANDLER tb-flag) -- and its IPSR/XPSR register writes
                    // update env->v7m.exception WITHOUT rebuilding the cached
                    // hflags that feed that tb-flag (unicorn_arm.c rebuilds
                    // only for APSR/CPSR/CP_REG writes). So whether a
                    // handler's terminal `pop {...,pc}`/`bx lr` to an
                    // EXC_RETURN value raises EXCP_EXCEPTION_EXIT (8) or is
                    // treated as a plain branch depends on a stale flag; the
                    // plain-branch case fetches from the unmapped magic
                    // address and lands here as a prefetch abort instead
                    // (bug-146, hit under sustained EXTI trigger edges). The
                    // pop has fully retired either way, so the CPU state is
                    // identical to the intno=8 case -- handle it identically.
                    let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0);
                    if is_exception_return_pc(pc as u32) {
                        let sys = System { uc: RefCell::new(uc), p: p.clone(), d: d.clone() };
                        p.nvic.borrow_mut().return_from_interrupt(&sys);
                        p.nvic.borrow_mut().run_pending_interrupts(&sys, vector_table_addr);
                    } else {
                        let xpsr = uc.reg_read(RegisterARM::XPSR).unwrap_or(0);
                        let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                        let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0);
                        let primask = uc.reg_read(RegisterARM::PRIMASK).unwrap_or(0);
                        let basepri = uc.reg_read(RegisterARM::BASEPRI).unwrap_or(0);
                        error!(
                            "intr_hook intno={:08x} pc={:08x} xpsr={:08x} lr={:08x} sp={:08x} primask={:08x} basepri={:08x}",
                            exception, pc, xpsr, lr, sp, primask, basepri
                        );
                    }
                }
                _ => {
                    error!("intr_hook intno={:08x}", exception);
                    std::process::exit(1);
                }
            }
        }).expect("add_intr_hook failed");
    }

    uc.add_mem_hook(HookType::MEM_UNMAPPED, 0, u64::MAX, |uc, type_, addr, size, value| {
        if type_ == MemType::WRITE_UNMAPPED {
            warn!("{:?} addr=0x{:08x} size={} value=0x{:08x}", type_, addr, size, value);
        } else {
            warn!("{:?} addr=0x{:08x} size={}", type_, addr, size);
        }

        // Skip the faulting instruction. Its width is decoded on demand from
        // the two bytes at PC (a 16-bit halfword >= 0xE800 is the first half
        // of a 32-bit Thumb-2 encoding, prefixes 0b11101/0b11110/0b11111) --
        // there is no per-instruction hook tracking sizes anymore, and this
        // path only runs on the rare unmapped access.
        let pc = uc.reg_read(RegisterARM::PC).expect("failed to get pc");
        let mut insn = [0u8; 2];
        let width = match uc.mem_read(pc, &mut insn) {
            Ok(()) if u16::from_le_bytes(insn) >= 0xE800 => 4,
            _ => 2,
        };
        uc.reg_write(RegisterARM::PC, thumb(pc + width)).unwrap();

        CONTINUE_EXECUTION.store(true, Ordering::Release);

        false
    }).expect("add_mem_hook failed");

    let vector_table = VectorTable::from_memory(&uc, vector_table_addr)?;
    let mut pc = vector_table.reset as u64;
    uc.reg_write(RegisterARM::SP, vector_table.sp.into()).map_err(UniErr)?;
    //uc.reg_write(RegisterARM::LR, 0xFFFF_FFFF).map_err(UniErr)?;

    info!("Starting emulation");

    loop {
        let max_instructions = args.max_instructions.map(|c|
            // yes, we want to panic if this goes negative.
            c - NUM_INSTRUCTIONS.load(Ordering::Relaxed)
        );
        if max_instructions == Some(0) {
            info!("Reached target number of instructions. Done");
            break;
        }

        let result = uc.emu_start(
            pc,
            args.stop_addr.unwrap_or(0) as u64,
            0,
            max_instructions.unwrap_or(0) as usize,
        ).map_err(UniErr);
        let returned_pc = uc.reg_read(RegisterARM::PC).expect("failed to get pc");
        pc = thumb(returned_pc);

        if STOP_REQUESTED.load(Ordering::Relaxed) {
            info!("Stop requested");
            break;
        }

        if let Err(e) = result {
            if CONTINUE_EXECUTION.swap(false, Ordering::AcqRel) {
                // This was a bad memory access, we keep going.
                if crate::verbose() >= 3 {
                    trace!("Resuming execution pc={:08x}", pc);
                }
                pc = thumb(pc);
                continue;
            } else {
                bail!("{e} at pc=0x{returned_pc:08x}");
            }
        }

        if args.stop_addr == Some(returned_pc as u32) {
            let r3 = uc.reg_read(RegisterARM::R3).expect("failed to read R3");
            info!("{}", stop_address_state(returned_pc as u32, r3 as u32));
            break;
        }

        if BUSY_LOOP_REACHED.load(Ordering::Relaxed) {
            break;
        }
    }

    if let Some(n) = args.dump_stack {
        dump_stack(&mut uc, n);
    }

    for fb in framebuffers.images {
        fb.borrow().write_to_disk()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CpuModel;
    use unicorn_engine::unicorn_const::Prot;

    /// Documents the Unicorn contract Nvic::run_interrupt depends on: a code
    /// hook fires *before* its instruction executes, and a PC write plus
    /// emu_stop() from inside it must prevent that instruction from retiring
    /// -- otherwise dispatching an exception from the code hook would corrupt
    /// state whenever the interrupted instruction mutates SP/PC (e.g. a
    /// `pop {..., pc}` would consume the just-pushed exception frame).
    /// Verified false for both a block-leading and a mid-block instruction
    /// (bug-146 investigation); this pins the mid-block case.
    #[test]
    fn pc_write_plus_emu_stop_from_a_code_hook_prevents_the_hooked_instruction_retiring() {
        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap(); // thread code
        uc.mem_map(0x2000, 0x1000, Prot::ALL).unwrap(); // "handler"
        uc.mem_map(0x3000, 0x1000, Prot::ALL).unwrap(); // stack

        // nop; pop {r0, pc} -- the pop is mid-translation-block, like the
        // crash site (perfEventImpl's epilogue pop at 0x260568).
        uc.mem_write(0x1000, &[0x00, 0xbf, 0x01, 0xbd]).unwrap();
        uc.mem_write(0x2000, &[0x00, 0xbf]).unwrap(); // nop
        uc.mem_write(0x3000, &0x1111_1111u32.to_le_bytes()).unwrap(); // -> r0
        uc.mem_write(0x3004, &0x0000_1001u32.to_le_bytes()).unwrap(); // -> pc

        uc.reg_write(RegisterARM::SP, 0x3000).unwrap();
        uc.reg_write(RegisterARM::R0, 0xaaaa_aaaa).unwrap();

        uc.add_code_hook(0x1002, 0x1002, |uc, _pc, _size| {
            uc.reg_write(RegisterARM::PC, 0x2000).unwrap();
            uc.emu_stop().unwrap();
        })
        .unwrap();

        uc.emu_start(0x1001, 0, 0, 0).unwrap();

        assert_eq!(uc.reg_read(RegisterARM::SP).unwrap(), 0x3000, "pop retired: SP moved");
        assert_eq!(uc.reg_read(RegisterARM::R0).unwrap(), 0xaaaa_aaaa, "pop retired: r0 loaded");
        assert_eq!(uc.reg_read(RegisterARM::PC).unwrap(), 0x2000, "PC write was lost");
    }

    /// Same contract as above, but for the *block* hook that interrupt
    /// dispatch now runs from: a PC write plus emu_stop() from the block
    /// hook must prevent the block's instructions from retiring. The block
    /// hook call is injected at the head of the translated block, before
    /// its first instruction.
    #[test]
    fn pc_write_plus_emu_stop_from_a_block_hook_prevents_the_block_executing() {
        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap();
        uc.mem_map(0x2000, 0x1000, Prot::ALL).unwrap();
        uc.mem_map(0x3000, 0x1000, Prot::ALL).unwrap();

        // nop; pop {r0, pc} -- if the block ran, SP/r0/PC all change.
        uc.mem_write(0x1000, &[0x00, 0xbf, 0x01, 0xbd]).unwrap();
        uc.mem_write(0x2000, &[0x00, 0xbf]).unwrap(); // nop
        uc.mem_write(0x3000, &0x1111_1111u32.to_le_bytes()).unwrap();
        uc.mem_write(0x3004, &0x0000_1001u32.to_le_bytes()).unwrap();

        uc.reg_write(RegisterARM::SP, 0x3000).unwrap();
        uc.reg_write(RegisterARM::R0, 0xaaaa_aaaa).unwrap();

        uc.add_block_hook(0x1000, 0x1000, |uc, _addr, _size| {
            uc.reg_write(RegisterARM::PC, 0x2000).unwrap();
            uc.emu_stop().unwrap();
        })
        .unwrap();

        uc.emu_start(0x1001, 0, 0, 0).unwrap();

        assert_eq!(uc.reg_read(RegisterARM::SP).unwrap(), 0x3000, "block retired: SP moved");
        assert_eq!(uc.reg_read(RegisterARM::R0).unwrap(), 0xaaaa_aaaa, "block retired: r0 loaded");
        assert_eq!(uc.reg_read(RegisterARM::PC).unwrap(), 0x2000, "PC write was lost");
    }

    /// Pins the bug-146 mechanism: a `pop {pc}` of an EXC_RETURN magic value
    /// executed from a *thread-mode* translation block (Unicorn's HANDLER
    /// tb-flag unset) is a plain branch, so the fetch at the magic address
    /// raises a prefetch abort (intno=3) with PC parked in the magic range --
    /// NOT the EXCP_EXCEPTION_EXIT (8) that a handler-mode block would raise.
    /// run_emulator's intr_hook relies on this to route such aborts into the
    /// normal exception-return path.
    #[test]
    fn exc_return_pop_from_a_thread_mode_block_aborts_with_pc_in_the_magic_range() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap();
        uc.mem_map(0x3000, 0x1000, Prot::ALL).unwrap();

        uc.mem_write(0x1000, &[0x00, 0xbd]).unwrap(); // pop {pc}
        uc.mem_write(0x3000, &0xffff_ffed_u32.to_le_bytes()).unwrap();
        uc.reg_write(RegisterARM::SP, 0x3000).unwrap();

        let seen = Arc::new(AtomicU64::new(0));
        let seen_in_hook = seen.clone();
        uc.add_intr_hook(move |uc, intno| {
            let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0);
            seen_in_hook.store(((intno as u64) << 32) | (pc & 0xffff_ffff), Ordering::Relaxed);
            uc.emu_stop().unwrap();
        })
        .unwrap();

        uc.emu_start(0x1001, 0, 0, 0).ok();

        let intno = (seen.load(Ordering::Relaxed) >> 32) as u32;
        let pc = seen.load(Ordering::Relaxed) as u32;
        assert_eq!(intno, 3, "expected a prefetch abort from the thread-mode block");
        assert!(
            is_exception_return_pc(pc),
            "abort PC {pc:#x} must be in the EXC_RETURN magic range"
        );
    }

    #[test]
    fn cortex_m7_executes_proteus_vdiv() {
        let vdiv_f32_s14_s0_s1 = [0x80, 0xee, 0x20, 0x7a];
        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap();
        uc.mem_write(0x1000, &vdiv_f32_s14_s0_s1).unwrap();
        uc.reg_write(RegisterARM::S0, 9.0_f32.to_bits() as u64)
            .unwrap();
        uc.reg_write(RegisterARM::S1, 2.0_f32.to_bits() as u64)
            .unwrap();

        uc.emu_start(0x1001, 0x1004, 0, 1).unwrap();

        assert_eq!(
            uc.reg_read(RegisterARM::S14).unwrap() as u32,
            4.5_f32.to_bits(),
        );
    }

    #[test]
    fn cortex_m7_executes_proteus_vfp_continuation() {
        let proteus_vfp_continuation = [0xb7, 0xee, 0xc7, 0x7a];
        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap();
        uc.mem_write(0x1000, &proteus_vfp_continuation).unwrap();

        uc.emu_start(0x1001, 0x1004, 0, 1).unwrap();

        assert_eq!(uc.reg_read(RegisterARM::PC).unwrap(), 0x1004);
    }

    #[test]
    fn cortex_m7_executes_thumb_wfi_without_invalid_instruction() {
        let wfi_idle_loop = [0x30, 0xbf, 0xfd, 0xe7];
        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap();
        uc.mem_write(0x1000, &wfi_idle_loop).unwrap();

        uc.emu_start(0x1001, 0, 0, 2).unwrap();
        let next_pc = uc.reg_read(RegisterARM::PC).unwrap();
        assert_eq!(next_pc, 0x1002);
        assert!(uc.emu_start(thumb(next_pc), 0, 0, 1).is_ok());
        assert_eq!(uc.reg_read(RegisterARM::PC).unwrap(), 0x1000);
    }

    #[test]
    fn stop_address_state_reports_runtime_r3() {
        assert_eq!(
            stop_address_state(0x0020_a134, 0x4002_3800),
            "Stop address reached at pc=0x0020a134 r3=0x40023800"
        );
    }
}
