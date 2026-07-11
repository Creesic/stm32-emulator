# Native Firmware Launcher

The native launcher is a Dear ImGui desktop front end for the existing
stm32-emulator command-line program. It never embeds a second emulator core:
it writes a temporary YAML configuration, starts the CLI as a child, and shows
its stdout and stderr.

## Build

On Windows with CMake 4.2, use the project build environment:

    $env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
    $env:CMAKE_GENERATOR='Ninja'
    cargo build --release --bins

Start the launcher:

    .\target\release\stm32-launcher.exe

The launcher looks for a sibling stm32-emulator executable first. Use
**Choose emulator** to select a different executable explicitly.

## Catalog updates

The released launcher does not read an epicefi_fw checkout. Update the
checked-in static catalog during development:

    cargo run --release --bin generate-variant-registry -- C:\Users\Tera\Documents\GitHub\epicefi_fw

The generator examines every firmware/config/boards board.yaml, sorts the
result, and gives duplicate source labels a board-qualified ID.

## Running firmware

1. Choose a firmware .bin.
2. Select a cataloged variant.
3. Select its SVD asset when the profile requires one.
4. Inspect **Resolved Configuration**. Run is enabled only after the firmware,
   SVD, and evidence-backed memory map validate.
5. Use **Run emulator** and inspect **Emulator Output**.

proteus_f7 is currently the only partial runnable EpicEFI profile. It uses the
verified STM32F767 code and AXI flash aliases plus the mapped F7 RAM regions.
Its firmware still encounters the separately documented unmodeled FLASH ACR
startup boundary.

Other cataloged variants are intentionally blocked until their MCU, SVD,
memory map, and device requirements are verified. The launcher does not
invent these mappings merely to start a process.

For a non-EpicEFI image, enable **Manual profile** and provide an existing SVD
file plus explicit vector-table, flash, and RAM values. The launcher maps only
those entered regions.
