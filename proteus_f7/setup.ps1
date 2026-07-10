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

$svdDestination = Join-Path $destination 'STM32F767.svd'
Invoke-WebRequest -Uri 'https://raw.githubusercontent.com/tinygo-org/stm32-svd/main/svd/stm32f767.svd' -OutFile $svdDestination
if ((Get-Item -LiteralPath $svdDestination).Length -eq 0) {
    throw 'The downloaded STM32F767 SVD is empty.'
}
Write-Host 'Prepared rusefi.bin and STM32F767.svd.'
