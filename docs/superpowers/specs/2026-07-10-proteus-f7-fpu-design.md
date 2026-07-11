# Proteus F7 FPU Execution Design

## Goal

Execute the unmodified Proteus F7 firmware’s Cortex-M7 single-precision FPU
instructions under Unicorn so startup can progress beyond the observed
VDIV.F32 instruction at 0x002397ec (whose next PC is 0x002397f0).

## Architecture

Upgrade the Unicorn Rust binding to version 2.1.5 and select an ARM CPU model
explicitly when creating the MCLASS engine. The emulator configuration gains a
required CPU model field; Proteus selects Cortex-M7 and the existing STM32F4
examples select Cortex-M4.

The SCB model retains CPACR at offset 0x88. The existing firmware write of
0x00F00000 is then observable through the same memory-mapped architectural
register it uses on hardware.

## Scope

- Use Unicorn’s supported CORTEX_M7 CPU model through ctl_set_cpu_model.
- Add YAML CPU model values cortex-m4 and cortex-m7.
- Add a direct regression test that maps and executes the exact four-byte
  VDIV.F32 instruction from the Proteus image, with initialized S0 and S1,
  then verifies S14 contains the correct IEEE-754 result.
- Verify the Proteus bounded trace passes the former VDIV.F32 PC without an
  INSN_INVALID error.

## Non-Goals

- No custom floating-point instruction hook, software FPU, patched firmware,
  or local Unicorn fork.
- No change to USB peripheral behavior in this increment.
- No claim that FPU execution alone reaches USB initialization; subsequent
  startup traces remain evidence-driven.

## Validation

The CPU-model test must fail on the current binding/configuration and pass
only with the selected Cortex-M7 model. SCB tests verify CPACR write/read
round-trip. The Proteus run records its next startup boundary after the former
VDIV.F32 failure and must not report INSN_INVALID while executing the
instruction at 0x002397ec.
