param([string]$ExampleDirectory = $PSScriptRoot)

$ErrorActionPreference = 'Stop'
$requiredFiles = @('config.yaml', 'rusefi.bin', 'STM32F767.svd')
foreach ($file in $requiredFiles) {
    $path = Join-Path $ExampleDirectory $file
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing required asset: $path. Run .\setup.ps1 first."
    }
}

$config = Get-Content -Raw (Join-Path $ExampleDirectory 'config.yaml')
foreach ($line in @(
    'svd: STM32F767.svd',
    'vector_table: 0x00200000',
    'start: 0x00200000',
    'start: 0x08000000',
    'load: rusefi.bin',
    'start: 0x20000000',
    'start: 0x20020000',
    'start: 0x2007c000',
    'peripheral: OTG_FS_GLOBAL',
    'listen: 127.0.0.1:29000',
    'max_buffered_bytes: 65536',
    'ecu_io:',
    'listen: 127.0.0.1:29002',
    'name: din1',
    'name: ls16',
    'name: ign12',
    'name: av11',
    'name: vbatt'
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

$originalErrorActionPreference = $ErrorActionPreference
$originalCmakePolicyVersion = $env:CMAKE_POLICY_VERSION_MINIMUM
$originalCmakeGenerator = $env:CMAKE_GENERATOR
$originalCargoTargetDirectory = $env:CARGO_TARGET_DIR
$compatibilityTargetDirectory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'

Push-Location $ExampleDirectory
try {
    $ErrorActionPreference = 'Continue'
    $env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
    $env:CMAKE_GENERATOR = 'Ninja'
    $env:CARGO_TARGET_DIR = $compatibilityTargetDirectory
    $output = (& cargo run --release --bin stm32-emulator -- config.yaml --max-instructions 1 --color never -vvvv 2>&1 | Out-String)
    $exitCode = $LASTEXITCODE
} finally {
    Pop-Location
    $ErrorActionPreference = $originalErrorActionPreference
    $env:CMAKE_POLICY_VERSION_MINIMUM = $originalCmakePolicyVersion
    $env:CMAKE_GENERATOR = $originalCmakeGenerator
    $env:CARGO_TARGET_DIR = $originalCargoTargetDirectory
}

if ($exitCode -ne 0) {
    throw "One-instruction emulator run failed:$([Environment]::NewLine)$output"
}

if ($output -notmatch 'pc=0x002003d4') {
    throw "The trace did not enter the reset handler:$([Environment]::NewLine)$output"
}

Write-Host 'Proteus F7 boot harness verified.'
