# Proteus F7 VFP Continuation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the next unmodified Proteus F7 VFP instruction at `0x002397f0` through the actual Unicorn Cortex-M7 core.

**Architecture:** First add a standalone regression for the exact four bytes that Unicorn currently rejects. Rebuild the project against pinned Unicorn upstream `dev` commit `e95899191fadb9e82421ecf8c92c40a40b93cb6a`; retain that dependency only if the regression and real firmware continuation pass. If upstream still fails, stop at a source-backed failure report and create a narrowly scoped core-patch plan from the actual decoder divergence rather than inventing a firmware-side workaround.

**Tech Stack:** Rust 2021, unicorn-engine upstream Git dependency, Cargo, CMake, Ninja, PowerShell.

## Global Constraints

- Retain `CpuModel::CortexM7` and the stateful `SCB.CPACR` model.
- Do not patch `rusefi.bin`, advance PC around an opcode, install an instruction hook, or substitute a software floating-point result.
- Use only the pinned upstream revision `e95899191fadb9e82421ecf8c92c40a40b93cb6a` for the initial comparison; never leave a moving branch dependency in `Cargo.toml`.
- All Cargo commands set `CMAKE_POLICY_VERSION_MINIMUM=3.5`, `CMAKE_GENERATOR=Ninja`, and `CARGO_TARGET_DIR=%LOCALAPPDATA%\stm32-emulator-proteus-f7-target`.
- Do not resume USB work until the real firmware executes the instruction at `0x002397f0` and reaches `0x002397f4`.
- Stage only files named by each task; preserve the unrelated OpenWolf, Codex, and launcher worktree changes.

---

## File Structure

- Modify `src/emulator.rs`: contain exact raw-opcode regressions that prove both Cortex-M7 VFP instructions execute as core instructions.
- Modify `Cargo.toml` and `Cargo.lock`: use the reproducible upstream Unicorn revision for the compatibility evaluation.
- Modify `proteus_f7/verify_fpu.ps1`: stop only after executing `0x002397f0`, rather than immediately before it.
- Modify `proteus_f7/README.md`: describe the stronger continuation check and its USB prerequisite.
- Create `docs/superpowers/specs/2026-07-10-proteus-f7-unicorn-core-failure-report.md` only when the pinned upstream revision still reports `INSN_INVALID`; it records raw-test output, pinned revision, and the exact upstream source location before any vendor patch is designed.

### Task 1: Add the exact VFP continuation regression

**Files:**
- Modify: `src/emulator.rs:273-300`

**Interfaces:**
- Consumes: `initialize_arm_engine(CpuModel) -> Result<Unicorn<'static, ()>>` and `RegisterARM` from the current emulator module.
- Produces: `cortex_m7_executes_proteus_vfp_continuation`, which accepts only a successful core execution of bytes `b7 ee c7 7a` at a Thumb address.

- [ ] **Step 1: Add the failing raw-instruction test beside the existing VDIV regression**

```rust
    #[test]
    fn cortex_m7_executes_proteus_vfp_continuation() {
        let proteus_vfp_continuation = [0xb7, 0xee, 0xc7, 0x7a];
        let mut uc = initialize_arm_engine(CpuModel::CortexM7).unwrap();
        uc.mem_map(0x1000, 0x1000, Prot::ALL).unwrap();
        uc.mem_write(0x1000, &proteus_vfp_continuation).unwrap();

        uc.emu_start(0x1001, 0x1004, 0, 1).unwrap();

        assert_eq!(uc.reg_read(RegisterARM::PC).unwrap(), 0x1004);
    }
```

The test deliberately asserts only architectural execution and the next PC. It must not assert a guessed mnemonic or invent input/output register values before a real decoder identifies the opcode.

- [ ] **Step 2: Run the focused regression against the released dependency**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
$env:CMAKE_GENERATOR='Ninja'
$env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo test cortex_m7_executes_proteus_vfp_continuation -- --exact
```

Expected: FAIL with `Unicorn error=INSN_INVALID`. Record the complete test output in `.wolf/buglog.json` as a second occurrence of `bug-028` before changing the dependency.

- [ ] **Step 3: Verify the existing preceding-instruction regression still passes**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
$env:CMAKE_GENERATOR='Ninja'
$env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo test cortex_m7_executes_proteus_vdiv -- --exact
```

Expected: PASS, proving the new failure boundary is the next firmware instruction and not a loss of Cortex-M7 VFP setup.

