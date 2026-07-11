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
    'model: cortex-m7',
    'svd: STM32F767.svd',
    'vector_table: 0x00200000'
)) {
    if (-not $config.Contains($line)) {
        throw "Configuration contract is missing: $line"
    }
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
    $output = (& cargo run --release --bin stm32-emulator -- config.yaml --stop-addr 0x002397f4 --max-instructions 1000000 --color never 2>&1 | Out-String)
    $exitCode = $LASTEXITCODE
} finally {
    Pop-Location
    $ErrorActionPreference = $originalErrorActionPreference
    $env:CMAKE_POLICY_VERSION_MINIMUM = $originalCmakePolicyVersion
    $env:CMAKE_GENERATOR = $originalCmakeGenerator
    $env:CARGO_TARGET_DIR = $originalCargoTargetDirectory
}

if ($exitCode -ne 0) {
    throw "FPU boundary run failed:$([Environment]::NewLine)$output"
}

if ($output -notmatch 'Stop address reached, stopping') {
    throw "The run did not reach the post-VDIV stop address:$([Environment]::NewLine)$output"
}

if ($output -match 'INSN_INVALID') {
    throw "Unicorn rejected the Proteus VFP continuation at 0x002397f0:$([Environment]::NewLine)$output"
}

Write-Host 'Proteus F7 VFP continuation verified.'
