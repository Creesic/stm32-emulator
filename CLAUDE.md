# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## OpenWolf

@.wolf/OPENWOLF.md

This project uses OpenWolf for context management. Read and follow .wolf/OPENWOLF.md every session. Check .wolf/cerebrum.md before generating code. Check .wolf/anatomy.md before reading files.

## Project overview

A Rust emulator for STM32 microcontrollers, originally built to run unmodified vendor firmware for 3D printers (e.g. Elegoo Saturn, Anycubic Mono X) so their behavior can be studied without physical hardware, and now also used to bring up other Cortex-M firmware (e.g. the rusEFI Proteus F767 ECU, see `proteus_f7/`). ARM Cortex-M instructions run on Unicorn (a QEMU fork); this repo supplies the peripherals, external devices, and interrupt controller that Unicorn doesn't model itself. Register addresses come from vendor SVD files rather than being hardcoded, so new STM32 variants mostly just need a config file.

A second front end, the native launcher (`src/launcher/`, `src/bin/stm32-launcher.rs`), wraps the CLI in a Dear ImGui desktop GUI — see "Native launcher" below.

## Build & run

```
cargo build --release
```

On Windows, `sdl2` (bundled) and `glium` trigger a CMake-based native build; with newer CMake you need, before building:
```
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:CMAKE_GENERATOR = 'Ninja'
```

The emulator is invoked with a YAML config file whose paths (firmware binaries, SVD file) are relative to the current directory, so `cargo run` must be run from inside an example directory, not the repo root:

```
cd saturn     && cargo run --release -- config.yaml -v
cd monox      && cargo run --release -- config.yaml -v
cd proteus_f7 && cargo run --release --bin stm32-emulator -- config.yaml -v
```

`proteus_f7`'s firmware binary and SVD are gitignored local assets (fetched by `proteus_f7/setup.ps1`); `verify_boot.ps1`/`verify_fpu.ps1` are bounded PowerShell smoke tests for that bring-up.

Useful CLI flags (see `src/main.rs`):
- `-v` (repeatable up to `-vvvv`): `-v` info-level peripheral traces, higher levels add register-level and per-instruction disassembly tracing.
- `-m/--max-instructions <N>`: stop after N instructions.
- `-s/--stop-addr <addr>`: stop when PC reaches an address (hex allowed, e.g. `0x0801234`).
- `-b/--busy-loop-stop`: stop once the PC repeats the same address twice in a row (an infinite loop).
- `-i/--interrupt-period <N>`: check for pending interrupts every N instructions (default 1; higher is faster but less accurate).
- `-d/--dump-stack <N>`: print N stack words at exit.

`asm.py` hand-assembles Thumb instructions (needs `pip3 install keystone-engine`) for use in a config's `patches:` section, e.g. `./asm.py <<< 'nop'`.

### Tests

```
cargo test
```

There's no separate lint config. Tests are split across two crate targets:
- Unit tests colocated in `#[cfg(test)] mod tests` blocks inside the modules they cover (e.g. `emulator.rs`, `config.rs`, `peripherals/mod.rs`, `peripherals/pwr.rs`) — these compile as part of the `stm32-emulator` binary target (declared via `mod` in `main.rs`).
- Integration tests in `tests/launcher_*.rs` — these link against `src/lib.rs`, which only exposes `pub mod launcher`, so they exercise the launcher's profile/registry/process/workspace logic, not the core emulator.

## Architecture

**Entry flow**: `main.rs` parses CLI args, deserializes the YAML config (`config.rs`) and the SVD file (`svd_parser`), then calls `emulator::run_emulator`.

