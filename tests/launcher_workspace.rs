use std::path::PathBuf;

use stm32_emulator::launcher::workspace::{
    SavedLauncherState, WindowPlacement, Workspace, WorkspaceStore,
};

#[test]
fn workspace_round_trips_loaded_files_and_window_placement() {
    let root = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::in_directory(root.path()).unwrap();
    let expected = Workspace {
        window: Some(WindowPlacement {
            x: 120,
            y: 90,
            width: 1500,
            height: 920,
        }),
        state: SavedLauncherState {
            firmware: Some(PathBuf::from("firmware.bin")),
            svd: Some(PathBuf::from("chip.svd")),
            emulator_executable: Some(PathBuf::from("stm32-emulator.exe")),
            selected_variant: Some("proteus_f7".to_owned()),
            filter: "proteus".to_owned(),
            ..Default::default()
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
