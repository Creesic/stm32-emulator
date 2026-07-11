# Proteus F7 FPU Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the unmodified Proteus STM32F767 firmware through its observed VDIV.F32 instruction by upgrading Unicorn and explicitly selecting a Cortex-M7 core.

**Architecture:** Keep CPU-family selection in deserialized emulator configuration and translate that value to Unicorn only while the MCLASS engine is created. Keep CPACR as an SCB-owned architectural register, then prove core capability with the original four VDIV bytes before asking the full firmware to reach the post-instruction program counter.

**Tech Stack:** Rust 2021, unicorn-engine 2.1.5, serde_yaml, PowerShell, Cargo, Ninja.

## Global Constraints

- Upgrade to exactly unicorn-engine 2.1.5; do not use a fork or an instruction hook.
- CPU model values are required YAML strings: cortex-m4 and cortex-m7.
- Proteus uses cortex-m7; the Saturn and Mono X STM32F407 configurations use cortex-m4.
- Preserve unmodified firmware and do not implement a software FPU.
- Keep USB changes out of this plan; resume the CDC-over-TCP plan only after the Proteus FPU trace passes.
- All Cargo commands on this machine set CMAKE_POLICY_VERSION_MINIMUM=3.5, CMAKE_GENERATOR=Ninja, and CARGO_TARGET_DIR to the local Proteus target directory.
- Stage only the files named by each task; the checkout already contains unrelated launcher and startup-model work.

---

## File Structure

- Modify Cargo.toml and Cargo.lock: move the Unicorn Rust binding to 2.1.5 so CPU-model controls are available.
- Modify src/config.rs: add the required, serde-deserializable CpuModel enum to CPU configuration.
- Modify src/system.rs: update the three Unicorn memory-mapping calls to the 2.1.5 `Prot` and `u64` API.
- Modify src/emulator.rs: construct the MCLASS engine with the configured Cortex-M4 or Cortex-M7 Unicorn model and host the exact VDIV regression.
- Modify src/peripherals/scb.rs and src/peripherals/mod.rs: retain SCB.CPACR and guarantee that the actual SCB peripheral slot covers offset 0x88.
- Modify proteus_f7/config.yaml, saturn/config.yaml, and monox/config.yaml: select their concrete CPU family.
- Create proteus_f7/verify_fpu.ps1: bounded, non-verbose firmware gate ending immediately after the known VDIV instruction.
- Modify proteus_f7/README.md: document the new FPU verification command.

### Task 1: Upgrade Unicorn and make CPU family explicit

**Files:**
- Modify: Cargo.toml
- Modify: Cargo.lock
- Modify: src/config.rs
- Modify: src/system.rs
- Modify: proteus_f7/config.yaml
- Modify: saturn/config.yaml
- Modify: monox/config.yaml

**Interfaces:**
- Consumes: the top-level cpu YAML mapping already deserialized by Config.
- Produces: CpuModel::{CortexM4, CortexM7} and Cpu { model: CpuModel, .. } for the later Unicorn engine-selection task.

- [ ] **Step 1: Write a failing YAML CPU-model test in src/config.rs**

```rust
#[cfg(test)]
mod tests {
    use super::{Config, CpuModel};

    #[test]
    fn cpu_model_deserializes_kebab_case_name() {
        let config: Config = serde_yaml::from_str(
            "cpu:\n  model: cortex-m7\n  svd: chip.svd\n  vector_table: 0x00200000\nregions: []",
        )
        .unwrap();

        assert_eq!(config.cpu.model, CpuModel::CortexM7);
    }
}
```

- [ ] **Step 2: Run the focused test before implementation**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test cpu_model_deserializes_kebab_case_name
```

Expected: compilation fails because CpuModel and Cpu.model do not yet exist.

- [ ] **Step 3: Implement required CPU-family deserialization and update example configurations**

```rust
// src/config.rs
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CpuModel {
    CortexM4,
    CortexM7,
}

#[derive(Debug, Deserialize)]
pub struct Cpu {
    pub model: CpuModel,
    pub svd: String,
    pub vector_table: u32,
}
```

Change the dependency line to unicorn-engine = "2.1.5", then run cargo update -p unicorn-engine --precise 2.1.5. Add model: cortex-m7 to the Proteus config and model: cortex-m4 to both STM32F407 example configs.

The 2.1.5 binding renames `Permission` to `Prot` and accepts `u64` map lengths. In src/system.rs, import `unicorn_const::Prot`, pass `(end - start) as u64` to `mmio_map`, and pass `size as u64` with `Prot::ALL` to `mem_map`; retain `size` as `usize` for the existing slice bound.

- [ ] **Step 4: Run the focused test and compile every binary**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test cpu_model_deserializes_kebab_case_name; cargo build --release --bins
```

