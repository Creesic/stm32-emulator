use std::path::PathBuf;

use stm32_emulator::launcher::{KnownVariant, LauncherCpuModel, ResolvedProfile};

/// Full Proteus harness pin table (name, pin, direction), copied from
/// `src/launcher/boards/proteus_f7.rs`'s `ECU_IO_PINS`. Order matters: this
/// asserts the generated YAML's pin list matches exactly, not just that each
/// entry is present somewhere.
fn expected_ecu_io_pins() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("vr1", "PE7", "input"),
        ("vr2", "PE8", "input"),
        ("din1", "PC6", "input"),
        ("din2", "PE11", "input"),
        ("din3", "PE12", "input"),
        ("din4", "PE14", "input"),
        ("din5", "PE13", "input"),
        ("din6", "PE15", "input"),
        ("ls1", "PD7", "output"),
        ("ls2", "PG9", "output"),
        ("ls3", "PG10", "output"),
        ("ls4", "PG11", "output"),
        ("ls5", "PG12", "output"),
        ("ls6", "PG13", "output"),
        ("ls7", "PG14", "output"),
        ("ls8", "PB4", "output"),
        ("ls9", "PB5", "output"),
        ("ls10", "PB6", "output"),
        ("ls11", "PB7", "output"),
        ("ls12", "PB8", "output"),
        ("ls13", "PB9", "output"),
        ("ls14", "PE0", "output"),
        ("ls15", "PE1", "output"),
        ("ls16", "PE2", "output"),
        ("hs1", "PA9", "output"),
        ("hs2", "PA8", "output"),
        ("hs3", "PD15", "output"),
        ("hs4", "PD14", "output"),
        ("ign1", "PD4", "output"),
        ("ign2", "PD3", "output"),
        ("ign3", "PC9", "output"),
        ("ign4", "PC8", "output"),
        ("ign5", "PC7", "output"),
        ("ign6", "PG8", "output"),
        ("ign7", "PG7", "output"),
        ("ign8", "PG6", "output"),
        ("ign9", "PG5", "output"),
        ("ign10", "PG4", "output"),
        ("ign11", "PG3", "output"),
        ("ign12", "PG2", "output"),
    ]
}

/// Full Proteus harness ADC channel table (name, pin), copied from
/// `src/launcher/boards/proteus_f7.rs`'s `ECU_IO_ADC_CHANNELS`.
fn expected_ecu_io_adc_channels() -> Vec<(&'static str, &'static str)> {
    vec![
        ("at1", "PC4"),
        ("at2", "PC5"),
        ("at3", "PB0"),
        ("at4", "PB1"),
        ("av1", "PC0"),
        ("av2", "PC1"),
        ("av3", "PC2"),
        ("av4", "PC3"),
        ("av5", "PA0"),
        ("av6", "PA1"),
        ("av7", "PA2"),
        ("av8", "PA3"),
        ("av9", "PA4"),
        ("av10", "PA5"),
        ("av11", "PA6"),
        ("vbatt", "PA7"),
    ]
}

/// Parses `yaml` and returns the first (only) `devices.ecu_io` entry as a
/// `serde_yaml::Value`, for structural (not substring) assertions.
fn ecu_io_value(yaml: &str) -> serde_yaml::Value {
    let root: serde_yaml::Value = serde_yaml::from_str(yaml).expect("generated YAML must parse");
    root["devices"]["ecu_io"][0].clone()
}

fn pins_from(ecu_io: &serde_yaml::Value) -> Vec<(String, String, String)> {
    ecu_io["pins"]
        .as_sequence()
        .expect("ecu_io.pins must be a sequence")
        .iter()
        .map(|pin| {
            (
                pin["name"].as_str().expect("pin.name must be a string").to_string(),
                pin["pin"].as_str().expect("pin.pin must be a string").to_string(),
                pin["direction"].as_str().expect("pin.direction must be a string").to_string(),
            )
        })
        .collect()
}

fn adc_channels_from(ecu_io: &serde_yaml::Value) -> Vec<(String, String)> {
    ecu_io["adc_channels"]
        .as_sequence()
        .expect("ecu_io.adc_channels must be a sequence")
        .iter()
        .map(|channel| {
            (
                channel["name"].as_str().expect("adc_channel.name must be a string").to_string(),
                channel["pin"].as_str().expect("adc_channel.pin must be a string").to_string(),
            )
        })
        .collect()
}

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
fn proteus_f7_yaml_binds_every_harness_name_to_its_exact_pin_and_direction() {
    // The substring checks above (direction/name counts) pass under any
    // permutation of pins, so they can't catch a name bound to the wrong
    // pin or direction. Assert the full structural map instead.
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    let ecu_io = ecu_io_value(&profile.to_yaml().unwrap());

    let expected_pins: Vec<(String, String, String)> = expected_ecu_io_pins()
        .into_iter()
        .map(|(name, pin, direction)| (name.to_string(), pin.to_string(), direction.to_string()))
        .collect();
    assert_eq!(pins_from(&ecu_io), expected_pins);

    let expected_adc_channels: Vec<(String, String)> = expected_ecu_io_adc_channels()
        .into_iter()
        .map(|(name, pin)| (name.to_string(), pin.to_string()))
        .collect();
    assert_eq!(adc_channels_from(&ecu_io), expected_adc_channels);
}

#[test]
fn proteus_f7_config_yaml_matches_the_launcher_generated_ecu_io_device() {
    // Drift guard: the harness map is deliberately duplicated between the
    // launcher profile tables and proteus_f7/config.yaml (see
    // docs/superpowers/specs/2026-07-15-proteus-harness-io-design.md). This
    // asserts the two surfaces stay entry-for-entry identical.
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();
    let launcher_ecu_io = ecu_io_value(&profile.to_yaml().unwrap());

    let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("proteus_f7/config.yaml");
    let config_yaml = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", config_path.display()));
    let config_ecu_io = ecu_io_value(&config_yaml);

    assert_eq!(pins_from(&launcher_ecu_io), pins_from(&config_ecu_io));
    assert_eq!(adc_channels_from(&launcher_ecu_io), adc_channels_from(&config_ecu_io));
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
