# rusEFI Proteus F767 Bring-up

The pinned `rusefi.bin` firmware image and `STM32F767.svd` peripheral
description are included in the repository so this harness works from a clean
checkout. To replace them with another local firmware snapshot and the latest
SVD, run:

```powershell
.\setup.ps1
```

Verify reset-handler entry:

```powershell
.\verify_boot.ps1
```

Verify the Cortex-M7 VFP continuation before continuing USB work. This runs
the VDIV at 0x002397ec and the next VFP instruction at 0x002397f0, stopping at
0x002397f4:

```powershell
.\verify_fpu.ps1
```

Capture a bounded peripheral trace:

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:CMAKE_GENERATOR = 'Ninja'
$env:CARGO_TARGET_DIR = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'
cargo run --release --bin stm32-emulator -- config.yaml --max-instructions 50000 --busy-loop-stop --color never -v 2>&1 | Tee-Object baseline-trace.log
```
