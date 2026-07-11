# Native Firmware Launcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to execute this plan task-by-task.

**Goal:** Ship a cross-platform native Dear ImGui launcher that selects a firmware file and a checked-in catalog of known EpicEFI variants, validates the selected emulation profile, and streams the existing emulator CLI output.

**Architecture:** A new stm32_emulator::launcher library owns the static variant catalog, profile resolution, YAML generation, validation, and child-process lifecycle. The stm32-launcher binary provides the Dear ImGui desktop UI; it launches the existing emulator CLI as a child rather than duplicating the emulator core. A development-only generator reads epicefi_fw once and writes the checked-in registry source; the released application never needs that source tree.

**Tech Stack:** Rust 2021, imgui 0.12 with docking, imgui-winit-support 0.13, imgui-glium-renderer 0.13, glium 0.35, serde, serde_yaml, tempfile, rfd.

## Constraints

- Every known EpicEFI board variant is selectable from compiled-in data; no runtime epicefi_fw path is used.
- A variant may be selectable but not runnable. Only evidence-backed mappings produce an emulation configuration.
- proteus_f7 is the first partial runnable profile and retains the verified STM32F767 memory map already used by proteus_f7/config.yaml.
- The existing command-line emulator remains functional and is the only process that runs firmware.
- Use standard Rust process and path APIs for all child-process and filesystem boundaries so the launcher builds on Windows, Linux, and macOS.

## File Structure

- Modify: Cargo.toml
- Add: src/lib.rs
- Add: src/launcher/mod.rs — domain types, validation, and temporary YAML resolution
- Add: src/launcher/registry.rs — static catalog access and evidence-backed overrides
- Add: src/launcher/generated_variants.rs — sorted, checked-in generated catalog
- Add: src/launcher/process.rs — emulator command construction and output capture
- Add: src/bin/generate-variant-registry.rs — development registry generator
- Add: src/bin/stm32-launcher.rs — native Dear ImGui application
- Add: docs/native-launcher.md — build, generator, and launcher usage

## Task 1: Add launcher domain types and configuration resolution

**Files:**
- Modify: Cargo.toml
- Add: src/lib.rs
- Add: src/launcher/mod.rs

1. Write tests in src/launcher/mod.rs that assert KnownVariant::proteus_f7() resolves to a profile with vector table 0x0020_0000 and both firmware aliases (0x0020_0000 and 0x0800_0000), and that an unsupported variant cannot resolve to a runnable profile.
2. Run cargo test --lib launcher::tests and confirm it fails because the launcher module does not yet exist.
3. Add src/lib.rs exposing pub mod launcher;, then model EmulationSupport, KnownVariant, MemoryRegion, ResolvedProfile, validation errors, and a serializable YAML DTO in src/launcher/mod.rs.
4. Implement profile resolution and YAML serialization. The resolved configuration must carry absolute firmware and SVD paths supplied by the launcher while preserving the memory-region structure used by config.rs.
5. Run cargo test --lib launcher::tests; then run cargo test to ensure the existing peripheral tests remain green.
6. Commit the isolated domain layer.

## Task 2: Generate and check in the static EpicEFI variant registry

**Files:**
- Add: src/bin/generate-variant-registry.rs
- Add: src/launcher/generated_variants.rs
- Add: src/launcher/registry.rs
- Modify: src/launcher/mod.rs

1. Write registry tests asserting IDs are unique, entries are sorted by ID, the catalog includes proteus_f7, and no unsupported entry resolves into a runnable profile.
2. Run the focused registry tests and confirm the missing catalog causes failure.
3. Build the development-only generator. It reads firmware/config/boards/**/board.yaml, derives a stable identifier and display metadata, sorts the result, and emits Rust source into src/launcher/generated_variants.rs.
4. Generate the catalog from C:\Users\Tera\Documents\GitHub\epicefi_fw. Check the generated file into this repository. Variants default to Unsupported until a reviewed override provides a known mapping.
5. Add a reviewed proteus_f7 override in registry.rs:

       MCU: STM32F767
       SVD: STM32F767.svd
       Vector table: 0x0020_0000
       Firmware ITCM alias: 0x0020_0000, size 0x0020_0000
       Firmware AXI alias:  0x0800_0000, size 0x0020_0000
       ITCM RAM:            0x0000_0000, size 0x0000_4000
       DTCM RAM:            0x2000_0000, size 0x0002_0000
       SRAM1:               0x2002_0000, size 0x0005_c000
       SRAM2:               0x2007_c000, size 0x0000_4000

