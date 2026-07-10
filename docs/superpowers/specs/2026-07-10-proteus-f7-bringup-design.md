# Proteus F7 Firmware Bring-up Design

## Goal

Run the rusEFI Proteus F7 snapshot deterministically in this emulator, first
as a bounded, traceable CPU boot and then through incremental, evidence-based
hardware modeling.

## Known Firmware Facts

- Snapshot directory: `C:\\Users\\Tera\\Desktop\\Epictuner\\rusefi.snapshot.proteus_f7`
- Image: `rusefi.bin` (751,596 bytes)
- Initial stack pointer: `0x20021000`
- Reset vector: `0x002003D5`
- The vector values require mapping the image at the STM32F7 code-interface
  alias beginning at `0x00200000`.

## Phase 1: Minimal Boot Harness

Create a self-contained `proteus_f7/` example with a YAML configuration,
the firmware image, and the selected STM32F7 SVD. The configuration will:

1. Map `rusefi.bin` at `0x00200000` and set `cpu.vector_table` to that
   address.
2. Map the required STM32F7 RAM regions, beginning with the region containing
   the documented initial stack pointer.
3. Use the SVD for the exact Proteus F7 MCU. The part number is a required
   pre-configuration discovery step; no neighboring F7 SVD will be substituted
   merely to make the emulator start.
4. Avoid external-device configuration and firmware patches in the initial
   run. In particular, CAN, sensor, output-driver, and board-specific responses
   will not be fabricated.

Run the example from its directory with a finite instruction bound and register
tracing. Record reset entry, early clock/peripheral initialization, and every
unmapped MMIO address.

## Phase 2: Trace-Driven Hardware Modeling

Convert the phase-1 trace into a ranked device backlog. For each next boot
boundary:

1. Identify the first blocking peripheral interaction and the corresponding
   board connection from the Proteus hardware definition.
2. Add only the minimal supported peripheral or external-device behavior needed
   to satisfy that interaction.
3. Re-run the same bounded trace and retain the result as the new baseline.

Likely categories include RCC/clock readiness, timers and interrupts, CAN,
ADC/sensor inputs, GPIO expanders, and output drivers. Their order is dictated
by the trace, not presumed from the ECU's expected feature set.

## Validation

Phase 1 is complete when:

- Cargo builds successfully.
- The example configuration parses using an exact F7 SVD.
- The emulator reaches `0x002003D5` and executes a documented finite
  instruction window reproducibly.
- The trace has no configuration, mapping, or SVD-resolution errors.

Unmapped peripheral accesses are expected phase-1 observations, not evidence
that a device has been implemented.

## Error Handling

If the firmware accesses a region outside the initial maps, preserve the
emulator's existing fault logging and add that address to the phase-2 backlog.
If the exact MCU part or its SVD cannot be established from local snapshot
metadata and available board sources, stop instead of selecting a speculative
replacement.
