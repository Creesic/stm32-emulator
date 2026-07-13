# Launcher Process Status Indicator Design

## Problem

The native launcher GUI (`src/bin/stm32-launcher.rs`) currently tracks only a
`running: bool` for the emulator child process. The Emulator Output panel
shows just "Running" or "Idle," and the Signal Chain bar's step-4 "Emulator"
indicator is a binary cyan/amber dot. Neither distinguishes "the process
exited cleanly" from "the process crashed" — `RunningEmulator::is_running()`
(`src/launcher/process.rs`) already calls `Child::try_wait()`, which returns
the process's `ExitStatus`, but discards it down to a bare `bool`. The user
has no way to tell, at a glance, whether the emulator is still running,
stopped cleanly, or died with an error, short of reading raw log lines or
checking Task Manager.

## Goal

Show a status of `Idle` / `Running` / `Halted` / `Crashed` in two places:
the Emulator Output panel (replacing the current Running/Idle text) and the
Signal Chain bar's step-4 "Emulator" indicator — so the state is visible
whether or not the Output panel is in view.

## State model

`RunStatus` (new enum, `src/launcher/ui_state.rs`), replacing
`LauncherState`'s `running: bool` field outright — one source of truth,
nothing to keep in sync:

```rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunStatus {
    #[default]
    Idle,
    Running,
    Halted,
    Crashed,
}
```

`LauncherState::can_run()` keys off `self.status != RunStatus::Running`
(same gating behavior as the current `!self.running`).

`RunningEmulator::is_running() -> Result<bool, ProcessError>`
(`src/launcher/process.rs`) is replaced by:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Running,
    Exited { success: bool },
}

pub fn poll_state(&mut self) -> Result<ProcessState, ProcessError> {
    match self.child.try_wait().map_err(ProcessError::io)? {
        None => Ok(ProcessState::Running),
        Some(status) => Ok(ProcessState::Exited { success: status.success() }),
    }
}
```

### Transition rules

- `App::start()` succeeds → `RunStatus::Running`.
- User clicks **Stop** (`App::stop()`) → always `RunStatus::Halted`,
  regardless of the resulting exit code. `Child::kill()` uses
  `TerminateProcess` on Windows, which reports a non-zero exit code even for
  a deliberate stop; "the user asked it to stop" must override the exit-code
  check, or every manual Stop would misreport as Crashed.
- Process exits on its own, detected on the next frame in
  `App::refresh_process()` via `poll_state()`:
  - `Exited { success: true }` → `RunStatus::Halted`.
  - `Exited { success: false }` → `RunStatus::Crashed`.
- `poll_state()` returns `Err` (the OS query itself failed) →
  `RunStatus::Crashed`, and `last_error` is set to the error message (this
  path already exists today).
- New: when `refresh_process()` transitions to `Crashed` via
  `Exited { success: false }` (not a poll error), also set
  `last_error` to a short explanatory message (e.g. "Emulator exited with a
  non-zero status."), since today a bad exit code alone leaves `last_error`
  unset and the existing red error line under the Run/Stop buttons would
  stay empty.

This intentionally does not distinguish "process exited cleanly on its own"
(e.g. a future `--stop-addr`/`--max-instructions`/busy-loop bound) from
"user clicked Stop" — both are `Halted`. The GUI does not currently expose
those bounding flags (`build_emulator_arguments` only appends `-v` flags),
so a spontaneous clean exit is rare in practice; if the GUI later exposes
those flags, revisit whether `Halted` needs to be split further.

## Rendering

New `GRAY` color constant in `src/bin/stm32-launcher.rs`, alongside the
existing `BG`/`PANEL`/`AMBER`/`CYAN`/`RED`. One shared mapping used by both
display spots so they can't disagree with each other:

| Status  | Color | 
|---------|-------|
| Idle    | GRAY  |
| Running | CYAN  |
| Halted  | AMBER |
| Crashed | RED   |

- **Emulator Output panel**: the existing status text next to the Run/Stop
  buttons becomes `status_label(app.state.status)` rendered in
  `status_color(app.state.status)`.
- **Signal Chain bar, step 4 ("Emulator")**: a new `emulator_step_indicator`
  helper renders this step using the same color/label mapping. The existing
  `indicator(ui, label, bool)` helper (shared by steps 1–3, which are plain
  booleans) is left unchanged — a new helper avoids forcing steps 1-3 to
  pass colors they don't need.

## Testing

`process.rs` and `stm32-launcher.rs`'s `App` orchestration have no existing
tests (process spawning and GUI state aren't covered today), so this design
scopes new tests to the pure logic introduced:

- `tests/launcher_state.rs`: default `LauncherState::status` is
  `RunStatus::Idle`; `can_run()` is `true` for `Idle`/`Halted`/`Crashed`
  (with firmware and a selected variant set) and `false` for `Running`.
- A `#[cfg(test)]` block in `process.rs` verifying the exit-code →
  `ProcessState` mapping using
  `std::os::windows::process::ExitStatusExt::from_raw(code)` to construct an
  `ExitStatus` directly (no real child process needed) — code `0` maps to
  `Exited { success: true }`, non-zero maps to `Exited { success: false }`.
  This mirrors the existing `#[cfg(windows)]` precedent already in this
  file (`RunningEmulator::spawn`'s `creation_flags` call).