6. Run the focused tests, cargo test, and the generator a second time; verify git diff --exit-code -- src/launcher/generated_variants.rs so generation is deterministic.
7. Commit the generated catalog, generator, and reviewed override together.

## Task 3: Add child-process execution and output capture

**Files:**
- Add: src/launcher/process.rs
- Modify: src/launcher/mod.rs
- Modify: Cargo.toml

1. Write tests for process argument construction. For example, resolving resolved.yaml at verbosity one must create [resolved.yaml, -v]; a nonexistent firmware path must fail validation before a child is started.
2. Run the process tests and confirm they fail before implementation.
3. Implement build_emulator_arguments, a RunningEmulator type, and a temporary-config holder using tempfile. Spawn the existing emulator executable with piped stdout and stderr, collect lines on reader threads into a bounded VecDeque, and expose non-blocking polling plus explicit termination.
4. Keep emulator executable discovery explicit: use a caller-provided path first, then a sibling executable where available, and surface a clear actionable error if neither is usable. Do not silently invoke an unrelated shell command.
5. Run the focused tests and cargo test.
6. Commit the process layer.

## Task 4: Build the dockable native Dear ImGui launcher

**Files:**
- Modify: Cargo.toml
- Add: src/bin/stm32-launcher.rs
- Modify: src/launcher/mod.rs
- Add: docs/native-launcher.md

1. Add a focused state test that a default LauncherState has no firmware selected, no child process, and a disabled Run action.
2. Run the focused test and confirm it fails before UI state exists.
3. Add the GUI dependencies:

       imgui = { version = "0.12", features = ["docking"] }
       imgui-winit-support = "0.13"
       imgui-glium-renderer = "0.13"
       glium = { version = "0.35", features = ["glutin_backend", "simple_window_builder"] }
       rfd = "0.15"
       tempfile = "3"

4. Create a winit/glium window, attach Dear ImGui, and set imgui::ConfigFlags::DOCKING_ENABLE before rendering frames.
5. Render four dockable panels:

   - Firmware & Variant: native firmware-file picker, searchable static variant selector, selected MCU family, and support state.
   - Resolved Configuration: evidence-backed mapping summary and read-only generated YAML preview; show the manual-profile form only when selected.
   - Hardware Notes: known/unknown peripheral-model notes and links to the profile evidence held in source comments/docs.
   - Emulator Output: stdout/stderr lines, process status, and Start/Stop controls.

6. Disable Run until the profile and firmware validate. For unsupported static variants, explain that the variant is cataloged but has no verified emulation map. Do not manufacture an SVD, memory map, or device model.
7. On Run, write the resolved YAML into a temporary directory and invoke the process layer. On Stop and application exit, terminate only the launcher-owned child and join or release its output readers.
8. Document the normal commands:

       cargo run --release --bin generate-variant-registry -- C:\Users\Tera\Documents\GitHub\epicefi_fw
       cargo run --release --bin stm32-launcher

9. Run cargo test, cargo build --release --bin stm32-launcher, and manually open the launcher. Confirm the catalog lists Proteus variants, proteus_f7 accepts a valid firmware file, and unsupported variants remain blocked with clear status.
10. Commit the launcher UI and user documentation.

## Task 5: Full verification and handoff

**Files:**
- Review all files above

1. Run cargo fmt --check and format only the files introduced by this feature if required.
2. Run cargo test and cargo build --release --bin stm32-launcher from the repository root.
3. Regenerate the catalog from the known EpicEFI source and verify it produces no diff.
4. Start the release launcher manually and inspect the four-panel layout, firmware picker, static catalog, validation state, and child-output panel.
5. Record the verification results in the handoff with any remaining emulator-model limitations, particularly that cataloging a variant is not a claim that its firmware runs.
