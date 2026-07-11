# Launcher Workspace Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the native launcher workspace across runs, emit valid CPU-model YAML, and suppress Windows console windows for both launcher and emulator child.

**Architecture:** A reusable launcher workspace module owns YAML serialization and the user-state paths. The binary restores this snapshot before building the winit window, routes ImGui to the same app-data directory, and saves state on UI/window changes and close. Profile CPU model flows from a resolved profile into YAML; Windows-only process flags hide console windows without changing captured output.

**Tech Stack:** Rust 2021, serde_yaml, Dear ImGui 0.12, winit 0.30, glium 0.35, standard library Windows CommandExt.

## Global Constraints

- Store state under LOCALAPPDATA/stm32-emulator/launcher on Windows; use XDG_STATE_HOME or HOME/.local/state on non-Windows.
- Keep ImGui layout in imgui.ini and application state in workspace.yaml.
- Restore files and form values, never a running emulator process or captured output.
- Every emitted emulator YAML CPU section contains cortex-m4 or cortex-m7.
- The Windows GUI subsystem and CREATE_NO_WINDOW use cfg(windows); non-Windows behavior remains unchanged.
- Do not add a registry dependency, cloud state, or runtime EpicEFI source checkout.

---

## File Structure

- Create src/launcher/workspace.rs: serializable workspace snapshots, user-state directory selection, YAML load/save, and tests.
- Modify src/launcher/mod.rs: add LauncherCpuModel to ResolvedProfile and emitted YAML.
- Modify src/launcher/ui_state.rs: define serializable manual-profile data restored by the binary.
- Modify src/launcher/process.rs: configure Windows child creation without a console.
- Modify src/bin/stm32-launcher.rs: GUI subsystem, workspace restoration/saving, ImGui INI location, native window placement, and manual CPU selector.
- Modify tests/launcher_profile.rs and create tests/launcher_workspace.rs.

### Task 1: Make CPU model an explicit launcher-profile value

**Files:**
- Modify: src/launcher/mod.rs
- Modify: tests/launcher_profile.rs

**Interfaces:**
- Produces: LauncherCpuModel::{CortexM4, CortexM7}, ResolvedProfile { cpu_model: LauncherCpuModel, .. }, and YAML cpu.model.

- [ ] **Step 1: Write failing profile tests**

```rust
#[test]
fn proteus_f7_yaml_selects_cortex_m7() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    ).unwrap();

    assert!(profile.to_yaml().unwrap().contains("model: cortex-m7"));
}

#[test]
fn manual_profile_yaml_uses_the_selected_cpu_model() {
    let profile = ResolvedProfile::manual(
        LauncherCpuModel::CortexM4, PathBuf::from("firmware.bin"),
        PathBuf::from("chip.svd"), 0x0800_0000, 0x0800_0000,
        0x0010_0000, 0x2000_0000, 0x0002_0000,
    );

    assert!(profile.to_yaml().unwrap().contains("model: cortex-m4"));
}
```

- [ ] **Step 2: Run the focused tests before implementation**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test launcher_profile
```

Expected: compilation fails because LauncherCpuModel and the manual CPU parameter do not exist.

- [ ] **Step 3: Implement model propagation**

```rust
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherCpuModel { CortexM4, CortexM7 }

pub struct ResolvedProfile {
    pub cpu_model: LauncherCpuModel,
    // retain existing fields
}

struct ProfileTemplate {
    cpu_model: LauncherCpuModel,
    vector_table: u32,
    regions: &'static [MemoryRegion],
}

struct YamlCpu<'a> {
    model: LauncherCpuModel,
    svd: Cow<'a, str>,
    vector_table: u32,
}
```

Set PROTEUS_F7_PROFILE.cpu_model to CortexM7. Make ResolvedProfile::manual accept the explicit model and copy it to the YAML structure.

- [ ] **Step 4: Re-run focused tests**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test launcher_profile
```

Expected: profile tests pass and generated YAML includes the required model field.

- [ ] **Step 5: Commit CPU-model profile repair**

```powershell
git add src/launcher/mod.rs tests/launcher_profile.rs
git commit -m "fix: emit launcher CPU model"
```

### Task 2: Add a durable launcher workspace store

**Files:**
- Create: src/launcher/workspace.rs
- Modify: src/launcher/mod.rs
- Modify: src/launcher/ui_state.rs
- Create: tests/launcher_workspace.rs

**Interfaces:**
- Produces: WorkspacePaths { directory, imgui_ini, workspace_yaml }, Workspace, WindowPlacement, ManualProfileState, and WorkspaceStore::{load, save}.

- [ ] **Step 1: Write failing workspace tests**

```rust
#[test]
fn workspace_round_trips_loaded_files_form_and_window_placement() {
    let root = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::in_directory(root.path()).unwrap();
    let expected = Workspace {
        window: Some(WindowPlacement { x: 120, y: 90, width: 1500, height: 920 }),
        state: SavedLauncherState {
            firmware: Some(PathBuf::from("firmware.bin")),
            svd: Some(PathBuf::from("chip.svd")),
            emulator_executable: Some(PathBuf::from("stm32-emulator.exe")),
            selected_variant: Some("proteus_f7".to_owned()),
            filter: "proteus".to_owned(),
            manual: ManualProfileState::default(),
        },
    };
    store.save(&expected).unwrap();
    assert_eq!(store.load().unwrap(), expected);
}

#[test]
fn malformed_workspace_falls_back_to_default() {
    let root = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::in_directory(root.path()).unwrap();
    std::fs::write(store.workspace_path(), "not: [valid").unwrap();
    assert_eq!(store.load().unwrap(), Workspace::default());
}
```

