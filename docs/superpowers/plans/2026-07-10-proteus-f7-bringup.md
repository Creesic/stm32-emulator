# Proteus F7 Bring-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a reproducible STM32F767 boot harness for the supplied rusEFI Proteus F7 firmware and verify that execution enters its reset handler.

**Architecture:** A tracked `proteus_f7/` example holds only configuration, setup, validation, and usage documentation. `setup.ps1` copies the user-supplied firmware locally and downloads TinyGo's maintained, vendor-derived F767 SVD, both ignored by Git; `verify_boot.ps1` asserts the known vectors, configuration contract, and one-instruction reset-handler smoke run.

**Tech Stack:** Rust/Cargo emulator, Unicorn Cortex-M emulation, YAML configuration, PowerShell 5+, STMicroelectronics STM32F7 SVD archive.

## Global Constraints

- The target MCU is STM32F767, confirmed by rusEFI's Proteus F767 programming documentation.
- Map `rusefi.bin` at both `0x00200000` and `0x08000000`; set `cpu.vector_table` to `0x00200000`.
- Preserve the firmware's observed initial SP `0x20021000` and reset vector `0x002003D5`.
- Do not add firmware patches or configured external devices during phase 1.
- Do not commit `rusefi.bin`, `STM32F767.svd`, or generated trace logs.
- Run the emulator from `proteus_f7/` so asset references remain relative.
- Set `CMAKE_POLICY_VERSION_MINIMUM=3.5`, `CMAKE_GENERATOR=Ninja`, and `CARGO_TARGET_DIR=%LOCALAPPDATA%\\stm32-emulator-proteus-f7-target` for Cargo commands on this CMake 4 installation.

---

## File Structure

- `.gitignore` — Excludes local firmware, downloaded SVD, and baseline trace output.
- `proteus_f7/setup.ps1` — Copies `rusefi.bin` from the supplied snapshot and acquires TinyGo's maintained `stm32f767.svd`.
- `proteus_f7/config.yaml` — Maps the F767 code-interface alias and RAM regions; declares no emulated board devices.
- `proteus_f7/verify_boot.ps1` — Validates assets and vectors, then proves reset-handler entry through a one-instruction emulator run.
- `proteus_f7/README.md` — Exact setup, verification, and bounded trace commands.

### Task 1: Create the reproducible F767 boot harness

**Files:**

- Modify: `.gitignore`
- Create: `proteus_f7/setup.ps1`
- Create: `proteus_f7/config.yaml`
- Create: `proteus_f7/verify_boot.ps1`
- Create: `proteus_f7/README.md`

**Interfaces:**

- Consumes: `C:\\Users\\Tera\\Desktop\\Epictuner\\rusefi.snapshot.proteus_f7\\rusefi.bin` and ST's `https://www.st.com/resource/en/svd/stm32f7_svd.zip`.
- Produces: ignored `proteus_f7/rusefi.bin` and `proteus_f7/STM32F767.svd`, plus a zero-exit `proteus_f7/verify_boot.ps1` smoke test.

- [ ] **Step 1: Write the failing preflight test**

Create `proteus_f7/verify_boot.ps1` with this content. It must initially fail because neither `config.yaml` nor the local assets exist.

```powershell
param([string]$ExampleDirectory = $PSScriptRoot)

$ErrorActionPreference = 'Stop'
$requiredFiles = @('config.yaml', 'rusefi.bin', 'STM32F767.svd')
foreach ($file in $requiredFiles) {
    $path = Join-Path $ExampleDirectory $file
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing required asset: $path. Run .\\setup.ps1 first."
    }
}

$config = Get-Content -Raw (Join-Path $ExampleDirectory 'config.yaml')
foreach ($line in @(
    'svd: STM32F767.svd',
    'vector_table: 0x00200000',
    'start: 0x00200000',
    'load: rusefi.bin',
    'start: 0x20000000',
    'start: 0x20020000',
    'start: 0x2007c000'
)) {
    if (-not $config.Contains($line)) {
        throw "Configuration contract is missing: $line"
    }
}

$image = [System.IO.File]::ReadAllBytes((Join-Path $ExampleDirectory 'rusefi.bin'))
if ($image.Length -lt 8) {
    throw 'Firmware image is shorter than its vector table.'
}
if ([BitConverter]::ToUInt32($image, 0) -ne 0x20021000) {
    throw 'Unexpected initial stack pointer.'
}
if ([BitConverter]::ToUInt32($image, 4) -ne 0x002003D5) {
    throw 'Unexpected reset vector.'
}

Push-Location $ExampleDirectory
try {
    $output = (& cargo run --release -- config.yaml --max-instructions 1 --color never -vvvv 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw "One-instruction emulator run failed:$([Environment]::NewLine)$output"
    }
} finally {
    Pop-Location
}

if ($output -notmatch 'pc=0x002003d4') {
    throw "The trace did not enter the reset handler:$([Environment]::NewLine)$output"
}

Write-Host 'Proteus F7 boot harness verified.'
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```powershell
& .\proteus_f7\verify_boot.ps1
```

Expected: failure beginning with `Missing required asset:` for `proteus_f7/config.yaml`.

- [ ] **Step 3: Add the setup script, configuration, ignore rules, and usage documentation**

Add these entries to `.gitignore`:

```gitignore
/proteus_f7/rusefi.bin
/proteus_f7/STM32F767.svd
/proteus_f7/baseline-trace.log
```

Create `proteus_f7/setup.ps1`:

```powershell
param(
    [string]$SnapshotDirectory = 'C:\Users\Tera\Desktop\Epictuner\rusefi.snapshot.proteus_f7'
)

