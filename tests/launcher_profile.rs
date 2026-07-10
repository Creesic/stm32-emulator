use std::path::PathBuf;

use stm32_emulator::launcher::{KnownVariant, ResolvedProfile};

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
