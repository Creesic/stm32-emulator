# Proteus F7 Full Harness I/O Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the complete Proteus F7 harness I/O surface (32 outputs, 8 digital inputs, 16 ADC channels) through the existing `ecu_io` TCP bridge, named by physical harness position, in both the CLI config and the launcher's compiled-in profile — with the launcher's board data split into a per-board module for future boards.

**Architecture:** No emulator-core code changes — `Adc` already resolves all 16 ADC1 channels by pin and `EcuIo` is config-driven. The work is: (1) move Proteus-specific launcher data into a new `src/launcher/boards/proteus_f7.rs` module, (2) grow the signal tables in that module and in `proteus_f7/config.yaml` to the full harness map, (3) update the tests/scripts/docs that pin the old 9-signal names.

**Tech Stack:** Rust (serde/serde_yaml for launcher config generation), YAML, PowerShell smoke script.

**Spec:** `docs/superpowers/specs/2026-07-15-proteus-harness-io-design.md` — the naming table there is authoritative (source: `proteus_meta.h` in the epicefi_fw checkout).

## Global Constraints

- Windows builds need `CMAKE_POLICY_VERSION_MINIMUM=3.5` and `CMAKE_GENERATOR=Ninja` in the environment (Git Bash: prefix commands; verify_boot.ps1 sets them itself).
- Unit tests live in the binary target: `cargo test --bin stm32-emulator`; launcher integration tests: `cargo test --test launcher_profile`.
- Old functional signal names (`map`, `tps`, `clt`, `iat`, `crank`, `cam`, `inj1`, `ign1` as a pin alias) are **replaced**, not aliased. `vbatt` keeps its name. (`ign1` survives but as harness position PD4, same pin as before.)
- Harness name → pin mapping is fixed by the spec; `din5`=PE13 and `din6`=PE15 (deliberately not ascending pin order — matches PROTEUS_DIGITAL_5/6).
- OpenWolf bookkeeping: after each task, append a line to `.wolf/memory.md`; update `.wolf/anatomy.md` when files are created.

---

### Task 1: Move Proteus F7 board data into `src/launcher/boards/proteus_f7.rs` (pure move, no behavior change)

**Files:**
- Create: `src/launcher/boards/mod.rs`
- Create: `src/launcher/boards/proteus_f7.rs`
- Modify: `src/launcher/mod.rs` (delete `PROTEUS_F7_REGIONS`/`PROTEUS_F7_PATCHES`/`PROTEUS_F7_ECU_IO_PINS`/`PROTEUS_F7_ECU_IO_ADC_CHANNELS`/`PROTEUS_F7_PROFILE` at lines ~306–392; declare `mod boards;`; repoint `KnownVariant::proteus_f7()` at line ~49)

**Interfaces:**
- Consumes: `ProfileTemplate`, `MemoryRegion`, `MemoryPatch`, `EcuIoPin`, `EcuIoAdcChannel`, `EcuIoDevice`, `UsbCdcTcpDevice`, `LauncherCpuModel` — all already defined in `src/launcher/mod.rs`. They are private or pub items of the `launcher` module; a child module reaches them via `use super::...` (Rust private items are visible to descendant modules — no visibility changes needed on them).
- Produces: `boards::proteus_f7::PROFILE` — `pub(crate) static`-like const of type `ProfileTemplate`, the only item `mod.rs` needs.

This is a refactor: tests stay green throughout; no TDD cycle, but run the suite before and after.

- [ ] **Step 1: Run the launcher tests to confirm a green baseline**

Run: `cargo test --test launcher_profile`
Expected: all pass (currently 6+ tests).

- [ ] **Step 2: Create `src/launcher/boards/mod.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-board launcher profile data. One module per supported board:
//! adding a board means adding a file here plus a `KnownVariant`
//! constructor in the parent module -- board data never lives inline
//! in shared launcher code.

pub(crate) mod proteus_f7;
```

- [ ] **Step 3: Create `src/launcher/boards/proteus_f7.rs` with the existing tables moved verbatim**

The constants are renamed to drop the `PROTEUS_F7_` prefix (the module path now carries it). Copy the region/patch contents exactly as they appear in `src/launcher/mod.rs:306-392` today:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later

