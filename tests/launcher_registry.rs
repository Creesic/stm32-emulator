use std::collections::HashSet;
use std::path::PathBuf;

use stm32_emulator::launcher::{registry, EmulationSupport, ResolvedProfile};

#[test]
fn compiled_catalog_has_unique_sorted_variant_ids() {
    let variants = registry::all_variants();
    let ids: Vec<_> = variants.iter().map(|variant| variant.id).collect();
    let unique: HashSet<_> = ids.iter().copied().collect();

    assert_eq!(ids.len(), unique.len());
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(ids.contains(&"proteus_f7"));
}

#[test]
fn unsupported_catalog_variants_do_not_resolve() {
    for variant in registry::all_variants()
        .iter()
        .filter(|variant| variant.support == EmulationSupport::Unsupported)
    {
        assert!(
            ResolvedProfile::for_variant(
                *variant,
                PathBuf::from(r"C:\firmware\image.bin"),
                PathBuf::from(r"C:\svd\device.svd"),
            )
            .is_err(),
            "{} unexpectedly resolved",
            variant.id
        );
    }
}