**Emulator core** (`emulator.rs`): creates a Unicorn ARM instance in `Mode::THUMB` and then explicitly selects the Cortex-M model (`ctl_set_cpu_model`, from `config.cpu.model`: `cortex-m4`/`cortex-m7`) — Unicorn 2.1.5 treats the older `Mode::MCLASS` flag as a forced Cortex-M33 selection, so the model must be set this way instead. It then installs three hooks and runs `uc.emu_start` in a loop (re-entering after handled faults):
- A **code hook** fires on every instruction: advances the NVIC (`run_pending_interrupts` every `interrupt_period` instructions), optionally disassembles/logs the instruction (`-vvvv`), and periodically pumps SDL events / redraws framebuffer windows.
- An **interrupt hook** handles Unicorn exception numbers; `8` ("return from exception") is the main case, delegating to `Nvic::return_from_interrupt`.
- A **memory hook** on unmapped access logs a warning and skips the faulting instruction instead of crashing — vendor firmware often touches addresses this emulator doesn't model, and treating that as fatal would stop emulation too early.

Because Unicorn has no Cortex-M NVIC model, `peripherals/nvic.rs` hand-implements ARM's exception entry/return: pushing/popping the context register set (with MSP/PSP and FPU-lazy-stacking awareness), encoding `LR` as the correct `EXC_RETURN` value, and reading the vector table (base set by `cpu.vector_table` in config).

**System** (`system.rs`): `System { uc, p: Rc<Peripherals>, d: Rc<ExtDevices> }` is the bag of state threaded through every read/write callback (cloned via `Rc`, since Unicorn's hook closures can't hold borrows). `system::prepare()`:
1. Maps `config.regions` into Unicorn memory and loads firmware/RAM contents, then applies `config.patches` (raw byte patches, e.g. to NOP out delay loops that only waste emulated time).
2. Builds `ExtDevices` from `config.devices`.
3. Builds `Peripherals` by walking the SVD device tree (`Peripherals::from_svd`).
4. Registers one `mmio_map` callback per range in `Peripherals::MEMORY_MAPS` that dispatches every read/write by address to the right peripheral.

**Peripherals** (`peripherals/mod.rs` + `peripherals/*.rs`): for each SVD-declared peripheral, `register_peripheral` tries a fixed chain of `Xxx::new(name, ...)` constructors (`Nvic`, `SysTick`, `Scb`, `Gpio`, `Usart`, `Fsmc`, `Rcc`, `I2c`, `Dma`, `Spi`, `Flash`, `Rtc`, `Tim11`, `OtgFs`) that pattern-match on the SVD name (e.g. anything starting with `"SPI"` becomes an `Spi`). Unmatched peripherals are still kept as "debug peripherals" purely for register-name trace logging. All implemented peripherals share the `Peripheral` trait (`read`/`write` by register byte offset). Address handling in `Peripherals::read`/`write` covers ARM bit-banding (`0x4200_0000`-`0x43FF_FFFF` aliasing individual bits) and forces 4-byte-aligned register access. Some peripherals (`Pwr`, `Rcc`, `Flash`, `Rtc`, `Tim11`) don't model real hardware behavior at all — they're minimal state machines that report the "ready" status bits firmware polls for during clock/voltage bring-up, since that's all a boot sequence actually needs. Note some SVD files omit core peripherals entirely (e.g. the F767 SVD has no SysTick/SCB); those get registered at their architecturally-fixed addresses regardless of what's in the SVD.