Expected: the deserialization test passes and all binaries compile against Unicorn 2.1.5.

- [ ] **Step 5: Commit only the dependency, configuration, and CPU-core selection files**

```powershell
git add Cargo.toml Cargo.lock src/config.rs src/system.rs proteus_f7/config.yaml saturn/config.yaml monox/config.yaml
git commit -m "feat: select Cortex-M model for emulation"
```

### Task 2: Model and route SCB.CPACR

**Files:**
- Modify: src/peripherals/scb.rs
- Modify: src/peripherals/mod.rs

**Interfaces:**
- Consumes: aligned SCB relative register offsets supplied by Peripherals::read and Peripherals::write.
- Produces: a stateful CPACR read/write at offset 0x88 and an SCB model range of base through base + 0x8f inclusive.

- [ ] **Step 1: Write failing CPACR and range tests**

```rust
// src/peripherals/scb.rs test module
#[test]
fn cpacr_retains_the_firmware_fpu_enable_value() {
    let mut scb = Scb::default();
    scb.write_cpacr(0x00f0_0000);
    assert_eq!(scb.read_cpacr(), 0x00f0_0000);
}

// src/peripherals/mod.rs test module
#[test]
fn scb_model_range_includes_cpacr() {
    assert_eq!(
        Peripherals::modeled_range("SCB", 0xe000_ed00, 4),
        (0xe000_ed00, 0xe000_ed8f),
    );
}
```

- [ ] **Step 2: Run the two tests before implementation**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test cpacr_retains_the_firmware_fpu_enable_value; cargo test scb_model_range_includes_cpacr
```

Expected: compilation fails because the CPACR helper and modeled-range function do not exist.

- [ ] **Step 3: Store CPACR and expand only the real SCB model slot**

```rust
// src/peripherals/scb.rs
#[derive(Default)]
pub struct Scb {
    cpacr: u32,
}

impl Scb {
    fn write_cpacr(&mut self, value: u32) {
        self.cpacr = value;
    }

    fn read_cpacr(&self) -> u32 {
        self.cpacr
    }
}

impl Peripheral for Scb {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0088 => self.read_cpacr(),
            _ => 0,
        }
    }

    fn write(&mut self, sys: &System, offset: u32, value: u32) {
        match offset {
            0x0004 => { /* retain the existing ICSR interrupt behavior */ }
            0x0088 => self.write_cpacr(value),
            _ => {}
        }
    }
}

// src/peripherals/mod.rs
pub fn modeled_range(name: &str, base: u32, size: u32) -> (u32, u32) {
    match name {
        "FSMC" => (0x6000_0000, 0xa000_1000),
        "SCB" => (base, base + 0x8f),
        _ => (base, base + size),
    }
}
```

Call modeled_range from register_peripheral after retaining the original SVD range for debug-only register names. Keep both existing ICSR pending-interrupt branches intact.

- [ ] **Step 4: Run the focused tests and existing peripheral tests**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test cpacr_retains_the_firmware_fpu_enable_value; cargo test scb_model_range_includes_cpacr; cargo test peripherals::tests
```

Expected: both new tests and the established PWR, FLASH, and TIM11 peripheral tests pass.

- [ ] **Step 5: Commit the SCB behavior**

```powershell
git add src/peripherals/scb.rs src/peripherals/mod.rs
git commit -m "feat: retain SCB CPACR configuration"
```

### Task 3: Prove Cortex-M7 executes the exact Proteus VDIV instruction

**Files:**
- Modify: src/emulator.rs

**Interfaces:**
- Consumes: CpuModel::CortexM7 from Task 1 and Unicorn CPU-model controls from the upgraded binding.
- Produces: initialize_arm_engine(model: CpuModel) -> anyhow::Result<Unicorn<()>> and an emulator unit test tied to the four firmware bytes 80 ee 20 7a at Proteus address 0x002397ec.

- [ ] **Step 1: Add the direct FPU regression test**

```rust
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
        uc.reg_write(RegisterARM::S0, 9.0_f32.to_bits() as u64).unwrap();
        uc.reg_write(RegisterARM::S1, 2.0_f32.to_bits() as u64).unwrap();

        uc.emu_start(0x1001, 0x1004, 0, 1).unwrap();

        assert_eq!(
            uc.reg_read(RegisterARM::S14).unwrap() as u32,
            4.5_f32.to_bits(),
        );
    }
}
```

