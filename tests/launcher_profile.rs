use std::path::PathBuf;

use stm32_emulator::launcher::{KnownVariant, LauncherCpuModel, ResolvedProfile};

#[test]
fn proteus_f7_resolves_both_verified_firmware_aliases() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from(r"C:\firmware\rusefi.bin"),
        PathBuf::from(r"C:\svd\STM32F767.svd"),
    )
    .expect("Proteus F7 has an evidence-backed partial profile");

    assert_eq!(profile.vector_table, 0x0020_0000);
    assert!(profile
        .regions
        .iter()
        .any(|region| region.start == 0x0020_0000 && region.load_firmware));
    assert!(profile
        .regions
        .iter()
        .any(|region| region.start == 0x0800_0000 && region.load_firmware));
}

#[test]
fn unsupported_variant_cannot_resolve_to_a_runnable_profile() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::unsupported_for_test(),
        PathBuf::from(r"C:\firmware\unknown.bin"),
        PathBuf::from(r"C:\svd\unknown.svd"),
    );

    assert!(profile.is_err());
}

#[test]
fn manual_profile_uses_only_explicit_memory_values() {
    let profile = ResolvedProfile::manual(
        LauncherCpuModel::CortexM4,
        PathBuf::from(r"C:\firmware\custom.bin"),
        PathBuf::from(r"C:\svd\custom.svd"),
        0x0800_0000,
        0x0800_0000,
        0x0010_0000,
        0x2000_0000,
        0x0002_0000,
    );

    assert_eq!(profile.vector_table, 0x0800_0000);
    assert_eq!(profile.regions.len(), 2);
    assert_eq!(profile.regions[0].start, 0x0800_0000);
    assert!(profile.regions[0].load_firmware);
    assert_eq!(profile.regions[1].start, 0x2000_0000);
}

#[test]
fn proteus_f7_yaml_selects_cortex_m7() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    assert!(profile.to_yaml().unwrap().contains("model: cortex-m7"));
}

#[test]
fn proteus_f7_yaml_includes_the_usb_cdc_tcp_and_ecu_io_devices() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    let yaml = profile.to_yaml().unwrap();
    assert!(yaml.contains("usb_cdc_tcp"));
    assert!(yaml.contains("peripheral: OTG_FS_GLOBAL"));
    assert!(yaml.contains("127.0.0.1:29000"));
    assert!(yaml.contains("max_buffered_bytes: 65536"));
    assert!(yaml.contains("ecu_io"));
    assert!(yaml.contains("127.0.0.1:29002"));
    assert!(yaml.contains("name: crank"));
    assert!(yaml.contains("direction: input"));
}

#[test]
fn manual_profile_yaml_has_no_devices_section() {
    let profile = ResolvedProfile::manual(
        LauncherCpuModel::CortexM4,
        PathBuf::from("firmware.bin"),
        PathBuf::from("chip.svd"),
        0x0800_0000,
        0x0800_0000,
        0x0010_0000,
        0x2000_0000,
        0x0002_0000,
    );

    assert!(!profile.to_yaml().unwrap().contains("devices"));
}

#[test]
fn manual_profile_yaml_uses_the_selected_cpu_model() {
    let profile = ResolvedProfile::manual(
        LauncherCpuModel::CortexM4,
        PathBuf::from("firmware.bin"),
        PathBuf::from("chip.svd"),
        0x0800_0000,
        0x0800_0000,
        0x0010_0000,
        0x2000_0000,
        0x0002_0000,
    );

    assert!(profile.to_yaml().unwrap().contains("model: cortex-m4"));
}
