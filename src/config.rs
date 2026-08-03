// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Region {
    pub name: String,
    pub start: u32,
    pub size: u32,
    pub load: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Patch {
    pub start: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CpuModel {
    CortexM4,
    CortexM7,
}

#[derive(Debug, Deserialize)]
pub struct Cpu {
    pub model: CpuModel,
    pub svd: String,
    pub vector_table: u32,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InternalFlashModel {
    Stm32F7DualBank,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FlashPersistence {
    pub start: u32,
    pub size: u32,
    pub backing_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct InternalFlashConfig {
    pub model: InternalFlashModel,
    pub start: u32,
    pub size: u32,
    #[serde(default)]
    pub aliases: Vec<u32>,
    pub persistent: Option<FlashPersistence>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub cpu: Cpu,
    pub regions: Vec<Region>,
    pub internal_flash: Option<InternalFlashConfig>,
    pub patches: Option<Vec<Patch>>,
    pub peripherals: Option<crate::peripherals::PeripheralsConfig>,
    pub devices: Option<crate::ext_devices::ExtDevicesConfig>,
    pub framebuffers: Option<Vec<crate::framebuffers::FramebufferConfig>>,
}

#[cfg(test)]
mod tests {
    use super::{Config, CpuModel, InternalFlashModel};

    #[test]
    fn cpu_model_deserializes_kebab_case_name() {
        let config: Config = serde_yaml::from_str(
            "cpu:\n  model: cortex-m7\n  svd: chip.svd\n  vector_table: 0x00200000\nregions: []",
        )
        .unwrap();

        assert_eq!(config.cpu.model, CpuModel::CortexM7);
    }

    #[test]
    fn internal_flash_deserializes_persistent_bank() {
        let config: Config = serde_yaml::from_str(
            "cpu:\n  model: cortex-m7\n  svd: chip.svd\n  vector_table: 0x00200000\nregions: []\ninternal_flash:\n  model: stm32-f7-dual-bank\n  start: 0x08000000\n  size: 0x200000\n  aliases: [0x00200000]\n  persistent:\n    start: 0x08100000\n    size: 0x100000\n",
        )
        .unwrap();

        let flash = config.internal_flash.unwrap();
        assert_eq!(flash.model, InternalFlashModel::Stm32F7DualBank);
        assert_eq!(flash.aliases, vec![0x0020_0000]);
        let persistent = flash.persistent.unwrap();
        assert_eq!(persistent.start, 0x0810_0000);
        assert_eq!(persistent.size, 0x0010_0000);
        assert_eq!(persistent.backing_file, None);
    }
}
