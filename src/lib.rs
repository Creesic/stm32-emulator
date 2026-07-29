// SPDX-License-Identifier: GPL-3.0-or-later

#[macro_use]
extern crate log;

pub mod launcher;

mod config;
mod emulator;
mod ext_devices;
mod framebuffers;
mod peripherals;
mod system;
mod util;

mod embedded;

pub use embedded::{run_embedded, EmbeddedEmulatorOptions};
pub use ext_devices::embedded_cdc::{active_embedded_link, EmbeddedCdcLink, EmbeddedCdcStage};
pub use ext_devices::embedded_ecu_io::{
    active_embedded_link as active_embedded_ecu_io_link, EmbeddedEcuIoLink, EmbeddedOutputEvent,
};

use std::sync::atomic::{AtomicU8, Ordering};

/// Internal command-line-shaped options consumed by the existing emulator
/// loop. The public embedded API deliberately exposes a smaller, host-friendly
/// options type instead.
pub(crate) struct Args {
    pub(crate) max_instructions: Option<u64>,
    pub(crate) stop_addr: Option<u32>,
    pub(crate) busy_loop_stop: bool,
    pub(crate) interrupt_period: u32,
    pub(crate) dump_stack: Option<usize>,
}

static VERBOSE: AtomicU8 = AtomicU8::new(0);

pub fn verbose() -> u8 {
    VERBOSE.load(Ordering::Relaxed)
}

fn set_verbose(verbose: u8) {
    VERBOSE.store(verbose, Ordering::Relaxed);
}

/// Opt-in CDC protocol tracing. Kept identical to the standalone binary's
/// behavior so embedding the emulator does not silently change diagnostics.
pub fn cdc_trace() -> bool {
    std::env::var("STM32_CDC_TRACE").is_ok_and(|value| value != "0" && !value.is_empty())
}

/// Requests shutdown of the currently running embedded emulator.
///
/// The present core supports one emulator instance per process because its
/// firmware clock and stop flags are process-wide. The higher-level EpicTuner
/// runtime enforces that single-instance contract.
pub fn request_stop() {
    emulator::request_stop();
}

pub fn instruction_count() -> u64 {
    emulator::instruction_count()
}