- [ ] **Step 2: Run the exact instruction test**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test cortex_m7_executes_proteus_vdiv -- --exact
```

Expected: compilation fails because initialize_arm_engine does not yet exist. If the compiler instead reaches a Unicorn error, record that exact error and stop; do not introduce an instruction hook or substitute a software result.


- [ ] **Step 3: Implement explicit MCLASS CPU-model selection**

```rust
use unicorn_engine::{ArmCpuModel, Unicorn};

fn initialize_arm_engine(model: CpuModel) -> Result<Unicorn<()>> {
    let mut uc = Unicorn::new(Arch::ARM, Mode::MCLASS | Mode::LITTLE_ENDIAN)
        .map_err(UniErr).context("Failed to initialize Unicorn instance")?;
    let unicorn_model = match model {
        CpuModel::CortexM4 => ArmCpuModel::CORTEX_M4 as i32,
        CpuModel::CortexM7 => ArmCpuModel::CORTEX_M7 as i32,
    };
    uc.ctl_set_cpu_model(unicorn_model)
        .map_err(UniErr).context("Failed to select configured ARM CPU model")?;
    Ok(uc)
}

// At the start of run_emulator:
let mut uc = initialize_arm_engine(config.cpu.model)?;
```

Do not change execution hooks or the error-recovery path. The helper must run before system::prepare maps memory or firmware starts executing.

- [ ] **Step 4: Re-run the exact instruction test**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test cortex_m7_executes_proteus_vdiv -- --exact
```

Expected: PASS with S14 holding the IEEE-754 bit pattern for 4.5.


- [ ] **Step 5: Format only the touched Rust files and run all Rust tests**

```powershell
rustfmt src/config.rs src/emulator.rs src/peripherals/scb.rs src/peripherals/mod.rs
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test
```

Expected: the VDIV regression and all repository tests pass. Do not run repository-wide formatter checks because the known baseline has unrelated formatting drift.

- [ ] **Step 6: Commit the direct execution proof**

```powershell
git add src/emulator.rs
git commit -m "test: verify Cortex-M7 FPU divide execution"
```

### Task 4: Gate the real Proteus boot trace at the post-VDIV PC

**Files:**
- Create: proteus_f7/verify_fpu.ps1
- Modify: proteus_f7/README.md

**Interfaces:**
- Consumes: local ignored rusefi.bin and STM32F767.svd assets, plus the Proteus CPU model configuration.
- Produces: a PowerShell verification that succeeds only when emulation reaches 0x002397f0, immediately after executing the VDIV at 0x002397ec.

- [ ] **Step 1: Create a failing bounded firmware check**

```powershell
# proteus_f7/verify_fpu.ps1 command body
$output = (& cargo run --release --bin stm32-emulator -- config.yaml --stop-addr 0x002397f0 --max-instructions 1000000 --color never 2>&1 | Out-String)
$exitCode = $LASTEXITCODE
if ($exitCode -ne 0) { throw "FPU boundary run failed:$([Environment]::NewLine)$output" }
if ($output -notmatch 'Stop address reached, stopping') {
    throw "The run did not reach the post-VDIV stop address:$([Environment]::NewLine)$output"
}
if ($output -match 'INSN_INVALID') {
    throw "Unicorn rejected the Proteus VDIV instruction:$([Environment]::NewLine)$output"
}
```

Follow verify_boot.ps1 for asset preflight, environment preservation, Push-Location to the example directory, and restoration in finally. The new script must write Proteus F7 FPU boundary verified. only after all checks pass.

- [ ] **Step 2: Run the script against the real local assets**

```powershell
powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_fpu.ps1
```

Expected: exit code 0 and Proteus F7 FPU boundary verified. If another emulation blocker appears before 0x002397f0, capture the bounded output, append it to the OpenWolf bug log, and stop rather than modelling unobserved hardware.

- [ ] **Step 3: Document and re-run the verification**

Add a Run FPU boundary verification subsection to proteus_f7/README.md with the exact PowerShell command above and a sentence that this is the prerequisite for continuing virtual USB work. Re-run the script after documentation changes.

- [ ] **Step 4: Commit the integration gate and documentation**

```powershell
git add proteus_f7/verify_fpu.ps1 proteus_f7/README.md
git commit -m "test: verify Proteus F7 FPU boot boundary"
```

## Final Verification

- [ ] Run cargo test with the required CMake and Ninja environment.
- [ ] Run cargo build --release --bins with the same environment.
- [ ] Run powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_boot.ps1.
- [ ] Run powershell -ExecutionPolicy Bypass -File .\proteus_f7\verify_fpu.ps1.
- [ ] Confirm git diff --check is clean for the files changed by this plan.