### Task 2: Evaluate the pinned upstream Unicorn core

**Files:**
- Modify: `Cargo.toml:8`
- Modify: `Cargo.lock`

**Interfaces:**
- Consumes: the Task 1 regression and the public `unicorn-engine` crate from the Unicorn monorepo.
- Produces: a locked `unicorn-engine` source package at revision `e95899191fadb9e82421ecf8c92c40a40b93cb6a`, or a documented evidence gate for a source-level core patch.

- [ ] **Step 1: Replace the released dependency with the pinned upstream revision**

```toml
# Cargo.toml
[dependencies]
unicorn-engine = { git = "https://github.com/unicorn-engine/unicorn.git", rev = "e95899191fadb9e82421ecf8c92c40a40b93cb6a" }
```

Leave every other dependency and the existing commented local `[patch.crates-io]` line unchanged. Run the following command so `Cargo.lock` captures the resolved Git revision:

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
$env:CMAKE_GENERATOR='Ninja'
$env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo update -p unicorn-engine
```

- [ ] **Step 2: Run both raw VFP tests against the pinned source build**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
$env:CMAKE_GENERATOR='Ninja'
$env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo test cortex_m7_executes_proteus_vdiv -- --exact
cargo test cortex_m7_executes_proteus_vfp_continuation -- --exact
```

Expected on a resolved upstream core: both tests PASS. If the continuation test still fails, do not change the raw test, CPU model, firmware, or emulator hook path; perform Task 4 instead.

- [ ] **Step 3: Check the source lock and compile the application binaries**

```powershell
rg -n 'source = "git\+https://github.com/unicorn-engine/unicorn.git\?rev=e95899191fadb9e82421ecf8c92c40a40b93cb6a' Cargo.lock
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
$env:CMAKE_GENERATOR='Ninja'
$env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo build --release --bins
```

Expected: Cargo.lock names the pinned upstream revision and all release binaries compile. If the Git revision is not present in Cargo.lock, restore the released dependency and stop; a reproducible comparison has not been established.

- [ ] **Step 4: Commit the raw regression and reproducible upstream evaluation**

```powershell
git add Cargo.toml Cargo.lock src/emulator.rs
git commit -m "test: evaluate upstream Unicorn VFP continuation"
```

Commit only after both raw tests pass. When the pinned revision still fails, do not commit a dependency change; continue to Task 4 with the released dependency restored.

### Task 3: Prove the real Proteus continuation and launcher behavior

**Files:**
- Modify: `proteus_f7/verify_fpu.ps1:36-57`
- Modify: `proteus_f7/README.md`

**Interfaces:**
- Consumes: the upstream core selected in Task 2, local ignored `rusefi.bin` and `STM32F767.svd`, and the existing `proteus_f7/config.yaml` Cortex-M7 profile.
- Produces: a script that reports success only after execution continues from `0x002397f0` to `0x002397f4`.

- [ ] **Step 1: Strengthen the firmware stop boundary and diagnostics**

Change the command and messages in `verify_fpu.ps1` to the following values:

```powershell
$output = (& cargo run --release --bin stm32-emulator -- config.yaml --stop-addr 0x002397f4 --max-instructions 1000000 --color never 2>&1 | Out-String)

if ($output -match 'INSN_INVALID') {
    throw "Unicorn rejected the Proteus VFP continuation at 0x002397f0:$([Environment]::NewLine)$output"
}

Write-Host 'Proteus F7 VFP continuation verified.'
```

Retain the script's asset preflight, environment save/restore, nonzero-exit check, and `Stop address reached, stopping` assertion. This stop address is `0x002397f4`, so Unicorn has executed the four bytes beginning at `0x002397f0`.

- [ ] **Step 2: Run the direct firmware verification**

```powershell
powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_fpu.ps1
```

Expected: exit code 0 and `Proteus F7 VFP continuation verified.` If execution reaches a later blocker, log that exact program counter and output; do not model an unobserved peripheral in this task.

- [ ] **Step 3: Document the stronger boundary**

Add this exact command to `proteus_f7/README.md`:

```powershell
powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_fpu.ps1
```

State that it executes both the original VDIV at `0x002397ec` and the next VFP instruction at `0x002397f0`, stopping at `0x002397f4`; it is the prerequisite for resuming USB CDC-over-TCP work.

- [ ] **Step 4: Verify the GUI profile manually with the real executable**

