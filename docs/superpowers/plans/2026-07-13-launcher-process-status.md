# Launcher Process Status Indicator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the native launcher GUI's binary "Running"/"Idle" text with a four-state `Idle`/`Running`/`Halted`/`Crashed` status, shown both next to the Run/Stop buttons and in the always-visible Signal Chain bar, so the user never has to infer whether the emulator child process is alive from Task Manager or log silence.

**Architecture:** `RunningEmulator::is_running() -> Result<bool, _>` (`src/launcher/process.rs`) becomes `poll_state() -> Result<ProcessState, _>`, surfacing the `ExitStatus` success bit that `Child::try_wait()` already returns today but currently discards. `LauncherState::running: bool` (`src/launcher/ui_state.rs`) becomes `LauncherState::status: RunStatus`, a single source of truth with no parallel bool to drift out of sync. `src/bin/stm32-launcher.rs` wires the two together: `start()`/`stop()`/`refresh_process()` drive the transitions, and one shared `status_color()`/`status_label()` pair renders both display spots identically.

**Tech Stack:** Rust, `std::process::{Child, ExitStatus}`, `imgui` (`Ui::text_colored`).

## Global Constraints

- `RunStatus` is the single source of truth for run state in `LauncherState` — no parallel `bool` field is kept alongside it.
- Manual **Stop** always yields `RunStatus::Halted`, regardless of the resulting exit code. `Child::kill()` uses `TerminateProcess` on Windows, which reports a non-zero exit code even for a deliberate stop — "the user asked it to stop" must override the exit-code check.
- A non-zero exit code detected automatically (the process exited on its own, not via manual Stop) yields `RunStatus::Crashed` and sets `LauncherState::last_error` to `"Emulator exited with a non-zero status."` so the existing red error line under the Run/Stop buttons explains why.
- Color mapping (`Idle`→gray, `Running`→cyan, `Halted`→amber, `Crashed`→red) is defined in exactly one place (`status_color`) and reused by both display spots (Emulator Output panel status text, Signal Chain step-4 indicator), so they cannot disagree with each other.
- `RunningEmulator`/`App`'s process-spawning and GUI behavior have no existing test coverage today, and this plan does not change that pattern. New tests target only the pure logic introduced: the exit-code classification, `RunStatus`/`can_run()`, and the color/label mapping.

---

### Task 1: `ProcessState` — surface the child process's real exit status

**Ground truth used by this task:** `src/launcher/process.rs:144-146` currently has:

```rust
pub fn is_running(&mut self) -> Result<bool, ProcessError> {
    Ok(self.child.try_wait().map_err(ProcessError::io)?.is_none())
}
```

`Child::try_wait()` (`std::process::Child`) returns `Ok(None)` while the process is still running, and `Ok(Some(ExitStatus))` once it has exited — `ExitStatus::success()` is `true` only for a zero exit code. This task stops throwing that status away.

**Files:**
- Modify: `src/launcher/process.rs:144-146`

**Interfaces:**
- Produces `ProcessState` (`#[derive(Clone, Copy, Debug, Eq, PartialEq)]`, variants `Running` and `Exited { success: bool }`) — Task 3 matches on this in `App::refresh_process()`.
- Produces `RunningEmulator::poll_state(&mut self) -> Result<ProcessState, ProcessError>`, replacing `is_running`. Same error type, same `&mut self` receiver, same call site pattern as the method it replaces.
- Consumes: `std::process::{Child, ExitStatus}` (already used in this file), `ProcessError` (already defined in this file).