//! Proteus F7 (STM32F767) launcher profile: memory map, boot patches,
//! and the ecu_io harness signal tables.

use super::{
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
```

- [ ] **Step 4: Update `src/launcher/mod.rs`**

Near the top, next to the existing module declarations (`pub mod process;` etc.), add:

```rust
mod boards;
```

In `KnownVariant::proteus_f7()` (line ~49), change:

```rust
            profile: Some(&PROTEUS_F7_PROFILE),
```

to:

```rust
            profile: Some(&boards::proteus_f7::PROFILE),
```

Delete the five moved constants (`PROTEUS_F7_REGIONS`, `PROTEUS_F7_PATCHES`, `PROTEUS_F7_ECU_IO_PINS`, `PROTEUS_F7_ECU_IO_ADC_CHANNELS`, `PROTEUS_F7_PROFILE`) from `mod.rs`. Leave `ProfileTemplate` and all the device/region types where they are.

- [ ] **Step 5: Verify no behavior change**

Run: `cargo test --test launcher_profile && cargo test --bin stm32-emulator`
Expected: everything passes, zero test edits. `cargo build` warns about nothing new.

- [ ] **Step 6: Update `.wolf/anatomy.md`** — add entries for `src/launcher/boards/mod.rs` and `src/launcher/boards/proteus_f7.rs` under the launcher section; append a `.wolf/memory.md` line.

- [ ] **Step 7: Commit**

```bash
git add src/launcher/boards/ src/launcher/mod.rs .wolf/anatomy.md .wolf/memory.md
git commit -m "refactor: move Proteus F7 board data into a per-board launcher module"
```

---

### Task 2: Grow the launcher tables to the full harness map (TDD)

**Files:**
- Modify: `tests/launcher_profile.rs` (test `proteus_f7_yaml_includes_the_usb_cdc_tcp_and_ecu_io_devices`, lines ~68–86)
- Modify: `src/launcher/boards/proteus_f7.rs` (the `ECU_IO_PINS` / `ECU_IO_ADC_CHANNELS` tables from Task 1)

**Interfaces:**
- Consumes: `boards::proteus_f7::PROFILE` and the YAML generation path (`ResolvedProfile::to_yaml`) from Task 1 — unchanged signatures.
- Produces: the canonical harness signal tables later tasks copy into YAML (Task 3) and docs (Task 4). Group counts: 8 inputs, 32 outputs, 16 ADC channels.

- [ ] **Step 1: Rewrite the failing test**

Replace the body of `proteus_f7_yaml_includes_the_usb_cdc_tcp_and_ecu_io_devices` in `tests/launcher_profile.rs` with:

```rust
#[test]
fn proteus_f7_yaml_includes_the_usb_cdc_tcp_and_ecu_io_devices() {
    let profile = ResolvedProfile::for_variant(
        KnownVariant::proteus_f7(),
        PathBuf::from("rusefi.bin"),
        PathBuf::from("STM32F767.svd"),
    )
    .unwrap();

    let yaml = profile.to_yaml().unwrap();
    assert!(yaml.contains("usb_cdc_tcp"));
    assert!(yaml.contains("peripheral: OTG_FS_GLOBAL"));
    assert!(yaml.contains("127.0.0.1:29000"));
    assert!(yaml.contains("max_buffered_bytes: 65536"));
    assert!(yaml.contains("ecu_io"));
    assert!(yaml.contains("127.0.0.1:29002"));

    // The full Proteus harness map: 8 digital inputs, 32 observed
    // outputs, 16 ADC channels (see
    // docs/superpowers/specs/2026-07-15-proteus-harness-io-design.md).
    assert_eq!(yaml.matches("direction: input").count(), 8);
    assert_eq!(yaml.matches("direction: output").count(), 32);
    assert_eq!(yaml.matches("name: av").count(), 11);
    assert_eq!(yaml.matches("name: at").count(), 4);
    assert!(yaml.contains("name: vbatt"));

    // Spot-check group boundaries against proteus_meta.h.
    assert!(yaml.contains("name: ls16"));
    assert!(yaml.contains("name: ign12"));
    assert!(yaml.contains("name: hs4"));
    assert!(yaml.contains("name: din6"));
    assert!(yaml.contains("name: vr1"));
    assert!(yaml.contains("pin: PE15")); // din6
    assert!(yaml.contains("pin: PG2")); // ign12
    assert!(!yaml.contains("name: crank")); // old functional names are gone
    assert!(!yaml.contains("name: map"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test launcher_profile proteus_f7_yaml_includes -- --nocapture`
Expected: FAIL on `yaml.matches("direction: input").count()` (2 != 8) — the old 4-pin/5-channel tables are still in place.

- [ ] **Step 3: Replace the two tables in `src/launcher/boards/proteus_f7.rs`**

```rust
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
```

(The array lengths in the type annotations change from 4→40 and 5→16.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test launcher_profile`
Expected: PASS, all tests in the file.

- [ ] **Step 5: Run the full suite**

Run: `cargo test --bin stm32-emulator && cargo test --test launcher_state && cargo test --test launcher_workspace`
Expected: PASS everywhere — nothing else consumes the tables.

- [ ] **Step 6: Commit**

```bash
git add tests/launcher_profile.rs src/launcher/boards/proteus_f7.rs
git commit -m "feat: full Proteus F7 harness signal map in the launcher profile"
```

---

### Task 3: Full harness map in `proteus_f7/config.yaml` + smoke-script contract

**Files:**
- Modify: `proteus_f7/config.yaml` (the `ecu_io:` block under `devices:`)
- Modify: `proteus_f7/verify_boot.ps1` (the config-contract line list, lines 25–27)

**Interfaces:**
- Consumes: the signal tables fixed in Task 2 — the YAML must match them name-for-name, pin-for-pin.
- Produces: the runtime config the live verification (Task 5) runs against.

- [ ] **Step 1: Replace the `ecu_io` block in `proteus_f7/config.yaml`**

Keep the `usb_cdc_tcp` entry untouched. Replace the `ecu_io:` list entry with (preserving the existing inline-map style):

```yaml
  ecu_io:
    - listen: 127.0.0.1:29002
      # Full Proteus harness map, named by connector position (see
      # docs/superpowers/specs/2026-07-15-proteus-harness-io-design.md;
      # source: proteus_meta.h). din5/din6 are PE13/PE15 by design.
      pins:
        - { name: vr1,   pin: PE7,  direction: input }
        - { name: vr2,   pin: PE8,  direction: input }
        - { name: din1,  pin: PC6,  direction: input }
        - { name: din2,  pin: PE11, direction: input }
        - { name: din3,  pin: PE12, direction: input }
        - { name: din4,  pin: PE14, direction: input }
        - { name: din5,  pin: PE13, direction: input }
        - { name: din6,  pin: PE15, direction: input }
        - { name: ls1,   pin: PD7,  direction: output }
        - { name: ls2,   pin: PG9,  direction: output }
        - { name: ls3,   pin: PG10, direction: output }
        - { name: ls4,   pin: PG11, direction: output }
        - { name: ls5,   pin: PG12, direction: output }
        - { name: ls6,   pin: PG13, direction: output }
        - { name: ls7,   pin: PG14, direction: output }
        - { name: ls8,   pin: PB4,  direction: output }
        - { name: ls9,   pin: PB5,  direction: output }
        - { name: ls10,  pin: PB6,  direction: output }
        - { name: ls11,  pin: PB7,  direction: output }
        - { name: ls12,  pin: PB8,  direction: output }
        - { name: ls13,  pin: PB9,  direction: output }
        - { name: ls14,  pin: PE0,  direction: output }
        - { name: ls15,  pin: PE1,  direction: output }
        - { name: ls16,  pin: PE2,  direction: output }
        - { name: hs1,   pin: PA9,  direction: output }
        - { name: hs2,   pin: PA8,  direction: output }
        - { name: hs3,   pin: PD15, direction: output }
        - { name: hs4,   pin: PD14, direction: output }
        - { name: ign1,  pin: PD4,  direction: output }
        - { name: ign2,  pin: PD3,  direction: output }
        - { name: ign3,  pin: PC9,  direction: output }
        - { name: ign4,  pin: PC8,  direction: output }
        - { name: ign5,  pin: PC7,  direction: output }
        - { name: ign6,  pin: PG8,  direction: output }
        - { name: ign7,  pin: PG7,  direction: output }
        - { name: ign8,  pin: PG6,  direction: output }
        - { name: ign9,  pin: PG5,  direction: output }
        - { name: ign10, pin: PG4,  direction: output }
        - { name: ign11, pin: PG3,  direction: output }
        - { name: ign12, pin: PG2,  direction: output }
      adc_channels:
        - { name: at1,   pin: PC4 }
        - { name: at2,   pin: PC5 }
        - { name: at3,   pin: PB0 }
        - { name: at4,   pin: PB1 }
        - { name: av1,   pin: PC0 }
        - { name: av2,   pin: PC1 }
        - { name: av3,   pin: PC2 }
        - { name: av4,   pin: PC3 }
        - { name: av5,   pin: PA0 }
        - { name: av6,   pin: PA1 }
        - { name: av7,   pin: PA2 }
        - { name: av8,   pin: PA3 }
        - { name: av9,   pin: PA4 }
        - { name: av10,  pin: PA5 }
        - { name: av11,  pin: PA6 }
        - { name: vbatt, pin: PA7 }
```

- [ ] **Step 2: Update the config contract in `proteus_f7/verify_boot.ps1`**

Replace the last contract line `'name: crank'` with harness-map sentinels:

```powershell
    'ecu_io:',
    'listen: 127.0.0.1:29002',
    'name: din1',
    'name: ls16',
    'name: ign12',
    'name: av11',
    'name: vbatt'
```

(Note the inline-map YAML style means the script's `.Contains()` check matches `name: din1,` — a substring — so the exact strings above still match.)

- [ ] **Step 3: Run the smoke test**

Run (PowerShell): `.\proteus_f7\verify_boot.ps1`
Expected: `Proteus F7 boot harness verified.` — this also proves the emulator parses the grown config (the one-instruction run deserializes it).

- [ ] **Step 4: Commit**

```bash
git add proteus_f7/config.yaml proteus_f7/verify_boot.ps1
git commit -m "feat: route the full Proteus F7 harness through ecu_io in the example config"
```

---

### Task 4: Update `docs/proteus-f7-ecu-io.md` to the harness naming

**Files:**
- Modify: `docs/proteus-f7-ecu-io.md` (the "Protocol" and "Current signal set" sections; add a migration note)

**Interfaces:**
- Consumes: the final signal map from Tasks 2–3.
- Produces: user-facing protocol documentation Task 5's verification references.

- [ ] **Step 1: Rewrite the "Protocol" example names and the "Current signal set" section**

In "Protocol", replace the parenthesized example names: digital inputs become (`vr1`, `vr2`, `din1`–`din6`), ADC channels become (`at1`–`at4`, `av1`–`av11`, `vbatt`), output pins become (`ls1`–`ls16`, `hs1`–`hs4`, `ign1`–`ign12`).

Replace the whole "Current signal set" section with:

```markdown
## Current signal set

Signals are named by physical Proteus harness position, not tune role —
the tune decides which position carries MAP or injector 3, exactly like
wiring a real harness. Source: `proteus_meta.h` (epicefi_fw).

| Group | Names | Pins (in order) |
|---|---|---|
| Lowside outputs | `ls1`–`ls16` | PD7, PG9–PG14, PB4–PB9, PE0–PE2 |
| Highside outputs | `hs1`–`hs4` | PA9, PA8, PD15, PD14 |
| Ignition outputs | `ign1`–`ign12` | PD4, PD3, PC9, PC8, PC7, PG8–PG2 |
| Analog Temp inputs | `at1`–`at4` | PC4, PC5, PB0, PB1 |
| Analog Volt inputs | `av1`–`av11` | PC0–PC3, PA0–PA6 |
| Battery sense | `vbatt` | PA7 |
| VR trigger inputs | `vr1`, `vr2` | PE7, PE8 |
| Digital inputs | `din1`–`din6` | PC6, PE11, PE12, PE14, PE13, PE15 |

Note `din5`/`din6` are PE13/PE15 in that order (PROTEUS_DIGITAL_5/6).

Migration from the pre-2026-07-15 functional names: `map`→`av1`,
`tps`→`av2`, `clt`→`at3`, `iat`→`at2`, `crank`→`din1`, `cam`→`din2`,
`inj1`→`ls1`; `vbatt` and `ign1` are unchanged (as names — `ign1` now
means harness ignition position 1, still PD4).

Only GPIO-driven output writes are observable; firmware functions routed
through hardware timer PWM (idle, boost) will not produce events until a
TIM output model exists.
```

- [ ] **Step 2: Refresh the stale liveness caveat**

The "What could not be confirmed live" paragraph predates the 2026-07-15 DMA transfer-complete delay fix. Prepend one sentence to that paragraph:

```markdown
**Historical note (pre-2026-07-15):** the capture below predates the DMA
transfer-complete delay fix; since that fix, firmware's slow-ADC loop runs
continuously (ADC1 is read constantly) and boot reaches `usbStart()`, so
the "ADC1 was never read" observation below no longer holds.
```

- [ ] **Step 3: Commit**

```bash
git add docs/proteus-f7-ecu-io.md
git commit -m "docs: document the full Proteus harness signal map for ecu_io"
```

---

### Task 5: Live end-to-end verification

**Files:**
- No repo changes expected; results logged to `.wolf/memory.md`. Scratch feeder script goes in the session scratchpad directory, not the repo.

**Interfaces:**
- Consumes: the built emulator + `proteus_f7/config.yaml` from Task 3.
- Produces: evidence the full map loads and routes — recorded, not committed.

- [ ] **Step 1: Build and launch**

```bash
CMAKE_POLICY_VERSION_MINIMUM=3.5 CMAKE_GENERATOR=Ninja cargo build --release --bin stm32-emulator
cd proteus_f7 && ../target/release/stm32-emulator config.yaml -v -m 400000000 > <scratchpad>/harness_verify.log 2>&1
```

(Run in background; 400M instructions comfortably passes `usbStart()` at ~57M.)

- [ ] **Step 2: Feed signals over the bridge while it runs**

Write `<scratchpad>/feed_harness.py`:

```python
import socket, time

s = socket.create_connection(("127.0.0.1", 29002), timeout=5)
lines = [b"av1=1500\n", b"at3=2000\n", b"vbatt=2100\n", b"din1=1\n", b"din1=0\n",
         b"bogus=1\n"]  # last one must be logged-and-ignored, not fatal
for line in lines:
    s.sendall(line)
    time.sleep(0.5)
time.sleep(3)
s.close()
```

Run: `python <scratchpad>/feed_harness.py`
Expected: exits cleanly (no connection reset).

- [ ] **Step 3: Check the emulator log**

Run: `grep -E "ECU IO" <scratchpad>/harness_verify.log`
Expected: `ECU IO client connected from 127.0.0.1:...` then `ECU IO client disconnected`; a warning-level line for the unknown `bogus` name; no errors or panics. Also confirm `usbStart` still fires: `grep "usbStart" <scratchpad>/harness_verify.log` shows the otg-instr marker.

- [ ] **Step 4: Optional human check (TunerStudio)**

With the emulator still running and TunerStudio attached to :29000, feed `av1=1500` and watch the gauge mapped to Analog Volt 1 (MAP in the stock Proteus tune) move. This step needs the human's TunerStudio session — report the automated evidence and ask them to confirm the gauge.

- [ ] **Step 5: Record the outcome**

Append the verification result (pass/fail, log excerpts) to `.wolf/memory.md`. If any signal fails to route, log it to `.wolf/buglog.json` before fixing.

---

## Self-Review Notes

- Spec coverage: naming table → Tasks 2/3/4; boards module split → Task 1; verification (launcher tests, smoke script, live feed) → Tasks 2/3/5; migration map → Task 4. Non-goals need no tasks.
- Type consistency: `EcuIoPin { name, pin, direction }` / `EcuIoAdcChannel { name, pin }` match `src/launcher/mod.rs:131-141`; `PROFILE` path `boards::proteus_f7::PROFILE` used consistently in Tasks 1–2.
- Pin/count cross-check against `proteus_meta.h`: 16 LS + 4 HS + 12 IGN = 32 outputs; 2 VR + 6 DIN = 8 inputs; 4 AT + 11 AV + vbatt = 16 ADC channels covering EFI_ADC_0–15 exactly.
