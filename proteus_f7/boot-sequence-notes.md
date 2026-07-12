# Proteus F7 boot-sequence trace evidence

Findings from investigating how far real rusEFI firmware (`rusefi.bin`,
also verified against a fresh matching-symbol `epicefi.bin` build) actually
boots in this emulator, prompted by the ECU I/O feature's live verification
(`docs/proteus-f7-ecu-io.md`) finding that `crank`/`cam` never raised an
EXTI interrupt and `map`/`tps`/etc. never reflected in ADC1.

## A sixth bug, found by resolving the exact repeating PC: TIM5 never modeled

**Symptom**: firmware never touches `ADC1`/`SYSCFG`, and `EXTI.IMR` is only
ever written once, for unrelated RTC lines (bits 17/21/22, value
`0x00620000`) — never for the crank pin's line (bit 6). A live capture
showed `TIM5.CNT32` (`addr=0x40000c24`) being read 67,000+ times, always
returning `0x00000000`.

**Root cause**: rusEFI uses TIM5 (via ChibiOS's PWM driver in output-compare
mode — see `firmware/hw_layer/ports/stm32/microsecond_timer_stm32.cpp:1-14`
in the `epicefi_fw` checkout) as its sole hardware timebase.
`getTimeNowLowerNt()` (`microsecond_timer_stm32.cpp:78`, inlined into
`getTimeNowNt()`) reads `TIM5->CNT` directly — resolved from the exact
repeating PC (`0x2398de`) via `addr2line` against a matching-symbol
`epicefi.elf`. This emulator never modeled TIM5 at all, so `CNT` always
read 0 regardless of firmware's configuration.

`EventQueue::executeOne()` (`firmware/controllers/system/event_queue.cpp:253`)
has an explicit, intentional busy-wait for near-term scheduled events:
`while (current->getMomentNt() > getTimeNowNt())`, commented "yes, that's a
busy wait but that's what we need here". Since `getTimeNowNt()` could never
advance past its frozen value, this spun forever the moment anything
scheduled a near-future event — which happens very early in boot, inside
`initSingleTimerExecutorHardware()` (`hardware.cpp:498`), called before
`efiExtiInit()` (`hardware.cpp:515`) and long before ADC/trigger init.

**Fix**: `src/peripherals/tim5.rs` — models `TIM5.CNT` as free-running
once `CR1.CEN` is set, mirroring the existing `Dwt` peripheral's CYCCNT
model exactly (`instruction count + offset` while enabled, frozen at its
last value while disabled). 4 unit tests. Committed as `4fe9110`.

**Verified live**: with the fix, `TIM5.CNT` genuinely advances (confirmed
via two different non-zero reads at different clk values), and the access
pattern changed from a continuous tight spin to a periodic once-per-tick
read. Firmware went on to touch GPIO (all ports), `CAN1`'s filter-setup
registers, `OTG_FS` (USB), `IWDG` (watchdog, kicked repeatedly), `FLASH`,
`RTC`, `PWR`, and `DMA1`/`DMA2` — none of which it ever reached before this
fix. This is a real, substantial, confirmed improvement.

## Still open: ADC1/SYSCFG/crank-EXTI never reached

Even with the TIM5 fix, `ADC1` and `SYSCFG` are never touched, and
`EXTI.IMR` is never written for the crank line, across every capture run so
far (up to ~350ms of firmware-modeled time / ~72 million instructions).

### Candidates ruled out (five research rounds against the `epicefi_fw` source)

1. **`loadConfiguration()`** (`epicefi.cpp:234` →
   `firmware/controllers/flash_main.cpp:402-430`): every failure path
   (`CrcFailed`, `NotFound` — blank/`0xFF` flash, `Failed`, `NotSupported`)
   falls through to `resetConfigurationExt(DEFAULT_ENGINE_TYPE)` and
   *always* returns — there is no blocking/waiting on a valid persisted
   config anywhere in this function. Confirms this emulator's blank config
   sector is not the blocker.
2. **Thread-spawning calls** — `startTunerStudioConnectivity()`
   (`tunerstudio.cpp:1121-1156`, just `memset` + console-action
   registration), `startSerialChannels()` (`tunerstudio_io_serial_ports.cpp:93-108`),
   `startStatusThreads()` (`status_loop.cpp:994-1000`), `initTriggerCentral()`
   (`trigger_central.cpp:1503-1515`) — all resolve to
   `ThreadController::start()` (`thread_controller.h:50-57`), a plain
   `chThdCreateStatic`-style spawn-and-return. None block the calling
   thread.