- [ ] **Step 1: Write the failing tests**

    Add to the bottom of `src/launcher/process.rs` (this file has no existing `#[cfg(test)]` block):

    ```rust
    #[cfg(all(test, windows))]
    mod tests {
        use std::os::windows::process::ExitStatusExt;
        use std::process::ExitStatus;

        use super::{classify_exit, ProcessState};

        #[test]
        fn a_zero_exit_code_is_classified_as_a_successful_exit() {
            assert_eq!(
                classify_exit(ExitStatus::from_raw(0)),
                ProcessState::Exited { success: true }
            );
        }

        #[test]
        fn a_nonzero_exit_code_is_classified_as_a_failed_exit() {
            assert_eq!(
                classify_exit(ExitStatus::from_raw(1)),
                ProcessState::Exited { success: false }
            );
        }
    }
    ```

    `ExitStatusExt::from_raw` (`std::os::windows::process::ExitStatusExt`) constructs a real `ExitStatus` directly from a raw code, so this test needs no actual child process. The module is gated `#[cfg(all(test, windows))]` because `from_raw` is Windows-only — matching the existing `#[cfg(windows)]` precedent already in this file (`RunningEmulator::spawn`'s `creation_flags` call at line 109-114).

- [ ] **Step 2: Run tests to verify they fail**

    The launcher lives in the library target (`src/lib.rs` exposes `pub mod launcher;`), so its tests run via `--lib`, not `--bin stm32-emulator`.

    Run: `cargo test --lib process::tests`
    Expected: FAIL to compile — `cannot find type ProcessState in this scope`, `cannot find function classify_exit in this scope`.

- [ ] **Step 3: Implement `ProcessState`, `classify_exit`, and `poll_state`**

    In `src/launcher/process.rs`, add just above `pub struct RunningEmulator {` (currently line 83):

    ```rust
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ProcessState {
        Running,
        Exited { success: bool },
    }

    fn classify_exit(status: std::process::ExitStatus) -> ProcessState {
        ProcessState::Exited { success: status.success() }
    }

    ```

    Then replace the existing `is_running` method (lines 144-146) with:

    ```rust
    pub fn poll_state(&mut self) -> Result<ProcessState, ProcessError> {
        match self.child.try_wait().map_err(ProcessError::io)? {
            None => Ok(ProcessState::Running),
            Some(status) => Ok(classify_exit(status)),
        }
    }
    ```

- [ ] **Step 4: Run tests to verify they pass**

    Run: `cargo test --lib process::tests`
    Expected: `test launcher::process::tests::a_zero_exit_code_is_classified_as_a_successful_exit ... ok` and `test launcher::process::tests::a_nonzero_exit_code_is_classified_as_a_failed_exit ... ok`, 2 passed.

    Also confirm the full library still builds (Task 3 will fix the now-broken call site in `stm32-launcher.rs`; that binary is expected to fail to compile until Task 3 lands — this is fine, it isn't part of the `stm32-emulator` library target):

    Run: `cargo build --lib`
    Expected: success.

- [ ] **Step 5: Commit**

    ```bash
    git add src/launcher/process.rs
    git commit -m "feat: track the emulator child process's real exit status"
    ```

---

### Task 2: `RunStatus` — replace the launcher's `running: bool`

**Ground truth used by this task:** `src/launcher/ui_state.rs` (full file, 19 lines):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct LauncherState {
    pub firmware: Option<PathBuf>,
    pub svd: Option<PathBuf>,
    pub emulator_executable: Option<PathBuf>,
    pub selected_variant: Option<String>,
    pub running: bool,
    pub last_error: Option<String>,
}

impl LauncherState {
    pub fn can_run(&self) -> bool {
        self.firmware.is_some() && self.selected_variant.is_some() && !self.running
    }
}
```

`tests/launcher_state.rs` (full file, 13 lines) is the existing integration test for this struct:

```rust
use stm32_emulator::launcher::ui_state::LauncherState;

#[test]
fn default_launcher_state_has_no_selection_or_runnable_process() {
    let state = LauncherState::default();

    assert!(state.firmware.is_none());
    assert!(state.svd.is_none());
    assert!(state.selected_variant.is_none());
    assert!(!state.running);
    assert!(!state.can_run());
}
```

`src/bin/stm32-launcher.rs:53-59`'s `App::new` constructs `LauncherState` using struct-update syntax (`..Default::default()`), so it does not name `running` explicitly and needs no change for this task:

```rust
state: LauncherState {
    firmware: saved.firmware,
    svd: saved.svd,
    emulator_executable: saved.emulator_executable,
    selected_variant: saved.selected_variant,
    ..Default::default()
},
```

**Files:**
- Modify: `src/launcher/ui_state.rs`
- Test: `tests/launcher_state.rs`

**Interfaces:**
- Produces `RunStatus` (`#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]`, variants `Idle` (`#[default]`), `Running`, `Halted`, `Crashed`) — Task 3 sets this field and reads it for both display spots.
- Produces `LauncherState::status: RunStatus`, replacing the `running: bool` field.
- Consumes: nothing new.

- [ ] **Step 1: Write the failing tests**

    Replace `tests/launcher_state.rs` in full:

    ```rust
    use stm32_emulator::launcher::ui_state::{LauncherState, RunStatus};

    #[test]
    fn default_launcher_state_has_no_selection_or_runnable_process() {
        let state = LauncherState::default();

        assert!(state.firmware.is_none());
        assert!(state.svd.is_none());
        assert!(state.selected_variant.is_none());
        assert_eq!(state.status, RunStatus::Idle);
        assert!(!state.can_run());
    }

    fn state_with_selection(status: RunStatus) -> LauncherState {
        LauncherState {
            firmware: Some(std::path::PathBuf::from("firmware.bin")),
            selected_variant: Some("proteus_f7".to_owned()),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn can_run_is_true_while_idle_halted_or_crashed_once_firmware_and_variant_are_selected() {
        assert!(state_with_selection(RunStatus::Idle).can_run());
        assert!(state_with_selection(RunStatus::Halted).can_run());
        assert!(state_with_selection(RunStatus::Crashed).can_run());
    }

    #[test]
    fn can_run_is_false_while_running() {
        assert!(!state_with_selection(RunStatus::Running).can_run());
    }
    ```

- [ ] **Step 2: Run tests to verify they fail**

    Run: `cargo test --test launcher_state`
    Expected: FAIL to compile — `unresolved import stm32_emulator::launcher::ui_state::RunStatus`, `no field status on type LauncherState`.

- [ ] **Step 3: Implement `RunStatus` and update `LauncherState`**

    Replace `src/launcher/ui_state.rs` in full:

    ```rust
    // SPDX-License-Identifier: GPL-3.0-or-later

    use std::path::PathBuf;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub enum RunStatus {
        #[default]
        Idle,
        Running,
        Halted,
        Crashed,
    }

    #[derive(Debug, Default)]
    pub struct LauncherState {
        pub firmware: Option<PathBuf>,
        pub svd: Option<PathBuf>,
        pub emulator_executable: Option<PathBuf>,
        pub selected_variant: Option<String>,
        pub status: RunStatus,
        pub last_error: Option<String>,
    }

    impl LauncherState {
        pub fn can_run(&self) -> bool {
            self.firmware.is_some() && self.selected_variant.is_some() && self.status != RunStatus::Running
        }
    }
    ```

- [ ] **Step 4: Run tests to verify they pass**

    Run: `cargo test --test launcher_state`
    Expected: 3 passed (`default_launcher_state_has_no_selection_or_runnable_process`, `can_run_is_true_while_idle_halted_or_crashed_once_firmware_and_variant_are_selected`, `can_run_is_false_while_running`).

    `src/bin/stm32-launcher.rs` is expected to still fail to compile at this point (it references the now-removed `state.running` field) — Task 3 fixes this. Confirm the library itself and the other launcher integration tests are unaffected:

    Run: `cargo test --lib && cargo test --test launcher_profile --test launcher_registry --test launcher_workspace`
    Expected: all passing, 0 failed.

- [ ] **Step 5: Commit**

    ```bash
    git add src/launcher/ui_state.rs tests/launcher_state.rs
    git commit -m "feat: add RunStatus to replace the launcher's running bool"
    ```

---

### Task 3: Wire `RunStatus` into the launcher GUI

**Ground truth used by this task:** exact current contents of the pieces of `src/bin/stm32-launcher.rs` this task touches.

Imports, `src/bin/stm32-launcher.rs:11-21`:

```rust
use stm32_emulator::launcher::process::{
    discover_emulator, validate_firmware, OutputStream, RunningEmulator, TemporaryConfig,
};
use stm32_emulator::launcher::registry::{all_variants, support_summary};
use stm32_emulator::launcher::ui_state::LauncherState;
use stm32_emulator::launcher::workspace::{
    SavedLauncherState, WindowPlacement, WorkspaceStore,
};
use stm32_emulator::launcher::{
    EmulationSupport, KnownVariant, LauncherCpuModel, ResolvedProfile,
};
```

Color constants, `src/bin/stm32-launcher.rs:23-27`:

```rust
const BG: [f32; 4] = [0.086, 0.106, 0.133, 1.0];
const PANEL: [f32; 4] = [0.133, 0.165, 0.208, 1.0];
const AMBER: [f32; 4] = [0.949, 0.722, 0.294, 1.0];
const CYAN: [f32; 4] = [0.314, 0.769, 0.827, 1.0];
const RED: [f32; 4] = [0.878, 0.424, 0.459, 1.0];
```

`start`/`stop`/`refresh_process`, `src/bin/stm32-launcher.rs:162-205`:

```rust
fn start(&mut self) {
    let result = (|| {
        let profile = self.resolved_profile()?;
        let yaml = profile.to_yaml().map_err(|error| error.to_string())?;
        let temporary_config =
            TemporaryConfig::write(&yaml).map_err(|error| error.to_string())?;
        let executable =
            discover_emulator(self.state.emulator_executable.as_deref()).map_err(|error| {
                format!("{error}. Use “Choose emulator” to select it explicitly.")
            })?;
        let process = RunningEmulator::spawn(&executable, temporary_config.path(), 1)
            .map_err(|error| error.to_string())?;
        self.temporary_config = Some(temporary_config);
        self.process = Some(process);
        self.state.running = true;
        Ok::<(), String>(())
    })();
    self.state.last_error = result.err();
}

fn stop(&mut self) {
    if let Some(process) = self.process.as_mut() {
        if let Err(error) = process.stop() {
            self.state.last_error = Some(error.to_string());
        }
    }
    self.process = None;
    self.temporary_config = None;
    self.state.running = false;
}

fn refresh_process(&mut self) {
    if let Some(process) = self.process.as_mut() {
        process.poll_output();
        match process.is_running() {
            Ok(true) => {}
            Ok(false) => self.state.running = false,
            Err(error) => {
                self.state.last_error = Some(error.to_string());
                self.state.running = false;
            }
        }
    }
}
```

`draw_signal_chain`/`indicator`, `src/bin/stm32-launcher.rs:341-367`:

```rust
fn draw_signal_chain(ui: &Ui, app: &App) {
    ui.window("Signal Chain")
        .position([12.0, 12.0], Condition::FirstUseEver)
        .size([640.0, 66.0], Condition::FirstUseEver)
        .build(|| {
            let firmware = app.state.firmware.is_some();
            let variant = app.manual.enabled || app.selected_variant().is_some();
            let profile = app.resolved_profile().is_ok();
            indicator(ui, "1  Firmware", firmware);
            ui.same_line();
            ui.text("→");
            ui.same_line();
            indicator(ui, "2  Variant", variant);
            ui.same_line();
            ui.text("→");
            ui.same_line();
            indicator(ui, "3  Profile", profile);
            ui.same_line();
            ui.text("→");
            ui.same_line();
            indicator(ui, "4  Emulator", app.state.running);
        });
}

fn indicator(ui: &Ui, label: &str, active: bool) {
    ui.text_colored(if active { CYAN } else { AMBER }, label);
}
```

Status text in `draw_output_panel`, `src/bin/stm32-launcher.rs:548-552`:

```rust
            ui.same_line();
            ui.text_colored(
                if app.state.running { CYAN } else { AMBER },
                if app.state.running { "Running" } else { "Idle" },
            );
```

**Files:**
- Modify: `src/bin/stm32-launcher.rs`

**Interfaces:**
- Consumes `ProcessState` (Task 1: `Running`, `Exited { success: bool }`) and `RunningEmulator::poll_state` (Task 1).
- Consumes `RunStatus` (Task 2: `Idle`, `Running`, `Halted`, `Crashed`) and `LauncherState::status` (Task 2).
- Produces `status_color(status: RunStatus) -> [f32; 4]` and `status_label(status: RunStatus) -> &'static str` — the single shared mapping used by both display spots.
- Produces `emulator_step_indicator(ui: &Ui, status: RunStatus)` for the Signal Chain bar's step 4, alongside the unmodified `indicator(ui, label, bool)` used by steps 1-3.

- [ ] **Step 1: Update imports and add the `GRAY` color constant**

    Replace `src/bin/stm32-launcher.rs:11-21` (the two `use stm32_emulator::launcher::...` lines that change):

    ```rust
    use stm32_emulator::launcher::process::{
        discover_emulator, validate_firmware, OutputStream, ProcessState, RunningEmulator,
        TemporaryConfig,
    };
    use stm32_emulator::launcher::registry::{all_variants, support_summary};
    use stm32_emulator::launcher::ui_state::{LauncherState, RunStatus};
    use stm32_emulator::launcher::workspace::{
        SavedLauncherState, WindowPlacement, WorkspaceStore,
    };
    use stm32_emulator::launcher::{
        EmulationSupport, KnownVariant, LauncherCpuModel, ResolvedProfile,
    };
    ```

    Add a fourth color constant after `src/bin/stm32-launcher.rs:27` (`const RED: ...`):

    ```rust
    const GRAY: [f32; 4] = [0.6, 0.6, 0.6, 1.0];
    ```

- [ ] **Step 2: Update `start`, `stop`, and `refresh_process`**

    In `start` (`src/bin/stm32-launcher.rs:176`), replace:

    ```rust
            self.state.running = true;
    ```

    with:

    ```rust
            self.state.status = RunStatus::Running;
    ```

    Replace `stop` (`src/bin/stm32-launcher.rs:182-191`) in full:

    ```rust
    fn stop(&mut self) {
        if let Some(process) = self.process.as_mut() {
            if let Err(error) = process.stop() {
                self.state.last_error = Some(error.to_string());
            }
        }
        self.process = None;
        self.temporary_config = None;
        self.state.status = RunStatus::Halted;
    }
    ```

    Replace `refresh_process` (`src/bin/stm32-launcher.rs:193-205`) in full:

    ```rust
    fn refresh_process(&mut self) {
        if let Some(process) = self.process.as_mut() {
            process.poll_output();
            match process.poll_state() {
                Ok(ProcessState::Running) => {}
                Ok(ProcessState::Exited { success: true }) => {
                    self.state.status = RunStatus::Halted;
                }
                Ok(ProcessState::Exited { success: false }) => {
                    self.state.status = RunStatus::Crashed;
                    self.state.last_error =
                        Some("Emulator exited with a non-zero status.".to_owned());
                }
                Err(error) => {
                    self.state.last_error = Some(error.to_string());
                    self.state.status = RunStatus::Crashed;
                }
            }
        }
    }
    ```

- [ ] **Step 3: Add the shared status mapping and the Signal Chain step-4 indicator**

    Add after `indicator` (`src/bin/stm32-launcher.rs:365-367`):

    ```rust
    fn status_color(status: RunStatus) -> [f32; 4] {
        match status {
            RunStatus::Idle => GRAY,
            RunStatus::Running => CYAN,
            RunStatus::Halted => AMBER,
            RunStatus::Crashed => RED,
        }
    }

    fn status_label(status: RunStatus) -> &'static str {
        match status {
            RunStatus::Idle => "Idle",
            RunStatus::Running => "Running",
            RunStatus::Halted => "Halted",
            RunStatus::Crashed => "Crashed",
        }
    }

    fn emulator_step_indicator(ui: &Ui, status: RunStatus) {
        ui.text_colored(status_color(status), "4  Emulator");
    }
    ```

    In `draw_signal_chain` (`src/bin/stm32-launcher.rs:361`), replace:

    ```rust
            indicator(ui, "4  Emulator", app.state.running);
    ```

    with:

    ```rust
            emulator_step_indicator(ui, app.state.status);
    ```

- [ ] **Step 4: Update the Emulator Output panel's status text**

    In `draw_output_panel` (`src/bin/stm32-launcher.rs:549-552`), replace:

    ```rust
            ui.text_colored(
                if app.state.running { CYAN } else { AMBER },
                if app.state.running { "Running" } else { "Idle" },
            );
    ```

    with:

    ```rust
            ui.text_colored(status_color(app.state.status), status_label(app.state.status));
    ```

- [ ] **Step 5: Add unit tests locking in the status mapping**

    Append to the end of `src/bin/stm32-launcher.rs` (595 lines today; this file has no existing `#[cfg(test)]` block):

    ```rust
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn each_run_status_maps_to_a_distinct_color() {
            assert_eq!(status_color(RunStatus::Idle), GRAY);
            assert_eq!(status_color(RunStatus::Running), CYAN);
            assert_eq!(status_color(RunStatus::Halted), AMBER);
            assert_eq!(status_color(RunStatus::Crashed), RED);
        }

        #[test]
        fn each_run_status_has_a_label() {
            assert_eq!(status_label(RunStatus::Idle), "Idle");
            assert_eq!(status_label(RunStatus::Running), "Running");
            assert_eq!(status_label(RunStatus::Halted), "Halted");
            assert_eq!(status_label(RunStatus::Crashed), "Crashed");
        }
    }
    ```

- [ ] **Step 6: Build and run the full test suite**

    Run: `cargo build --release --bin stm32-launcher`
    Expected: success, no errors.

    Run: `cargo test --bin stm32-launcher`
    Expected: `test tests::each_run_status_maps_to_a_distinct_color ... ok`, `test tests::each_run_status_has_a_label ... ok`, 2 passed.

    Run: `cargo test --lib && cargo test --test launcher_profile --test launcher_registry --test launcher_workspace --test launcher_state`
    Expected: all passing, 0 failed (this also confirms Tasks 1 and 2's tests still pass together with this task's changes).

- [ ] **Step 7: Manually verify in the running GUI**

    Run: `.\target\release\stm32-launcher.exe`

    1. Select a firmware `.bin`, choose the `proteus_f7` variant (or enable Manual profile with valid paths), so "Run emulator" becomes enabled.
    2. Click **Run emulator**. Expected: both the Signal Chain's step-4 "Emulator" label and the Emulator Output panel's status text turn cyan and read "Running".
    3. Click **Stop**. Expected: both turn amber and read "Halted".
    4. Click **Run emulator** again. In a separate PowerShell window, run `Get-Process stm32-emulator | Select-Object Id` to find the child process, then `Stop-Process -Id <id> -Force` to kill it out from under the launcher (simulating a crash, distinct from clicking the GUI's own Stop button). Expected: within about a second, both status spots turn red and read "Crashed", and the red error line under the buttons reads "Emulator exited with a non-zero status."

    Expected: all three states are visually distinguishable and match the scenario that produced them.

- [ ] **Step 8: Commit**

    ```bash
    git add src/bin/stm32-launcher.rs
    git commit -m "feat: show emulator run status (running/halted/crashed) in the launcher GUI"
    ```
