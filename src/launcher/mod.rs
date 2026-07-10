// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

pub mod registry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmulationSupport {
    Runnable,
    Partial,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KnownVariant {
    pub id: &'static str,
    pub display_name: &'static str,
    pub mcu: Option<&'static str>,
    pub support: EmulationSupport,
    profile: Option<&'static ProfileTemplate>,
}

impl KnownVariant {
    pub fn proteus_f7() -> Self {
        Self {
            id: "proteus_f7",
            display_name: "Proteus F7",
            mcu: Some("STM32F767"),
            support: EmulationSupport::Partial,
            profile: Some(&PROTEUS_F7_PROFILE),
        }
    }

    pub fn unsupported_for_test() -> Self {
        Self {
            id: "unsupported",
            display_name: "Unsupported board",
            mcu: None,
            support: EmulationSupport::Unsupported,
            profile: None,
        }
    }

    pub(crate) const fn cataloged(
        id: &'static str,
        display_name: &'static str,
        mcu: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            display_name,
            mcu,
            support: EmulationSupport::Unsupported,
            profile: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub name: &'static str,
    pub start: u32,
    pub size: u32,
    pub load_firmware: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProfile {
    pub variant: KnownVariant,
    pub vector_table: u32,
    pub firmware: PathBuf,
    pub svd: PathBuf,
    pub regions: Vec<MemoryRegion>,
}

impl ResolvedProfile {
    pub fn for_variant(
        variant: KnownVariant,
        firmware: PathBuf,
        svd: PathBuf,
    ) -> Result<Self, ProfileError> {
        let template = variant
            .profile
            .ok_or(ProfileError::UnsupportedVariant(variant.id))?;

        Ok(Self {
            variant,
            vector_table: template.vector_table,
            firmware,
            svd,
            regions: template.regions.to_vec(),
        })
    }

    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(&YamlConfig {
            cpu: YamlCpu {
                svd: self.svd.to_string_lossy(),
                vector_table: self.vector_table,
            },
            regions: self
                .regions
                .iter()
                .map(|region| YamlRegion {
                    name: region.name,
                    start: region.start,
                    size: region.size,
                    load: region
                        .load_firmware
                        .then(|| self.firmware.to_string_lossy()),
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    UnsupportedVariant(&'static str),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVariant(id) => {
                write!(
                    formatter,
                    "Variant '{id}' has no verified emulation profile"
                )
            }
        }
    }
}

impl std::error::Error for ProfileError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProfileTemplate {
    vector_table: u32,
    regions: &'static [MemoryRegion],
}

const PROTEUS_F7_REGIONS: [MemoryRegion; 6] = [
    MemoryRegion {
        name: "ROM-ITCM-alias",
        start: 0x0020_0000,
        size: 0x0020_0000,
        load_firmware: true,
    },
    MemoryRegion {
        name: "ROM-AXI-alias",
        start: 0x0800_0000,
        size: 0x0020_0000,
        load_firmware: true,
    },
    MemoryRegion {
        name: "ITCM-RAM",
        start: 0x0000_0000,
        size: 0x0000_4000,
        load_firmware: false,
    },
    MemoryRegion {
        name: "DTCM-RAM",
        start: 0x2000_0000,
        size: 0x0002_0000,
        load_firmware: false,
    },
    MemoryRegion {
        name: "SRAM1",
        start: 0x2002_0000,
        size: 0x0005_c000,
        load_firmware: false,
    },
    MemoryRegion {
        name: "SRAM2",
        start: 0x2007_c000,
        size: 0x0000_4000,
        load_firmware: false,
    },
];

const PROTEUS_F7_PROFILE: ProfileTemplate = ProfileTemplate {
    vector_table: 0x0020_0000,
    regions: &PROTEUS_F7_REGIONS,
};

#[derive(Serialize)]
struct YamlConfig<'a> {
    cpu: YamlCpu<'a>,
    regions: Vec<YamlRegion<'a>>,
}

#[derive(Serialize)]
struct YamlCpu<'a> {
    svd: std::borrow::Cow<'a, str>,
    vector_table: u32,
}

#[derive(Serialize)]
struct YamlRegion<'a> {
    name: &'a str,
    start: u32,
    size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    load: Option<std::borrow::Cow<'a, str>>,
}
