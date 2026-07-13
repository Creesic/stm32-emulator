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

// PC + instruction size
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

    let diassembler = Capstone::new()
        .arm()
        .mode(arch::arm::ArchMode::Thumb)
        .build()
        .expect("failed to initialize capstone");

    // We hook on each instructions, but we could skip this.
    // The slowdown is less than 50%. It's okay for now.
    {
        let trace_instructions = crate::verbose() >= 4;
        let busy_loop_stop = args.busy_loop_stop;
        let p = sys.p.clone();
        let d = sys.d.clone();
        let interrupt_period = args.interrupt_period;
        sys.uc.borrow_mut().add_code_hook(0, u64::MAX, move |uc, pc, size| {
            unsafe {
                if busy_loop_stop && LAST_INSTRUCTION.0 == pc as u32 {
                    info!("Busy loop reached");
                    uc.emu_stop().unwrap();
                    BUSY_LOOP_REACHED.store(true, Ordering::Release);
                }
                LAST_INSTRUCTION = (pc as u32, size as u8);
            }

            let n = NUM_INSTRUCTIONS.fetch_add(1, Ordering::Acquire);

            if trace_instructions {
                info!("{}", disassemble_instruction(&diassembler, uc, pc));
            }

            if n % interrupt_period as u64 == 0 {
                let sys = System { uc: RefCell::new(uc), p: p.clone(), d: d.clone() };
                p.nvic.borrow_mut().note_fetched_instruction(&sys, pc);
                p.nvic.borrow_mut().run_pending_interrupts(&sys, vector_table_addr);
            }

            if n & PUMP_EVENT_INST_INTERVAL == 0 {
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
                    let xpsr = uc.reg_read(RegisterARM::XPSR).unwrap_or(0);
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0);
                    let sp = uc.reg_read(RegisterARM::SP).unwrap_or(0);
                    let primask = uc.reg_read(RegisterARM::PRIMASK).unwrap_or(0);
                    let basepri = uc.reg_read(RegisterARM::BASEPRI).unwrap_or(0);
                    error!(
                        "intr_hook intno={:08x} xpsr={:08x} lr={:08x} sp={:08x} primask={:08x} basepri={:08x}",
                        exception, xpsr, lr, sp, primask, basepri
                    );
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

        unsafe {
            let pc = uc.reg_read(RegisterARM::PC).expect("failed to get pc");
            assert!(pc as u32 == LAST_INSTRUCTION.0);
            uc.reg_write(RegisterARM::PC, thumb(pc as u64 + LAST_INSTRUCTION.1 as u64)).unwrap();
        }

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
