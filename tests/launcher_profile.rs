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

    // The full Proteus harness map: 8 digital inputs, 32 observed
    // outputs, 16 ADC channels (see
    // docs/superpowers/specs/2026-07-15-proteus-harness-io-design.md).
    assert_eq!(yaml.matches("direction: input").count(), 8);
    assert_eq!(yaml.matches("direction: output").count(), 32);
    assert_eq!(yaml.matches("name: av").count(), 11);
    assert_eq!(yaml.matches("name: at").count(), 4);
    assert!(yaml.contains("name: vbatt"));

    // Spot-check group boundaries against proteus_meta.h.
    assert!(yaml.contains("name: ls16"));
    assert!(yaml.contains("name: ign12"));
    assert!(yaml.contains("name: hs4"));
    assert!(yaml.contains("name: din6"));
    assert!(yaml.contains("name: vr1"));
    assert!(yaml.contains("pin: PE15")); // din6
    assert!(yaml.contains("pin: PG2")); // ign12
    assert!(!yaml.contains("name: crank")); // old functional names are gone
    assert!(!yaml.contains("name: map"));
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

#[test]
fn proteus_f7_yaml_includes_the_flash_size_patch() {
    // Firmware reads its own flash size back from a fixed ROM address
    // (FLASHSIZE_BASE) at boot and refuses to continue if it reads as 0,
    // which it always would inside the otherwise-blank System-identifiers
    // region without this patch.
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    let yaml = profile.to_yaml().unwrap();
    assert!(yaml.contains("patches"));
    assert!(yaml.contains("start: 535884866")); // 0x1ff0f442
}

#[test]
fn manual_profile_yaml_has_no_patches_section() {
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

    assert!(!profile.to_yaml().unwrap().contains("patches"));
}

#[test]
fn usb_cdc_tcp_port_defaults_to_the_profiles_template_value() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    assert_eq!(profile.usb_cdc_tcp_port(), Some(29000));
}

#[test]
fn setting_the_usb_cdc_tcp_port_changes_the_generated_yaml() {
    let mut profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    profile.set_usb_cdc_tcp_port(40123);

    assert_eq!(profile.usb_cdc_tcp_port(), Some(40123));
    let yaml = profile.to_yaml().unwrap();
    assert!(yaml.contains("127.0.0.1:40123"));
    assert!(!yaml.contains("127.0.0.1:29000"));
}

#[test]
fn setting_the_usb_cdc_tcp_port_on_a_manual_profile_is_a_harmless_no_op() {
    // Manual profiles never have a usb_cdc_tcp device at all.
    let mut profile = ResolvedProfile::manual(
        LauncherCpuModel::CortexM4,
        PathBuf::from("firmware.bin"),
        PathBuf::from("chip.svd"),
        0x0800_0000,
        0x0800_0000,
        0x0010_0000,
        0x2000_0000,
        0x0002_0000,
    );

    profile.set_usb_cdc_tcp_port(40123);

    assert_eq!(profile.usb_cdc_tcp_port(), None);
}
