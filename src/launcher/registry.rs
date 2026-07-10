// SPDX-License-Identifier: GPL-3.0-or-later

use super::{EmulationSupport, KnownVariant};

#[path = "generated_variants.rs"]
mod generated_variants;

pub(super) struct GeneratedVariant {
    pub id: &'static str,
    pub display_name: &'static str,
    pub mcu_family: Option<&'static str>,
}

pub fn all_variants() -> Vec<KnownVariant> {
    generated_variants::GENERATED_VARIANTS
        .iter()
        .map(|variant| {
            if variant.id == "proteus_f7" {
                KnownVariant::proteus_f7()
            } else {
                KnownVariant::cataloged(variant.id, variant.display_name, variant.mcu_family)
            }
        })
        .collect()
}

pub fn support_summary(variant: KnownVariant) -> &'static str {
    match variant.support {
        EmulationSupport::Runnable => "Runnable: profile and devices are modeled.",
        EmulationSupport::Partial => {
            "Partial: the memory map is verified; device-model coverage remains incomplete."
        }
        EmulationSupport::Unsupported => {
            "Cataloged only: no evidence-backed MCU, memory map, and SVD profile exists."
        }
    }
}
