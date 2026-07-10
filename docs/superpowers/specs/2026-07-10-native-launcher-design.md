# Native Firmware Launcher Design

## Goal

Provide a cross-platform native Rust launcher for STM32 Emulator that lets a
user select a firmware image and a compiled-in, known epicefi board variant,
inspect the resolved configuration, and run the existing emulator while
watching its trace.

## Scope

The launcher is a separate executable built with the existing CLI emulator.
It is written in Rust using Dear ImGui with a native window/render backend and
does not require an `epicefi_fw` checkout at runtime.

The launcher contains a checked-in registry of every known epicefi board
variant. The registry is generated or updated by developers from the source
repository, then committed as Rust data. It is not dynamically scanned by the
released application.

## Variant Registry

Each `KnownVariant` records:

- Stable variant identifier and display name.
- MCU family and exact MCU model when established.
- Firmware flash load addresses, vector-table address, and RAM regions.
- Expected SVD asset and known external-device requirements.
- `EmulationSupport` state: `Runnable`, `Partial`, or `Unsupported`.
- A concise explanation of unsupported or partial behavior.

All discovered epicefi variants are selectable. A variant is runnable only if
its profile contains evidence-backed memory and SVD information. The launcher
must never synthesize a board mapping merely to enable the Run button.

Proteus F7 is the first `Partial` runnable variant. Its profile uses
STM32F767, both flash aliases at `0x00200000` and `0x08000000`, its mapped
RAM regions, and the local F767 SVD setup already established by this project.

## Workspace

The Dear ImGui application has four dockable panels:

1. **Firmware & Variant** — select a `.bin`, choose a known variant, and
   view its support state.
2. **Resolved Configuration** — display generated YAML, flash/RAM mappings,
   SVD path, and validation errors.
3. **Hardware Notes** — display the variant's connected-device expectations
   and current emulator limitations.
4. **Emulator Output** — start and stop the CLI child process and stream its
   stdout/stderr with bounded in-memory retention.

Manual configuration is available for non-epicefi images. It requires explicit
MCU/SVD, vector-table, flash, and RAM values before it becomes runnable.

## Process and Error Handling

The launcher writes a temporary YAML configuration and invokes the existing
CLI emulator as a child process. It captures the process output without
embedding a second emulator implementation.

The Run action remains disabled when the selected firmware, SVD, profile, or
required memory mapping is invalid. Child-process failure is shown in the
output panel with its exit code. Stop terminates only the launcher-owned child
process.

## Validation

Automated tests cover variant lookup, duplicate IDs, profile-to-YAML
generation, validation states, and CLI argument construction. A manual smoke
test verifies that the native window starts, renders all four panels, launches
the Proteus F7 configuration, and displays its trace.

## Cross-Platform Boundary

UI rendering and profile resolution remain platform-neutral. Process spawning,
temporary files, file dialogs, and termination are isolated behind small Rust
interfaces with Windows, macOS, and Linux implementations where required.