- [ ] **Step 2: Run the tests before implementation**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test launcher_workspace
```

Expected: compilation fails because workspace types and store do not exist.

- [ ] **Step 3: Implement workspace serialization and paths**

```rust
pub struct WorkspaceStore { paths: WorkspacePaths }

impl WorkspaceStore {
    pub fn in_directory(directory: impl Into<PathBuf>) -> Result<Self, WorkspaceError>;
    pub fn for_current_user() -> Result<Self, WorkspaceError>;
    pub fn load(&self) -> Result<Workspace, WorkspaceError>;
    pub fn save(&self, workspace: &Workspace) -> Result<(), WorkspaceError>;
    pub fn imgui_ini_path(&self) -> &Path;
    pub fn workspace_path(&self) -> &Path;
}
```

Use serde_yaml for Workspace. Create the directory before saving. Return Workspace::default only for a missing or malformed YAML file; return an error for directory or write failures. Put ManualProfileState in ui_state.rs with a LauncherCpuModel field and derive Serialize, Deserialize, Clone, Debug, Default, and PartialEq.

- [ ] **Step 4: Run workspace tests**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test launcher_workspace
```

Expected: both YAML round-trip and malformed-state fallback tests pass.

- [ ] **Step 5: Commit workspace library**

```powershell
git add src/launcher/workspace.rs src/launcher/ui_state.rs src/launcher/mod.rs tests/launcher_workspace.rs
git commit -m "feat: persist launcher workspace"
```

### Task 3: Restore the workspace in ImGui and hide Windows consoles

**Files:**
- Modify: src/bin/stm32-launcher.rs
- Modify: src/launcher/process.rs
- Modify: docs/native-launcher.md

**Interfaces:**
- Consumes: WorkspaceStore and Workspace from Task 2 plus LauncherCpuModel from Task 1.
- Produces: restored native window attributes, a fixed ImGui INI path, saved launcher inputs, and hidden Windows child creation.

- [ ] **Step 1: Write failing GUI-independent helper tests**

```rust
#[test]
fn placement_uses_default_when_saved_dimensions_are_zero() {
    assert_eq!(
        normalized_placement(Some(WindowPlacement { x: 1, y: 1, width: 0, height: 0 })),
        None,
    );
}
```

Place this test in a cfg(test) module beside the pure placement-normalization helper in stm32-launcher.rs.

- [ ] **Step 2: Run the helper test before implementation**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test placement_uses_default_when_saved_dimensions_are_zero
```

Expected: compilation fails because normalized_placement does not exist.

- [ ] **Step 3: Wire persistence and no-console behavior**

```rust
#![cfg_attr(windows, windows_subsystem = "windows")]

let store = WorkspaceStore::for_current_user().expect("launcher workspace directory");
let workspace = store.load().unwrap_or_default();
let attributes = glium::winit::window::Window::default_attributes()
    .with_title("STM32 Emulator — Firmware Launcher")
    .with_inner_size(PhysicalSize::new(width, height))
    .with_position(PhysicalPosition::new(x, y));
let builder = SimpleWindowBuilder::new().set_window_builder(attributes);

#[cfg(windows)]
{
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
```

Build window attributes from a normalized saved placement, otherwise use 1440x900 with no explicit position. Initialize ImGui with store.imgui_ini_path(). Restore App fields from Workspace.state. Add a Manual CPU Model combo with Cortex-M4 and Cortex-M7 values. Record Resized and Moved events, detect changed saved state after each redraw, debounce YAML saves, and force a final save plus imgui.save_ini_settings on CloseRequested. In RunningEmulator::spawn, create a mutable Command, set arguments/pipes, apply the cfg(windows) creation flag, then spawn it.

- [ ] **Step 4: Build and run tests**

```powershell
$env:CMAKE_POLICY_VERSION_MINIMUM='3.5'; $env:CMAKE_GENERATOR='Ninja'; $env:CARGO_TARGET_DIR=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'stm32-emulator-proteus-f7-target'; cargo test; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; cargo build --release --bin stm32-launcher
```

Expected: all tests pass and the Windows GUI launcher builds.

- [ ] **Step 5: Windows smoke test and documentation**

Launch the release stm32-launcher.exe directly, choose a firmware/profile, move and resize the app, change panel placement, close, and relaunch. Confirm the app restores the workspace; use Run emulator and confirm its output appears in the ImGui panel without either a launcher or child console window. Update docs/native-launcher.md with the state directory and restoration behavior.

- [ ] **Step 6: Commit integration**

```powershell
git add src/bin/stm32-launcher.rs src/launcher/process.rs docs/native-launcher.md
git commit -m "feat: restore launcher workspace without console windows"
```

## Final Verification

- [ ] Run cargo test with the required CMake and Ninja environment.
- [ ] Run cargo build --release --bins with the same environment.
- [ ] Launch stm32-launcher.exe twice to verify restored files, native placement, and ImGui docking.
- [ ] Start and stop a child emulator from the UI to confirm captured output without console windows.
- [ ] Confirm git diff --check is clean.
