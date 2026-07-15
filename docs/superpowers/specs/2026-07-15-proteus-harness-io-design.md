# Proteus F7 Full Harness I/O Routing Design

## Goal

Extend the existing `ecu_io` external-device bridge from its first-milestone
signal set (5 analog channels, 2 digital inputs, 2 observed outputs) to the
complete I/O surface a real Proteus F7 ECU exposes on its harness connectors,
so an external process can feed and observe every signal the firmware can be
tuned to use — without caring which functional role the current tune assigns
to a given pin.

This is a routing/coverage milestone: no waveform generation, no new
emulator-core capability. Signal generation stays with the external process,
exactly as the 2026-07-11 ecu_io design established.

## Naming

Signals are named after physical harness positions, not tune roles
(harness naming is stable across tunes; the tune decides which harness pin
is "MAP" or "injector 3", exactly like wiring a real Proteus):

| Group | Names | Pins (in order) |
|---|---|---|
| Lowside outputs | `ls1`–`ls16` | PD7, PG9, PG10, PG11, PG12, PG13, PG14, PB4, PB5, PB6, PB7, PB8, PB9, PE0, PE1, PE2 |
| Highside outputs | `hs1`–`hs4` | PA9, PA8, PD15, PD14 |
| Ignition outputs | `ign1`–`ign12` | PD4, PD3, PC9, PC8, PC7, PG8, PG7, PG6, PG5, PG4, PG3, PG2 |
| Analog Temp inputs | `at1`–`at4` | PC4, PC5, PB0, PB1 |
| Analog Volt inputs | `av1`–`av11` | PC0, PC1, PC2, PC3, PA0, PA1, PA2, PA3, PA4, PA5, PA6 |
| Battery sense | `vbatt` | PA7 |
| VR trigger inputs | `vr1`, `vr2` | PE7, PE8 |
| Digital inputs | `din1`–`din6` | PC6, PE11, PE12, PE14, PE13, PE15 |

Totals: 32 observed outputs, 8 digital inputs, 16 ADC channels — exactly
filling the 16 slow-ADC channels the `Adc` peripheral models (`EFI_ADC_0`
PA0 … `EFI_ADC_15` PC5).

Canonical source: `proteus_meta.h` in the epicefi_fw checkout — the board
definition the running firmware is actually built from. Note `din5`/`din6`
are PE13/PE15 in that order (the meta header's DIGITAL_5/DIGITAL_6), which
is deliberately not ascending pin order.

The old functional names are replaced, not aliased. Migration map for
existing users of the protocol: `map`→`av1`, `tps`→`av2`, `clt`→`at3`,
`iat`→`at2`, `vbatt`→`vbatt`, `crank`→`din1`, `cam`→`din2`, `inj1`→`ls1`,
`ign1`→`ign1`.

## Architecture

No emulator-core code changes. `Adc` already resolves all 16 ADC1 channels
by pin; `EcuIo` pins (input EXTI wiring, output write-callbacks) are fully
config-driven. The 8 digital inputs occupy distinct EXTI lines (PC6→6,
PE7→7, PE8→8, PE11–PE15→11–15), so no line collisions.

The full map is expressed in the two existing per-board surfaces:

1. **`proteus_f7/config.yaml`** — the `ecu_io` device block grows to the
   full 56-signal map.
2. **Launcher board module (new file): `src/launcher/boards/proteus_f7.rs`**
   — the Proteus-specific data currently inlined in `src/launcher/mod.rs`
   (`PROTEUS_F7_REGIONS`, `PROTEUS_F7_PATCHES`, `PROTEUS_F7_PROFILE`, and
   the ecu_io pin/ADC tables) moves into this module, and the tables grow
   to the same 56-signal map. `src/launcher/boards/mod.rs` re-exports each
   board's `ProfileTemplate`. This is the modularity requirement: boards
   beyond the Proteus F7 are planned, and adding one must mean adding one
   file plus a catalog entry, not editing shared code. The `ProfileTemplate`
   type itself stays in `launcher/mod.rs`; only per-board data moves.

The duplication between config.yaml and the launcher tables is accepted —
it is the established pattern (the launcher is deliberately self-contained
and never reads example configs at runtime). Alternatives considered and
rejected: making the launcher read `config.yaml` (against its design);
generating both tables from epicefi_fw's `board.yaml` at build time
(machinery disproportionate to one static, stable board map — revisit only
if per-board tables start churning).

## Protocol and Behavior

Unchanged from the 2026-07-11 design: line-oriented `name=value` over TCP
(`127.0.0.1:29002`), millivolts for ADC channels, `0`/`1` for digital pins,
output changes pushed as `name=value` lines. Unknown names are logged and
ignored, so a feeder written for the new map fails soft against an old
emulator and vice versa.

## Verification

- Launcher tests (`tests/launcher_profile.rs` and friends) updated to pin
  the new generated-YAML signal tables, including the group counts
  (16/4/12/4/11/1/2/6) and spot-checked pins (e.g. `ls16`=PE2,
  `ign12`=PG2, `din6`=PE15).
- `EcuIo`/`Adc` logic is already unit-tested generically; the new map is
  data, not logic.
- End-to-end: run the emulator, feed `av1=1500` / `at3=2000` over :29002,
  confirm TunerStudio (attached via the USB CDC bridge) shows the
  corresponding MAP/CLT gauge movement under a stock Proteus tune; toggle
  `din1` and confirm the EXTI interrupt path fires (trigger error counter
  or trace log).

## Non-Goals

- Waveform/trigger-pattern generation (external process's job).
- Real-time pacing.
- TIM-PWM-driven outputs: only GPIO-driven output writes are observable
  today. Firmware functions routed through hardware PWM (idle valve, boost)
  will not produce `name=value` events until a TIM output model exists —
  a known, accepted gap in this milestone.
- Boards other than the Proteus F7. The `boards/` module split prepares for
  them; it does not add any.