```powershell
$launcher = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target\release\stm32-launcher.exe'
Start-Process -FilePath $launcher -WindowStyle Hidden
```

In the launcher, select the Proteus F7 profile, select the snapshot `rusefi.bin`, then press Run emulator. Expected: the Emulator Output panel advances past `pc=0x002397f0` and does not report `INSN_INVALID` at that address. Close the launcher before any subsequent release build.

- [ ] **Step 5: Commit the real-firmware integration gate**

```powershell
git add proteus_f7/verify_fpu.ps1 proteus_f7/README.md
git commit -m "test: verify Proteus VFP continuation"
```

### Task 4: Evidence gate when upstream still rejects the opcode

**Files:**
- Modify: `Cargo.toml:8`
- Modify: `Cargo.lock`
- Create: `docs/superpowers/specs/2026-07-10-proteus-f7-unicorn-core-failure-report.md`

**Interfaces:**
- Consumes: a failing Task 2 continuation test against the immutable upstream revision and the released 2.1.5 baseline.
- Produces: a source-backed failure report that names the actual Unicorn/QEMU decode or translation location to change; it is the required input to a separate minimal vendor-patch design.

- [ ] **Step 1: Restore the released dependency after the failed comparison**

```toml
# Cargo.toml
[dependencies]
unicorn-engine = "2.1.5"
```

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
$env:CMAKE_GENERATOR='Ninja'
$env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo update -p unicorn-engine --precise 2.1.5
cargo test cortex_m7_executes_proteus_vfp_continuation -- --exact
```

Expected: the raw continuation regression fails with the known `INSN_INVALID` error on the released baseline, while the existing VDIV regression passes.

- [ ] **Step 2: Capture the exact upstream decoder path before proposing a patch**

```powershell
$upstream = Join-Path $env:TEMP "unicorn-proteus-vfp-dev-$PID"
git clone --no-checkout https://github.com/unicorn-engine/unicorn.git $upstream
git -C $upstream checkout --detach e95899191fadb9e82421ecf8c92c40a40b93cb6a
rg -n -i 'vfp|vcvt|vadd|vsub|vmla|vmls' "$upstream\qemu\target\arm" "$upstream\qemu\tcg\arm"
```

Expected: the checkout reports the pinned detached revision and the search identifies the actual ARM VFP decode/translation files available at that revision. Do not edit the clone or copy it into this repository during this task.

- [ ] **Step 3: Write the evidence report**

Create `docs/superpowers/specs/2026-07-10-proteus-f7-unicorn-core-failure-report.md` with all of the following concrete evidence:

```markdown
# Proteus F7 Unicorn Core Failure Report

- Released baseline: unicorn-engine 2.1.5
- Upstream comparison: e95899191fadb9e82421ecf8c92c40a40b93cb6a
- Raw bytes: b7 ee c7 7a
- Execution address: 0x00001001 (Thumb), corresponding firmware address 0x002397f0
- Required result: one successful Cortex-M7 core instruction, PC 0x00001004
- Observed released result: exact cargo-test failure output
- Observed upstream result: exact cargo-test failure output
- Candidate upstream source locations: exact paths and line numbers from the Task 4 search
- Excluded remedies: firmware patch, PC skip, Rust instruction hook, software FPU
```

Use the copied test output and `rg` line numbers, not inferred mnemonic names. Append the failure evidence to `bug-028` in `.wolf/buglog.json`.

- [ ] **Step 4: Commit the evidence report only**

```powershell
git add docs/superpowers/specs/2026-07-10-proteus-f7-unicorn-core-failure-report.md
git commit -m "docs: record Proteus Unicorn VFP core failure"
```

The next action is a new focused design and plan for the one confirmed upstream source change. Do not vendor a broad fork or create a local instruction emulation layer from this evidence-gathering task.

## Final Verification

- [ ] `cargo test cortex_m7_executes_proteus_vdiv -- --exact` passes.
- [ ] `cargo test cortex_m7_executes_proteus_vfp_continuation -- --exact` passes on the pinned upstream dependency.
- [ ] `cargo test` passes with the required CMake, Ninja, and target-directory environment.
- [ ] `cargo build --release --bins` passes after the launcher executable is closed.
- [ ] `powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_boot.ps1` passes.
- [ ] `powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_fpu.ps1` reaches `0x002397f4` without `INSN_INVALID`.
- [ ] The launcher’s Proteus F7 output visibly advances past `pc=0x002397f0`.
- [ ] `git diff --check` is clean for plan-owned files.
