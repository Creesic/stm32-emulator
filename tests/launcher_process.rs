use std::path::Path;

use stm32_emulator::launcher::process::{build_emulator_arguments, validate_firmware};

#[test]
fn builds_config_path_and_requested_verbosity_arguments() {
    assert_eq!(
        build_emulator_arguments(Path::new("resolved.yaml"), 1),
        vec!["resolved.yaml".to_owned(), "-v".to_owned()]
    );
}

#[test]
fn rejects_missing_firmware_before_starting_a_child_process() {
    let result = validate_firmware(Path::new(r"C:\not-a-real-firmware\image.bin"));

    assert!(result.is_err());
}
