// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod process;
pub mod registry;
pub mod ui_state;
pub mod workspace;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmulationSupport {
    Runnable,
    Partial,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherCpuModel {
    CortexM4,
    CortexM7,
}

impl Default for LauncherCpuModel {
    fn default() -> Self {
        Self::CortexM4
    }
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

    pub fn manual() -> Self {
        Self {
            id: "manual",
            display_name: "Manual profile",
            mcu: None,
            support: EmulationSupport::Runnable,
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
    pub cpu_model: LauncherCpuModel,
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
            cpu_model: template.cpu_model,
            vector_table: template.vector_table,
            firmware,
            svd,
            regions: template.regions.to_vec(),
        })
    }

    pub fn manual(
        cpu_model: LauncherCpuModel,
        firmware: PathBuf,
        svd: PathBuf,
        vector_table: u32,
        flash_start: u32,
        flash_size: u32,
        ram_start: u32,
        ram_size: u32,
    ) -> Self {
        Self {
            variant: KnownVariant::manual(),
            cpu_model,
            vector_table,
            firmware,
            svd,
            regions: vec![
                MemoryRegion {
                    name: "Manual-FLASH",
                    start: flash_start,
                    size: flash_size,
                    load_firmware: true,
                },
                MemoryRegion {
                    name: "Manual-RAM",
                    start: ram_start,
                    size: ram_size,
                    load_firmware: false,
                },
            ],
        }
    }

    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(&YamlConfig {
            cpu: YamlCpu {
                model: self.cpu_model,
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
    cpu_model: LauncherCpuModel,
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
    cpu_model: LauncherCpuModel::CortexM7,
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
    model: LauncherCpuModel,
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