$ErrorActionPreference = 'Stop'
$destination = $PSScriptRoot
$firmware = Join-Path $SnapshotDirectory 'rusefi.bin'
if (-not (Test-Path -LiteralPath $firmware -PathType Leaf)) {
    throw "Firmware image not found: $firmware"
}

Copy-Item -LiteralPath $firmware -Destination (Join-Path $destination 'rusefi.bin') -Force

$archive = Join-Path $env:TEMP 'stm32f7_svd.zip'
$expanded = Join-Path $env:TEMP 'stm32f7_svd'
Remove-Item -LiteralPath $expanded -Recurse -Force -ErrorAction SilentlyContinue
Invoke-WebRequest -Uri 'https://www.st.com/resource/en/svd/stm32f7_svd.zip' -OutFile $archive
Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force

$svd = Get-ChildItem -LiteralPath $expanded -Recurse -File -Filter 'STM32F767.svd' | Select-Object -First 1
if ($null -eq $svd) {
    throw 'STM32F767.svd was not present in the ST STM32F7 SVD archive.'
}
Copy-Item -LiteralPath $svd.FullName -Destination (Join-Path $destination 'STM32F767.svd') -Force
Write-Host 'Prepared rusefi.bin and STM32F767.svd.'
```

Create `proteus_f7/config.yaml`:

```yaml
cpu:
  svd: STM32F767.svd
  vector_table: 0x00200000
regions:
  - name: ROM-ITCM-alias
    start: 0x00200000
    load: rusefi.bin
    size: 0x200000
  - name: ROM-AXI-alias
    start: 0x08000000
    load: rusefi.bin
    size: 0x200000
  - name: ITCM-RAM
    start: 0x00000000
    size: 0x4000
  - name: DTCM-RAM
    start: 0x20000000
    size: 0x20000
  - name: SRAM1
    start: 0x20020000
    size: 0x5c000
  - name: SRAM2
    start: 0x2007c000
    size: 0x4000
```

Create `proteus_f7/README.md`:

````markdown
# rusEFI Proteus F767 Bring-up

From this directory, initialize the local ignored firmware assets:

```powershell
.\setup.ps1
```

Verify reset-handler entry:

```powershell
.\verify_boot.ps1
```

Capture a bounded peripheral trace:

```powershell
cargo run --release -- config.yaml --max-instructions 50000 --busy-loop-stop --color never -v 2>&1 | Tee-Object baseline-trace.log
```
````

- [ ] **Step 4: Run the test to verify it passes**

Run:

```powershell
& .\proteus_f7\setup.ps1
& .\proteus_f7\verify_boot.ps1
```

Expected: `Proteus F7 boot harness verified.`

- [ ] **Step 5: Commit the harness**

```powershell
git add .gitignore proteus_f7
git commit -m "feat: add Proteus F767 boot harness"
```

### Task 2: Capture and review the phase-1 peripheral baseline

**Files:**

- Create: `proteus_f7/baseline-trace.log` (ignored local artifact)
- Modify: `proteus_f7/README.md` only if the observed invocation differs from the documented command.

**Interfaces:**

- Consumes: the passing Task 1 harness.
- Produces: a bounded trace and a ranked list of the first unmodeled peripheral interactions required for phase 2.

- [ ] **Step 1: Run the bounded baseline**

Run:

```powershell
Push-Location .\proteus_f7
try {
    cargo run --release -- config.yaml --max-instructions 50000 --busy-loop-stop --color never -v 2>&1 | Tee-Object baseline-trace.log
    if ($LASTEXITCODE -ne 0) {
        throw "Baseline trace failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}
```

Expected: a zero-exit trace saved to `proteus_f7/baseline-trace.log`; unmapped MMIO warnings are retained as phase-2 evidence.

- [ ] **Step 2: Verify reset entry remains deterministic**

Run:

```powershell
& .\proteus_f7\verify_boot.ps1
Select-String -LiteralPath .\proteus_f7\baseline-trace.log -Pattern 'MEM_.*UNMAPPED|Starting emulation|Busy loop reached|Reached target number of instructions'
```

Expected: the verifier succeeds and the selected trace lines identify either a natural bounded stop or concrete unmapped addresses for follow-up.

- [ ] **Step 3: Document only observed invocation corrections**

If Task 2's command requires a documented flag change to finish successfully, update the bounded-trace block in `proteus_f7/README.md` with the exact command that ran. Do not add peripheral configuration or firmware patches during this task.

- [ ] **Step 4: Commit tracked documentation changes**

```powershell
git add proteus_f7/README.md
git commit -m "docs: record Proteus F767 baseline run"
```

If the README is unchanged, do not create an empty commit.

## Plan Self-Review

- Spec coverage: Task 1 implements exact MCU/SVD selection, code and RAM mappings, no-fabrication constraints, setup reproducibility, and reset-handler verification. Task 2 provides the bounded trace and turns observed unmapped access into phase-2 evidence.
- Placeholder scan: This plan contains no deferred implementation markers; every created file and command is specified.
- Interface consistency: `setup.ps1` creates the two ignored inputs required by `verify_boot.ps1`, and `config.yaml` is the single configuration consumed by both the verifier and baseline command.