**External devices** (`ext_devices/mod.rs` + `ext_devices/*.rs`): things wired to a peripheral rather than being one — `SpiFlash`, `UsartProbe`, `Display` (ILI9341-style TFT, driven over FSMC), `Lcd` (FPGA-driven panel), `Touchscreen` (ADS7846-style resistive touch), `UsbCdcTcp` (nonblocking single-client TCP bridge standing in for a real USB host — see below). Each implements `ExtDevice<Addr, Value>`; peripherals look theirs up by name via `ExtDevices::find_serial_device`/`find_mem_device`/`find_usb_cdc_tcp` and hold it as `Option<Rc<RefCell<dyn ExtDevice<...>>>>` (or `Rc<RefCell<UsbCdcTcp>>`), forwarding register-level I/O to it (see `Spi`'s `ext_device` field for the pattern).

**Virtual USB** (`peripherals/otg_fs.rs` + `ext_devices/usb_cdc_tcp.rs`): rather than emulating real USB packets, `OtgFs` models just enough of the OTG-FS global registers/interrupts (`GINTSTS`/`GINTMSK`/`GRSTCTL`) to let firmware believe a host is attached, and forwards CDC payload bytes to/from a plain TCP socket (`UsbCdcTcp`, config key `usb_cdc_tcp`) — so a real terminal/tool can attach over TCP instead of needing a real USB stack on either end.

**Framebuffers** (`framebuffers/mod.rs`): shared pixel sinks that `Display`/`Lcd` write into. Two backends: `image` (accumulate in memory, write PNG on exit) and `sdl` (bundled SDL2 live window, pumped from the code hook).

**GPIO & software SPI** (`peripherals/gpio.rs`, `peripherals/sw_spi.rs`): config refers to pins by name (e.g. `PA15`); `sw_spi` bit-bangs SPI over four named GPIO pins for firmware that talks to a device without using a hardware SPI peripheral (see `saturn/config.yaml`'s `SW_SPI_LCD`).

**Config schema** (`config.rs`): top-level YAML keys are `cpu` (SVD path, `vector_table` address, and `model`: `cortex-m4`/`cortex-m7`), `regions` (memory-mapped files, e.g. ROM/RAM), `patches` (byte patches by address), `peripherals` (currently just `software_spi`), `devices` (external devices, keyed by which SVD peripheral name they attach to — includes `usb_cdc_tcp`), and `framebuffers`.

**Example projects** (`saturn/`, `monox/`, `proteus_f7/`): each pairs a `config.yaml` with a firmware binary and an SVD file. `saturn`/`monox` target STM32F407 3D-printer firmware; `proteus_f7` targets the STM32F767-based rusEFI Proteus ECU and is a partial/in-progress bring-up (see `proteus_f7/README.md`). None are Cargo workspace members — running the emulator against them means `cd`-ing in first (see Build & run above).

## Native launcher

`src/launcher/` (a library exposed via `src/lib.rs`) plus the `stm32-launcher` and `generate-variant-registry` binaries in `src/bin/` implement a Dear ImGui desktop GUI front end (see `docs/native-launcher.md` for the user-facing walkthrough). It never embeds a second emulator core: it resolves a board profile, writes a temporary YAML config in the same schema `config.rs` deserializes, and runs the `stm32-emulator` CLI as a child process, streaming its stdout/stderr into the GUI console (`launcher/process.rs`).

- `launcher/mod.rs`: `KnownVariant`/`ResolvedProfile` model — resolves a catalog entry (or a user-provided "Manual profile") to a CPU model, vector table, and memory regions, and serializes that to the emulator's YAML config (`ResolvedProfile::to_yaml`).
- `launcher/generated_variants.rs`: a checked-in, compiled-in catalog of board variants (currently sourced from an `epicefi_fw` checkout). Regenerate it during development with `cargo run --release --bin generate-variant-registry -- <path-to-epicefi_fw>`; the launcher itself never reads that source tree at runtime. Only variants with a verified, evidence-backed memory map (currently just `proteus_f7`, marked `Partial`) are runnable — others are intentionally blocked rather than guessed at.
- `launcher/workspace.rs`: persists window/dock layout and last-used firmware/SVD/emulator paths and selected variant across launches (but never a running emulator process).
- `launcher/registry.rs`, `launcher/ui_state.rs`: catalog lookup/support-state descriptions and GUI-independent selection/run-state modeling, respectively.

## Design docs

`docs/superpowers/{specs,plans}/` holds the design specs and implementation plans behind recent feature work (native launcher, Proteus F7 bring-up, virtual USB, Cortex-M7/VFP support) — check there for the rationale behind a design if the code alone doesn't explain it.
