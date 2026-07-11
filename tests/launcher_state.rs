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
