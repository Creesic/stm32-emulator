# rusEFI Proteus F767 Bring-up

From this directory, initialize the local ignored firmware assets:

```powershell
.\setup.ps1
```

Verify reset-handler entry:

```powershell
.\verify_boot.ps1
```

Verify the Cortex-M7 floating-point boundary before continuing USB work:

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