3. **`i2cStart()`/`boardInitHardware()`** (`hardware.cpp:589-602`, the two
   steps between `initHardware()`'s entry and `initAdcInputs()` at line
   605): both are compiled out entirely for Proteus.
   `i2cStart(&EE_U2CD, ...)` is gated behind `STM32_I2C_USE_I2C3`, which
   defaults `FALSE` and is never overridden by Proteus's board files.
   `boardInitHardware()`'s weak default is a literal empty function, and
   Proteus's `setup_custom_board_overrides()`
   (`config/boards/proteus/board_configuration.cpp:353-356`) never assigns
   `custom_board_InitHardware`. This window is dead code for this board.
4. **`engineModules.apply_all(initNoConfiguration)`** (`epicefi.cpp:216-218`):
   a compile-time-unrolled call of `initNoConfiguration()` (a
   `virtual void {}` no-op base, `engine_module.h:12`) across all ~30
   registered modules. Only one override exists anywhere in the tree
   (`EthernetConsoleModule`, gated behind `EFI_ETHERNET`, which defaults
   `FALSE` and isn't set for Proteus) — so every module runs the empty
   no-op. Confirmed via full grep of `initNoConfiguration` across the tree.
5. **Full line-by-line read of `runEpicEfi()`** (`epicefi.cpp:174-260`):
   every call between the confirmed-safe steps above was enumerated; none
   looked blocking.

### Current best evidence: main thread's actual progress is unconfirmed

A ~72-million-instruction `-vvvv` capture (bounded via `--max-instructions`,
not an open-ended wait) shows the system genuinely idling — a tight 2
instruction loop at `0x27c13a`/`0x27c13c` (`wfi` / `b #0x27c13a`, the
confirmed ChibiOS idle thread), not an infinite busy-spin. This is why
`-b`/`--busy-loop-stop` never triggers: the loop alternates between two
addresses, never repeating the exact same PC twice in a row.

Tracing the last activity before settling into that loop resolved (via
`addr2line`) to:

- `runAndScheduleNext(ch_virtual_timer*, PeriodicTimerController*)`
  (`ChibiOS/os/rt/include/chvt.h:250`) and `ch_dlist_insert`
  (`chlists.h:544`) — ChibiOS's own virtual-timer kernel machinery,
  re-arming the IWDG-kick timer (`wdg_lld_reset`,
  `ChibiOS/os/hal/ports/STM32/LLD/xWDGv1/hal_wdg_lld.c:130`) for its next
  period.
- The GPIOE writes at `pc=0x2739c8` (`OutputPin::setOnchipValue()`,
  `controllers/system/efi_gpio.cpp:612`) toggling bits 4 and 6 — the
  comms/warning status LEDs (`docs/proteus-f7-usb.md`'s pin table:
  comms=E4, warning=E6) — is `communicationsBlinkingTask`'s periodic
  blink, spawned non-blockingly by `startStatusThreads()` (candidate #2,
  above).

**Both of these are independent background activity** — a kernel virtual
timer and a separately-spawned status thread — that ChibiOS's preemptive
scheduler runs regardless of whether the *main* `runEpicEfi()` thread
(whose call chain is what actually reaches `initHardware()`/
`initAdcInputs()`) has stalled. Neither observation actually confirms the
main thread is making progress. This is the key open question: is the main
thread genuinely blocked somewhere unidentified, or has it already run to
completion (e.g. into `initMainLoop()`/`runMainLoop()`) and is idling
correctly with nothing further scheduled?

**Next technique, not yet tried**: resolving this needs identifying which
thread is actually executing at a given point — e.g. correlating the stack
pointer (`sp`) against each thread's known stack region (ChibiOS static
thread stacks are fixed, compile-time-sized regions; the main thread's own
stack vs. `communicationsBlinkingTask`'s vs. the idle thread's are all
distinct memory ranges resolvable from the ELF/linker map). This is a
materially different technique than the source-reading and PC/register
tracing used for all five rounds above, and hasn't been attempted yet.

## Process notes

- Live captures used `--max-instructions` bounds (never open-ended waits)
  and `Monitor`/bounded polling loops, per this project's established
  "should be instant, not a long blind wait" methodology.
- `-vvvv` (full disassembly) throughput on this host: roughly 300-400K
  instructions/sec once warmed up (75M instructions ≈ 3.5GB log, ~4 minutes
  wall-clock). `-b`/`--busy-loop-stop` does not help for multi-instruction
  idle loops (only catches a literal single repeated PC).
- All capture logs were deleted after analysis; none were committed
  (matches `proteus_f7/*.log` already being gitignored).
