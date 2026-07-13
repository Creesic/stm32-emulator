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
