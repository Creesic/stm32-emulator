# Proteus F7 ECU I/O

A second TCP bridge, independent of the USB CDC one, lets an external
process drive the digital and analog signals rusEFI reads as engine
sensor inputs, and observe the outputs it drives in response. The
emulator does not generate any of these signals itself — it only
reflects whatever it's told.

## Launch

Same emulator process as `docs/proteus-f7-usb.md` — this is a second,
independent listener on the same run.

## Protocol

Connect with any raw TCP client, e.g. `ncat 127.0.0.1 29002`. One line
per message, `name=value`:

- Digital input pins (`vr1`, `vr2`, `din1`–`din6`): `value` is `0` or `1`.
- ADC channels (`at1`–`at4`, `av1`–`av11`, `vbatt`): `value` is
  millivolts, clamped to 0-3300 (VREF+).
- Output pins (`ls1`–`ls16`, `hs1`–`hs4`, `ign1`–`ign12`): the
  emulator sends `name=value` lines to the connected client whenever
  firmware drives that pin to a new level. There is nothing to send
  for these — they are observed, not driven.

Only one client at a time, same rule as the USB CDC bridge
(`docs/proteus-f7-usb.md`'s "One-client rule and disconnects").

## Current signal set

Signals are named by physical Proteus harness position, not tune role —
the tune decides which position carries MAP or injector 3, exactly like
wiring a real harness. Source: `proteus_meta.h` (epicefi_fw).

| Group | Names | Pins (in order) |
|---|---|---|
| Lowside outputs | `ls1`–`ls16` | PD7, PG9–PG14, PB4–PB9, PE0–PE2 |
| Highside outputs | `hs1`–`hs4` | PA9, PA8, PD15, PD14 |
| Ignition outputs | `ign1`–`ign12` | PD4, PD3, PC9, PC8, PC7, PG8–PG2 |
| Analog Temp inputs | `at1`–`at4` | PC4, PC5, PB0, PB1 |
| Analog Volt inputs | `av1`–`av11` | PC0–PC3, PA0–PA6 |
| Battery sense | `vbatt` | PA7 |
| VR trigger inputs | `vr1`, `vr2` | PE7, PE8 |
| Digital inputs | `din1`–`din6` | PC6, PE11, PE12, PE14, PE13, PE15 |

Note `din5`/`din6` are PE13/PE15 in that order (PROTEUS_DIGITAL_5/6).

Migration from the pre-2026-07-15 functional names: `map`→`av1`,
`tps`→`av2`, `clt`→`at3`, `iat`→`at2`, `crank`→`din1`, `cam`→`din2`,
`inj1`→`ls1`; `vbatt` and `ign1` are unchanged (as names — `ign1` now
means harness ignition position 1, still PD4).

Only GPIO-driven output writes are observable; firmware functions routed
through hardware timer PWM (idle, boost) will not produce events until a
TIM output model exists.

## Verification

`.\proteus_f7\verify_boot.ps1` passes, including the `ecu_io:`,
`listen: 127.0.0.1:29002` assertions and the harness-map sentinels
(`name: din1`, `name: ls16`, `name: ign12`, `name: av11`,
`name: vbatt`), and the existing one-instruction reset-vector smoke
test.

The TCP bridge itself is confirmed live: connecting a client to
`127.0.0.1:29002` and sending `map=1500\n` then four `crank=1\n`/
`crank=0\n` pairs produces exactly this in the emulator's log (`-vv`
or higher):

```
[clk=744226817 pc=0x002793de] INFO  ECU IO client connected from 127.0.0.1:59063
[clk=808321025 pc=0x002793de] INFO  ECU IO client disconnected
```

confirming the listener accepts a connection and processes the
`name=value` lines without error (this is the same connect/disconnect
pattern already relied on for the USB CDC bridge). On the same run,
the already-working TunerStudio path was reconfirmed healthy — a raw
`'Q'` byte sent to port 29000 got back the real 45-byte signature in
under 0.3s (`rusEFI Tera.2026.06.30.proteus_f7.1962987583`) — so the
general boot/RTOS/USB path was not regressed by this change.

**Historical note (pre-2026-07-15):** the capture below predates the DMA
transfer-complete delay fix; since that fix, firmware's slow-ADC loop runs
continuously (ADC1 is read constantly) and boot reaches `usbStart()`, so
the "ADC1 was never read" observation below no longer holds.

**What could not be confirmed live:** across two separate captures on
unmodified `rusefi.bin` — one at `-vv` reaching clk≈1.19 billion, one
at `-vvv` (register-level) reaching clk≈9.9 billion, together
representing tens of seconds of firmware-modeled time (SysTick fires
every 216,000 clk, i.e. 1ms) — `ADC1` was never read or written even
once beyond its startup peripheral-map registration, `SYSCFG` was
never touched at all, and `EXTI`'s `IMR` was written exactly once,
early in boot, to `0x00620000` (bits 17/21/22 — the RTC
alarm/tamper/wakeup lines ChibiOS's HAL wires unconditionally — not
bit 6, which is `crank`'s line). Sending `crank=1`/`crank=0` over the
bridge while this was captured produced no additional `EXTI`/`SYSCFG`
register activity and no `Running interrupt irq=23` (EXTI9_5) trace
line, so the interrupt path this milestone set out to confirm was not
observed firing.

This does not appear to be a config or wiring defect: `rusEFI`'s own
source (`firmware/config/boards/proteus/board_configuration.cpp:112`
in the `epicefi_fw` checkout) unconditionally assigns
`triggerInputPins[0] = PROTEUS_DIGITAL_1` (PC6) during board init, and
`startTriggerInputPins()` (`hw_layer/hardware.cpp:555`) is called
unconditionally from `startHardware()`, itself called near the end of
`initHardware()` — so on real hardware this path is not gated behind
TunerStudio configuration. Something in this emulator's current
bring-up appears to prevent firmware from reaching that point: one
concrete lead is that `TIM5`'s `CNT32` register is read very
frequently (67,000+ times in the `-vvv` capture) and always reads
`0x00000000` — its `CR1` was written once with the counter disabled
and never re-enabled — which is consistent with a hardware free-running
counter/timestamp source that never advances in this emulator, and
could stall any firmware code that waits on elapsed time measured
through it. This is offered as a starting point for follow-up
investigation, not a confirmed root cause — chasing it further was
out of scope for this task (wiring the config and reporting what live
verification actually showed, not debugging the emulator's broader
timer model).

The `EcuIo`/`Adc`/`Exti` Rust logic added by this project's earlier
tasks is independently covered by unit tests (`cargo test --bin
stm32-emulator`, 59 passing, including `ecu_io::`, `adc::`, and
`exti::` suites) that do not depend on real firmware reaching this
code path, so that logic is verified correct in isolation even though
this specific live, end-to-end firmware confirmation is not yet
possible.
