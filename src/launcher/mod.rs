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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryPatch {
    pub start: u32,
    pub data: &'static [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct UsbCdcTcpDevice {
    pub peripheral: &'static str,
    pub listen: &'static str,
    pub max_buffered_bytes: usize,
}

/// Owned counterpart of `UsbCdcTcpDevice`, used once a profile is resolved:
/// unlike the compile-time template (whose `listen` is a fixed `&'static
/// str`), the launcher lets the user edit this port at runtime, so it needs
/// to hold an owned string rather than a `const`-friendly static one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedUsbCdcTcp {
    pub peripheral: &'static str,
    pub listen: String,
    pub max_buffered_bytes: usize,
}

impl From<UsbCdcTcpDevice> for ResolvedUsbCdcTcp {
    fn from(device: UsbCdcTcpDevice) -> Self {
        Self {
            peripheral: device.peripheral,
            listen: device.listen.to_owned(),
            max_buffered_bytes: device.max_buffered_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EcuIoPin {
    pub name: &'static str,
    pub pin: &'static str,
    pub direction: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EcuIoAdcChannel {
    pub name: &'static str,
    pub pin: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EcuIoDevice {
    pub listen: &'static str,
    pub pins: &'static [EcuIoPin],
    pub adc_channels: &'static [EcuIoAdcChannel],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProfile {
    pub variant: KnownVariant,
    pub cpu_model: LauncherCpuModel,
    pub vector_table: u32,
    pub firmware: PathBuf,
    pub svd: PathBuf,
    pub regions: Vec<MemoryRegion>,
    pub patches: Vec<MemoryPatch>,
    pub usb_cdc_tcp: Option<ResolvedUsbCdcTcp>,
    pub ecu_io: Option<EcuIoDevice>,
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
            patches: template.patches.to_vec(),
            usb_cdc_tcp: template.usb_cdc_tcp.map(ResolvedUsbCdcTcp::from),
            ecu_io: template.ecu_io,
        })
    }

    /// The port currently configured for the USB CDC TCP bridge (e.g. what
    /// TunerStudio connects to), if this profile has one at all.
    pub fn usb_cdc_tcp_port(&self) -> Option<u16> {
        self.usb_cdc_tcp
            .as_ref()?
            .listen
            .rsplit(':')
            .next()?
            .parse()
            .ok()
    }

    /// Overrides the USB CDC TCP bridge's listen port (host stays
    /// 127.0.0.1). No-op if this profile has no USB CDC TCP device.
    pub fn set_usb_cdc_tcp_port(&mut self, port: u16) {
        if let Some(device) = &mut self.usb_cdc_tcp {
            device.listen = format!("127.0.0.1:{port}");
        }
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
            patches: Vec::new(),
            usb_cdc_tcp: None,
            ecu_io: None,
        }
    }

    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        let devices = if self.usb_cdc_tcp.is_some() || self.ecu_io.is_some() {
            Some(YamlDevices {
                usb_cdc_tcp: self.usb_cdc_tcp.iter().cloned().collect(),
                ecu_io: self.ecu_io.iter().copied().collect(),
            })
        } else {
            None
        };

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
            patches: self.patches.clone(),
            devices,
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
    patches: &'static [MemoryPatch],
    usb_cdc_tcp: Option<UsbCdcTcpDevice>,
    ecu_io: Option<EcuIoDevice>,
}

const PROTEUS_F7_REGIONS: [MemoryRegion; 7] = [
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
        name: "System-identifiers",
        start: 0x1ff0_f000,
        size: 0x0000_1000,
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

const PROTEUS_F7_PATCHES: [MemoryPatch; 1] = [
    // STM32F767's flash-size ID register (FLASHSIZE_BASE, RM0410) --
    // firmware reads this at boot via TM_ID_GetFlashSize() and refuses to
    // continue if it reports less than 1024K. It lives in the
    // otherwise-blank System-identifiers region above, which has no real
    // flash content, so it always read back 0 and firmware halted with
    // "expected at least 1024K of flash". Two little-endian bytes: 2048
    // (0x0800) KB, matching this profile's 0x200000-byte (2MB) ROM regions.
    MemoryPatch { start: 0x1ff0_f442, data: &[0x00, 0x08] },
];

const PROTEUS_F7_ECU_IO_PINS: [EcuIoPin; 4] = [
    EcuIoPin { name: "crank", pin: "PC6", direction: "input" },
    EcuIoPin { name: "cam", pin: "PE11", direction: "input" },
    EcuIoPin { name: "inj1", pin: "PD7", direction: "output" },
    EcuIoPin { name: "ign1", pin: "PD4", direction: "output" },
];

const PROTEUS_F7_ECU_IO_ADC_CHANNELS: [EcuIoAdcChannel; 5] = [
    EcuIoAdcChannel { name: "map", pin: "PC0" },
    EcuIoAdcChannel { name: "tps", pin: "PC1" },
    EcuIoAdcChannel { name: "clt", pin: "PB0" },
    EcuIoAdcChannel { name: "iat", pin: "PC5" },
    EcuIoAdcChannel { name: "vbatt", pin: "PA7" },
];

const PROTEUS_F7_PROFILE: ProfileTemplate = ProfileTemplate {
    cpu_model: LauncherCpuModel::CortexM7,
    vector_table: 0x0020_0000,
    regions: &PROTEUS_F7_REGIONS,
    patches: &PROTEUS_F7_PATCHES,
    usb_cdc_tcp: Some(UsbCdcTcpDevice {
        peripheral: "OTG_FS_GLOBAL",
        listen: "127.0.0.1:29000",
        max_buffered_bytes: 65536,
    }),
    ecu_io: Some(EcuIoDevice {
        listen: "127.0.0.1:29002",
        pins: &PROTEUS_F7_ECU_IO_PINS,
        adc_channels: &PROTEUS_F7_ECU_IO_ADC_CHANNELS,
    }),
};

#[derive(Serialize)]
struct YamlConfig<'a> {
    cpu: YamlCpu<'a>,
    regions: Vec<YamlRegion<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    patches: Vec<MemoryPatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    devices: Option<YamlDevices>,
}

#[derive(Serialize)]
struct YamlDevices {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    usb_cdc_tcp: Vec<ResolvedUsbCdcTcp>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ecu_io: Vec<EcuIoDevice>,
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
