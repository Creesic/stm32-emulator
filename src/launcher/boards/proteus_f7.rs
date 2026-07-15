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

// The complete Proteus harness signal map, named by physical connector
// position (the tune decides which position is MAP or injector 3).
// Source of truth: proteus_meta.h in the epicefi_fw checkout; the pin
// order within each group is that header's numbering. Note din5/din6 are
// PE13/PE15 (PROTEUS_DIGITAL_5/6) -- deliberately not ascending pin order.
const ECU_IO_PINS: [EcuIoPin; 40] = [
    // VR trigger inputs
    EcuIoPin { name: "vr1", pin: "PE7", direction: "input" },
    EcuIoPin { name: "vr2", pin: "PE8", direction: "input" },
    // Digital inputs
    EcuIoPin { name: "din1", pin: "PC6", direction: "input" },
    EcuIoPin { name: "din2", pin: "PE11", direction: "input" },
    EcuIoPin { name: "din3", pin: "PE12", direction: "input" },
    EcuIoPin { name: "din4", pin: "PE14", direction: "input" },
    EcuIoPin { name: "din5", pin: "PE13", direction: "input" },
    EcuIoPin { name: "din6", pin: "PE15", direction: "input" },
    // Lowside (injector-class) outputs
    EcuIoPin { name: "ls1", pin: "PD7", direction: "output" },
    EcuIoPin { name: "ls2", pin: "PG9", direction: "output" },
    EcuIoPin { name: "ls3", pin: "PG10", direction: "output" },
    EcuIoPin { name: "ls4", pin: "PG11", direction: "output" },
    EcuIoPin { name: "ls5", pin: "PG12", direction: "output" },
    EcuIoPin { name: "ls6", pin: "PG13", direction: "output" },
    EcuIoPin { name: "ls7", pin: "PG14", direction: "output" },
    EcuIoPin { name: "ls8", pin: "PB4", direction: "output" },
    EcuIoPin { name: "ls9", pin: "PB5", direction: "output" },
    EcuIoPin { name: "ls10", pin: "PB6", direction: "output" },
    EcuIoPin { name: "ls11", pin: "PB7", direction: "output" },
    EcuIoPin { name: "ls12", pin: "PB8", direction: "output" },
    EcuIoPin { name: "ls13", pin: "PB9", direction: "output" },
    EcuIoPin { name: "ls14", pin: "PE0", direction: "output" },
    EcuIoPin { name: "ls15", pin: "PE1", direction: "output" },
    EcuIoPin { name: "ls16", pin: "PE2", direction: "output" },
    // Highside outputs
    EcuIoPin { name: "hs1", pin: "PA9", direction: "output" },
    EcuIoPin { name: "hs2", pin: "PA8", direction: "output" },
    EcuIoPin { name: "hs3", pin: "PD15", direction: "output" },
    EcuIoPin { name: "hs4", pin: "PD14", direction: "output" },
    // Ignition outputs
    EcuIoPin { name: "ign1", pin: "PD4", direction: "output" },
    EcuIoPin { name: "ign2", pin: "PD3", direction: "output" },
    EcuIoPin { name: "ign3", pin: "PC9", direction: "output" },
    EcuIoPin { name: "ign4", pin: "PC8", direction: "output" },
    EcuIoPin { name: "ign5", pin: "PC7", direction: "output" },
    EcuIoPin { name: "ign6", pin: "PG8", direction: "output" },
    EcuIoPin { name: "ign7", pin: "PG7", direction: "output" },
    EcuIoPin { name: "ign8", pin: "PG6", direction: "output" },
    EcuIoPin { name: "ign9", pin: "PG5", direction: "output" },
    EcuIoPin { name: "ign10", pin: "PG4", direction: "output" },
    EcuIoPin { name: "ign11", pin: "PG3", direction: "output" },
    EcuIoPin { name: "ign12", pin: "PG2", direction: "output" },
];

// All 16 slow-ADC channels (EFI_ADC_0 PA0 ... EFI_ADC_15 PC5), exactly
// covering the modeled ADC1 channel set with no gaps or overlaps.
const ECU_IO_ADC_CHANNELS: [EcuIoAdcChannel; 16] = [
    // Analog Temp inputs
    EcuIoAdcChannel { name: "at1", pin: "PC4" },
    EcuIoAdcChannel { name: "at2", pin: "PC5" },
    EcuIoAdcChannel { name: "at3", pin: "PB0" },
    EcuIoAdcChannel { name: "at4", pin: "PB1" },
    // Analog Volt inputs
    EcuIoAdcChannel { name: "av1", pin: "PC0" },
    EcuIoAdcChannel { name: "av2", pin: "PC1" },
    EcuIoAdcChannel { name: "av3", pin: "PC2" },
    EcuIoAdcChannel { name: "av4", pin: "PC3" },
    EcuIoAdcChannel { name: "av5", pin: "PA0" },
    EcuIoAdcChannel { name: "av6", pin: "PA1" },
    EcuIoAdcChannel { name: "av7", pin: "PA2" },
    EcuIoAdcChannel { name: "av8", pin: "PA3" },
    EcuIoAdcChannel { name: "av9", pin: "PA4" },
    EcuIoAdcChannel { name: "av10", pin: "PA5" },
    EcuIoAdcChannel { name: "av11", pin: "PA6" },
    // Battery sense
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
