// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;

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

#[derive(Debug, Deserialize)]
pub struct Config {
   pub cpu: Cpu,
   pub regions: Vec<Region>,
   pub patches: Option<Vec<Patch>>,
   pub peripherals: Option<crate::peripherals::PeripheralsConfig>,
   pub devices: Option<crate::ext_devices::ExtDevicesConfig>,
   pub framebuffers: Option<Vec<crate::framebuffers::FramebufferConfig>>,
}

#[cfg(test)]
mod tests {
    use super::{Config, CpuModel};

    #[test]
    fn cpu_model_deserializes_kebab_case_name() {
        let config: Config = serde_yaml::from_str(
            "cpu:\n  model: cortex-m7\n  svd: chip.svd\n  vector_table: 0x00200000\nregions: []",
        )
        .unwrap();

        assert_eq!(config.cpu.model, CpuModel::CortexM7);
    }
}
