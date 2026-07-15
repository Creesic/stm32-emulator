// SPDX-License-Identifier: GPL-3.0-or-later

//! Proteus F7 (STM32F767) launcher profile: memory map, boot patches,
//! and the ecu_io harness signal tables.

use super::super::{
    EcuIoAdcChannel, EcuIoDevice, EcuIoPin, LauncherCpuModel, MemoryPatch,
    MemoryRegion, ProfileTemplate, UsbCdcTcpDevice,
};

const REGIONS: [MemoryRegion; 7] = [
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

const PATCHES: [MemoryPatch; 1] = [
    // STM32F767's flash-size ID register (FLASHSIZE_BASE, RM0410) --
    // firmware reads this at boot via TM_ID_GetFlashSize() and refuses to
    // continue if it reports less than 1024K. It lives in the
    // otherwise-blank System-identifiers region above, which has no real
    // flash content, so it always read back 0 and firmware halted with
    // "expected at least 1024K of flash". Two little-endian bytes: 2048
    // (0x0800) KB, matching this profile's 0x200000-byte (2MB) ROM regions.
    MemoryPatch { start: 0x1ff0_f442, data: &[0x00, 0x08] },
];

const ECU_IO_PINS: [EcuIoPin; 4] = [
    EcuIoPin { name: "crank", pin: "PC6", direction: "input" },
    EcuIoPin { name: "cam", pin: "PE11", direction: "input" },
    EcuIoPin { name: "inj1", pin: "PD7", direction: "output" },
    EcuIoPin { name: "ign1", pin: "PD4", direction: "output" },
];

const ECU_IO_ADC_CHANNELS: [EcuIoAdcChannel; 5] = [
    EcuIoAdcChannel { name: "map", pin: "PC0" },
    EcuIoAdcChannel { name: "tps", pin: "PC1" },
    EcuIoAdcChannel { name: "clt", pin: "PB0" },
    EcuIoAdcChannel { name: "iat", pin: "PC5" },
    EcuIoAdcChannel { name: "vbatt", pin: "PA7" },
];

pub(crate) const PROFILE: ProfileTemplate = ProfileTemplate {
    cpu_model: LauncherCpuModel::CortexM7,
    vector_table: 0x0020_0000,
    regions: &REGIONS,
    patches: &PATCHES,
    usb_cdc_tcp: Some(UsbCdcTcpDevice {
        peripheral: "OTG_FS_GLOBAL",
        listen: "127.0.0.1:29000",
        max_buffered_bytes: 65536,
    }),
    ecu_io: Some(EcuIoDevice {
        listen: "127.0.0.1:29002",
        pins: &ECU_IO_PINS,
        adc_channels: &ECU_IO_ADC_CHANNELS,
    }),
};
