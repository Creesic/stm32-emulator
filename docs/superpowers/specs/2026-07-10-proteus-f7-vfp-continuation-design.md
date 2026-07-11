# Proteus F7 VFP Continuation Design

## Goal

Execute the unmodified Proteus F7 instruction at 0x002397f0 after the already
working VDIV.F32, without a firmware patch, instruction hook, or skipped opcode.

## Architecture

Add a raw-instruction regression for bytes b7 ee c7 7a using Unicorn's
CORTEX_M7 model. First evaluate the current upstream Unicorn development
branch against that regression. If it still rejects the instruction, vendor a
minimal upstream-compatible QEMU/Unicorn VFP decode correction, with the raw
instruction test and the bounded Proteus trace as required regressions.

## Constraints

- Retain Cortex-M7 selection and the existing SCB CPACR model.
- No software floating-point substitute or firmware binary modification.
- Keep the dependency change isolated and reproducible on Windows.
- Continue only when each new startup boundary is trace-verified.

## Validation

- The exact instruction executes under CORTEX_M7 in a standalone regression.
- The Proteus F7 launcher profile advances beyond 0x002397f0.
- The direct Proteus verification and full Rust test suite continue to pass.
