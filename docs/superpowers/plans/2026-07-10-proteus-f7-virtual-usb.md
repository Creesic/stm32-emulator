# Proteus F7 Virtual USB Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Run Proteus F7 firmware’s real USB CDC stack against an STM32F767 OTG-FS device model and expose CDC bytes through one loopback TCP client.

**Architecture:** Retain the observed FLASH ACR write first so the firmware reaches USB initialization. OtgFs models only trace-proven device-mode DWC2 registers, FIFOs, and IRQ state. UsbCdcTcp owns a nonblocking TCP listener; a deterministic virtual host drives reset, enumeration, and CDC control transfers through endpoint zero.

**Tech Stack:** Rust 2021, Unicorn hooks, SVD-driven registration, standard-library TCP, serde YAML, existing NVIC.

## Global Constraints

- Keep firmware unmodified: no binary patches or ChibiOS memory injection.
- Model only device-mode OTG-FS behavior observed in trace; unknown registers remain trace-visible.
- Use raw binary TCP, one client, default listener 127.0.0.1:29000.
- Do not add Windows USB/COM drivers, USB HS, USB host mode, HID, mass storage, or physical USB passthrough.
- Use CMAKE_POLICY_VERSION_MINIMUM=3.5, CMAKE_GENERATOR=Ninja, and the existing compatibility target directory for Cargo on Windows.

---

## File Structure

- Modify: src/peripherals/mod.rs — register FLASH and OTG-FS by SVD name.
- Add: src/peripherals/flash.rs — FLASH_ACR state.
- Add: src/peripherals/otg_fs.rs — OTG-FS device state, FIFOs, and IRQs.
- Modify: src/ext_devices/mod.rs — USB CDC TCP configuration, ownership, and polling.
- Add: src/ext_devices/usb_cdc_tcp.rs — one-client nonblocking TCP bridge.
- Modify: src/emulator.rs — pump external sockets from the code hook.
- Modify: proteus_f7/config.yaml and verify_boot.ps1 — configure and verify Proteus USB.
- Add: proteus_f7/usb_trace_notes.md and docs/proteus-f7-usb.md — evidence and operation notes.
- Add: tests/usb_cdc_tcp.rs — socket bridge tests.

### Task 1: Preserve FLASH ACR and capture the first real OTG-FS access

**Files:**
- Create: src/peripherals/flash.rs
- Modify: src/peripherals/mod.rs
- Modify: proteus_f7/verify_boot.ps1
- Create: proteus_f7/usb_trace_notes.md

**Interfaces:**
- Produces Flash::acr_after_write(value: u32) -> u32.
- Produces Flash::new(name: &str) -> Option<Box<dyn Peripheral>>.

- [ ] **Step 1: Write failing FLASH tests in src/peripherals/mod.rs**

    #[test]
    fn flash_acr_retains_latency_and_cache_bits() {
        assert_eq!(
            crate::peripherals::flash::Flash::acr_after_write(0x0000_0707),
            0x0000_0707
        );
    }

- [ ] **Step 2: Verify the test fails**

    Run: cargo test flash_acr_retains_latency_and_cache_bits
    Expected: FAIL because peripherals::flash does not exist.

- [ ] **Step 3: Implement the minimal register**

    pub struct Flash { acr: u32 }

    impl Flash {
        pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
            (name == "FLASH").then(|| Box::new(Self { acr: 0 }) as Box<dyn Peripheral>)
        }

        pub(crate) fn acr_after_write(value: u32) -> u32 { value }
    }

    Read and write acr at offset 0x0000, then register Flash::new before the
    generic fallback in Peripherals::register_peripheral.

- [ ] **Step 4: Verify the register and the boot harness**

    Run: cargo test flash_acr_retains_latency_and_cache_bits
    Expected: PASS.

    Add a 500000-instruction bounded trace to verify_boot.ps1. Require a
    firmware-time access in 0x50000000 through 0x50000fff, then run:

    .\proteus_f7\verify_boot.ps1

    Expected: the old FLASH_ACR wait is absent; the new OTG assertion fails
    until OTG-FS is implemented.

- [ ] **Step 5: Record trace evidence**

    Run from proteus_f7:

    cargo run --release --bin stm32-emulator -- config.yaml --max-instructions 500000 --color never -vvvv

    Record the OTG-FS SVD name, first MMIO offsets, IRQ number, endpoint-zero
    registers, configuration value, CDC control requests, and bulk endpoint
    addresses in usb_trace_notes.md. Record only observed values.

- [ ] **Step 6: Commit**

    git add src/peripherals/flash.rs src/peripherals/mod.rs proteus_f7/verify_boot.ps1 proteus_f7/usb_trace_notes.md
    git commit -m "feat: retain Proteus F7 flash latency configuration"

### Task 2: Implement and test the TCP CDC bridge

**Files:**
- Create: src/ext_devices/usb_cdc_tcp.rs
- Modify: src/ext_devices/mod.rs
- Create: tests/usb_cdc_tcp.rs

**Interfaces:**
- Produces UsbCdcTcpConfig with peripheral, listen, and max_buffered_bytes.
- Produces UsbCdcTcp::new(config), poll(), local_addr(), push_from_device(bytes), and take_for_device(maximum).
- Produces ExtDevices::poll() for the emulator hook.

- [ ] **Step 1: Write the failing loopback test**

    #[test]
    fn loopback_client_exchanges_binary_bytes() {
        let mut bridge = UsbCdcTcp::new(UsbCdcTcpConfig {
            peripheral: "OTG_FS_GLOBAL".to_owned(),
            listen: "127.0.0.1:0".to_owned(),
            max_buffered_bytes: 64,
        }).unwrap();
        let mut client = TcpStream::connect(bridge.local_addr().unwrap()).unwrap();
        bridge.poll();
        bridge.push_from_device(&[0x00, 0xff, 0x42]);
        bridge.poll();
        let mut received = [0; 3];
        client.read_exact(&mut received).unwrap();
        assert_eq!(received, [0x00, 0xff, 0x42]);
        client.write_all(&[0x10, 0x20]).unwrap();
        bridge.poll();
        assert_eq!(bridge.take_for_device(64), vec![0x10, 0x20]);
    }

- [ ] **Step 2: Verify failure**

    Run: cargo test --test usb_cdc_tcp loopback_client_exchanges_binary_bytes
    Expected: FAIL because UsbCdcTcp does not exist.

- [ ] **Step 3: Implement the nonblocking bridge**

    Bind a nonblocking TcpListener. Accept only with no active client, set the
    client stream nonblocking, and use capped VecDeque byte queues in both
    directions. Read and write until WouldBlock. On a second client, accept
    then immediately drop it. On disconnect, clear only that client and keep
    the listener open.

- [ ] **Step 4: Add edge tests and wire ExtDevices**

    Add tests for one-client enforcement, disconnect recovery, and cap
    behavior that drops oldest queued bytes. Add usb_cdc_tcp to ExtDevicesConfig
    and construct Rc<RefCell<UsbCdcTcp>> values in into_ext_devices. Implement
    ExtDevices::poll by polling every configured bridge.

- [ ] **Step 5: Verify and commit**

    Run: cargo test --test usb_cdc_tcp
    Expected: PASS.

    git add src/ext_devices/mod.rs src/ext_devices/usb_cdc_tcp.rs tests/usb_cdc_tcp.rs
    git commit -m "feat: add loopback TCP CDC bridge"

### Task 3: Record trace evidence and model the DWC2 global/device/endpoint registers

**Ground truth used by this task:** `OtgFs::new` currently matches only the
SVD name `"OTG_FS_GLOBAL"` (src/peripherals/otg_fs.rs:34), but
`Peripherals::modeled_range("OTG_FS_GLOBAL", base, size)`
(src/peripherals/mod.rs:344) already widens that single peripheral's claimed
address span to `(base, base + 0x2000)`. Because `Peripherals::get_peripheral`
picks a peripheral by `start <= addr <= end` (src/peripherals/mod.rs:328-339),
every OTG_FS_DEVICE- and OTG_FS_PWRCLK-named register in the SVD/trace — and
FIFO index 0 — already lands inside the *same* `OtgFs` instance, at an
`offset` equal to `addr - 0x5000_0000`. That offset is exactly the
`stm32_otg_t` struct offset from ChibiOS's own
`firmware/ChibiOS/os/hal/ports/STM32/LLD/OTGv1/stm32_otg.h` (read in full
during planning). This task uses those struct offsets directly and ignores
the SVD's per-sub-block register names, two of which are proven wrong by the
trace: SVD calls struct offset `0x800` (`DCFG`) "CTL", and struct offset
`0x810` (`DIEPMSK`) "TSIZ" — confirmed by cross-referencing
`proteus_f7/usb-plan-evidence.log:6159-6160` (device-relative offset 0x0004,
named `DCTL` in the trace, is the true `DCTL`) against
`proteus_f7/usb-plan-evidence.log:10644-10645` (device-relative offset 0x0010,
traced as `TSIZ`, writes `0x00000000` then later `0x00000009` — a mask value,
not a transfer size, so it is `DIEPMSK`).

**Files:**
- Create: proteus_f7/usb_trace_notes.md
- Modify: src/peripherals/otg_fs.rs
- Modify: src/peripherals/mod.rs

**Interfaces:**
- Produces `OtgFs::new(name: &str, ext_devices: &ExtDevices) -> Option<Box<dyn Peripheral>>` (unchanged signature).
- Produces `OtgFs::for_test() -> Self` (unchanged signature).
- Produces `OtgFs::interrupt_pending(&self) -> bool`, `virtual_host_reset(&mut self)` (both already exist, behavior extended).
- Produces private helpers `raise_in_endpoint_interrupt(&mut self, ep: usize, bits: u32)`, `raise_out_endpoint_interrupt(&mut self, ep: usize, bits: u32)`, `push_tx_fifo_word(&mut self, ep: usize, value: u32)`, and `complete_in_transfer(&mut self, ep: usize)` — private is sufficient since only `otg_fs.rs` itself (including its own `mod tests`, which can see a parent module's private items) calls them; Tasks 4 and 5 extend `complete_in_transfer`'s body in place.
- Consumes nothing new from `ExtDevices` in this task.

- [ ] **Step 1: Write proteus_f7/usb_trace_notes.md from the already-captured trace**

    Create the file with this content (transcribed from
    `proteus_f7/usb-plan-evidence.log`, captured earlier this session by
    connecting one TCP client to the running emulator):

    ```markdown
    # Proteus F7 OTG-FS trace evidence

    Peripheral SVD name: `OTG_FS_GLOBAL` (base 0x5000_0000). IRQ 67
    (`OTG_FS_IRQn` from STM32F767.svd), already wired via
    `sys.p.nvic.borrow_mut().set_intr_pending(67)` in `OtgFs::poll`.

    `OTG_FS_DEVICE` (base 0x5000_0800), `OTG_FS_HOST` (base 0x5000_0400), and
    `OTG_FS_PWRCLK` (base 0x5000_0e00) are separate SVD peripherals but are
    NOT separately registered: `Peripherals::modeled_range` widens
    `OTG_FS_GLOBAL`'s claimed range to `(0x5000_0000, 0x5000_2000)`, so all
    four SVD peripherals' addresses, plus FIFO index 0 (`0x5000_1000`-
    `0x5000_1fff`), dispatch to the single `OtgFs` instance. The `offset`
    `OtgFs::read`/`write` receives is `addr - 0x5000_0000`, i.e. the
    `stm32_otg_t` struct offset from ChibiOS's
    `os/hal/ports/STM32/LLD/OTGv1/stm32_otg.h`.

    Two SVD register names are wrong; this project uses the struct-offset
    names instead:
    - offset 0x800 (SVD: "CTL") is `DCFG`.
    - offset 0x810 (SVD: "TSIZ") is `DIEPMSK`.

    ## Observed reset sequence (proteus_f7/usb-plan-evidence.log:10597-13469)

    Firmware boot-time OTG-FS init (clk=57044973 onward), before any virtual
    host activity:
    1. write GUSBCFG(0x00c)=0x40001440
    2. write DCFG(0x800)=0x02200003
    3. write PCGCCTL(0xe00)=0x00000000
    4. write GOTGCTL(0x000)=0x000000c0
    5. write GCCFG(0x038)=0x00010000
    6. read GRSTCTL(0x010)=0x80000000; write GRSTCTL=0x00000001 (core soft reset, self-clears)
    7. write GAHBCFG(0x008)=0x00000000
    8. for ep in 0..=5: read ie[ep].DIEPCTL(0x900+ep*0x20)=0; read oe[ep].DOEPCTL(0xb00+ep*0x20)=0; write ie[ep].DIEPINT(0x908+ep*0x20)=0xffffffff; write oe[ep].DOEPINT(0xb08+ep*0x20)=0xffffffff (W1C boilerplate over all 6 endpoint pairs)
    9. write DAINTMSK(0x81c)=0x00010001 (enable EP0 IN + EP0 OUT)
    10. write DIEPMSK(0x810)=0x00000000; write DOEPMSK(0x814)=0x00000000; write DAINTMSK(0x81c)=0x00000000
    11. write GINTMSK(0x018)=0xc0303c08
    12. write GINTSTS(0x014)=0xffffffff (clear all, W1C)
    13. read GAHBCFG=0; write GAHBCFG=0x00000001 (global interrupt enable)
    14. read DCTL(0x804)=0x00000002; write DCTL=0x00000000

    ## Observed virtual-host-reset response (clk=97910816 onward, after `virtual_host_reset()` sets USB_RESET and TCP client connects)

    1. read GINTSTS=0x00001000 (USB_RESET); read GINTMSK=0xc0303c08; write GINTSTS=0x00001000 (W1C ack)
    2. write GRSTCTL=0x00000020 (TXFFLSH); read GRSTCTL=0x80000000 (self-cleared)
    3. write DIEPEMPMSK(0x834)=0x00000000
    4. write DAINTMSK(0x81c)=0x00010001
    5. for ep in 0..=5: write ie[ep].DIEPCTL=0x08000000; write oe[ep].DOEPCTL=0x08000000; write ie[ep].DIEPINT=0xffffffff; write oe[ep].DOEPINT=0xffffffff
    6. write GRXFSIZ(0x024)=0x00000080 (RX FIFO depth 128 words)
    7. write GRSTCTL=0x00000010 (RXFFLSH); read GRSTCTL=0x80000000 (self-cleared)
    8. read DCFG=0x02200003; write DCFG=0x02200003 (unchanged)
    9. read GINTMSK=0xc0303c08; write GINTMSK=0xc03c3c18 (adds RXFLVL, IEPINT, OEPINT)
    10. write DIEPMSK(0x810)=0x00000009 (XFRCM|TOCM); write DOEPMSK(0x814)=0x00000009 (XFRCM|STUPM)
    11. write oe[0].DOEPTSIZ(0xb10)=0x60000000 (STUPCNT=3)
    12. write oe[0].DOEPCTL(0xb00)=0x10008040 (USBAEP|MPSIZ=64, EP0 OUT active)
    13. write ie[0].DIEPTSIZ(0x910)=0x00000000
    14. write ie[0].DIEPCTL(0x900)=0x10008040 (USBAEP|MPSIZ=64, EP0 IN active)
    15. write DIEPTXF0(0x028)=0x00100080 (TX FIFO0: depth 16 words, start word 0x80)
    16. idle: read GINTSTS=0; read GINTMSK=0xc03c3c18; write GINTSTS=0 (no-op)
    17. read ie[0].DTXFSTS(0x918)=0x00000000 — firmware is now polling for EP0 IN FIFO space before sending its first response; the trace ends here because no SETUP packet has been delivered yet (Task 4 adds that).
    ```

- [ ] **Step 2: Write failing controller tests in src/peripherals/otg_fs.rs**

    Add to the existing `#[cfg(test)] mod tests` block:

    ```rust
    #[test]
    fn device_control_register_retains_written_value() {
        let mut otg = OtgFs::for_test();
        otg.register_write(OtgFs::DCTL, 0x0000_0002);
        assert_eq!(otg.register_read(OtgFs::DCTL), 0x0000_0002);
    }

    #[test]
    fn endpoint_zero_and_endpoint_one_in_control_registers_are_independent() {
        let mut otg = OtgFs::for_test();
        otg.register_write(OtgFs::DIEP_BASE, 0x1000_8040);
        otg.register_write(OtgFs::DIEP_BASE + OtgFs::EP_STRIDE, 0x0800_0000);
        assert_eq!(otg.register_read(OtgFs::DIEP_BASE), 0x1000_8040);
        assert_eq!(
            otg.register_read(OtgFs::DIEP_BASE + OtgFs::EP_STRIDE),
            0x0800_0000
        );
    }

    #[test]
    fn endpoint_interrupt_bits_clear_on_write_one() {
        let mut otg = OtgFs::for_test();
        otg.raise_in_endpoint_interrupt(0, 0xffff_ffff);
        otg.register_write(OtgFs::DIEP_BASE + OtgFs::EP_INT_OFFSET, 0xffff_ffff);
        assert_eq!(
            otg.register_read(OtgFs::DIEP_BASE + OtgFs::EP_INT_OFFSET),
            0
        );
    }

    #[test]
    fn unmasked_endpoint_zero_out_interrupt_raises_oepint_and_global_interrupt() {
        let mut otg = OtgFs::for_test();
        otg.write_global_interrupt_mask(OtgFs::GINTSTS_OEPINT);
        otg.register_write(OtgFs::DAINTMSK, 0x0001_0000);
        otg.raise_out_endpoint_interrupt(0, OtgFs::DOEPINT_XFRC);
        assert!(otg.interrupt_pending());
        assert_eq!(
            otg.register_read(OtgFs::GINTSTS) & OtgFs::GINTSTS_OEPINT,
            OtgFs::GINTSTS_OEPINT
        );
    }
    ```

- [ ] **Step 3: Verify failure**

    Run: `cargo test otg_fs`
    Expected: FAIL — `DCTL`, `DIEP_BASE`, `EP_STRIDE`, `EP_INT_OFFSET`,
    `raise_in_endpoint_interrupt`, `raise_out_endpoint_interrupt`,
    `GINTSTS_OEPINT`, `DOEPINT_XFRC` do not exist yet.

- [ ] **Step 4: Implement the register model**

    Replace the body of `src/peripherals/otg_fs.rs` (keeping the existing
    `GRSTCTL`/`GINTSTS`/`GINTMSK`/`USB_RESET`/`GRSTCTL_*` constants and the
    existing `grstctl_*` tests) by adding:

    ```rust
    // Struct-offset constants from ChibiOS's stm32_otg_t (stm32_otg.h),
    // not the SVD's per-sub-block names — see usb_trace_notes.md for why.
    const DCFG: u32 = 0x0800;
    const DCTL: u32 = 0x0804;
    const DIEPMSK: u32 = 0x0810;
    const DOEPMSK: u32 = 0x0814;
    const DAINT: u32 = 0x0818;
    const DAINTMSK: u32 = 0x081c;
    const DIEPEMPMSK: u32 = 0x0834;

    const NUM_ENDPOINTS: usize = 6;
    const DIEP_BASE: u32 = 0x0900;
    const DOEP_BASE: u32 = 0x0b00;
    const EP_STRIDE: u32 = 0x0020;
    const EP_CTL_OFFSET: u32 = 0x00;
    const EP_INT_OFFSET: u32 = 0x08;
    const EP_TSIZ_OFFSET: u32 = 0x10;
    const DTXFSTS_OFFSET: u32 = 0x18;

    const GINTSTS_IEPINT: u32 = 1 << 18;
    const GINTSTS_OEPINT: u32 = 1 << 19;
    const DOEPINT_XFRC: u32 = 1 << 0;
    ```

    `GOTGCTL` (0x000), `GRXSTSR` (0x01c), `GRXSTSP` (0x020), `GRXFSIZ`
    (0x024), and the FIFO window base (0x1000) are deliberately NOT named
    here even though `usb_trace_notes.md` documents them: `GOTGCTL`/
    `GRXFSIZ` need no special handling beyond the existing generic
    `BTreeMap` passthrough (the trace only ever shows plain write-then-
    read-back), and `GRXSTSR`/`GRXSTSP`/the FIFO window are Task 4's, added
    together with the RX status queue and FIFO storage that give them
    behavior. A constant with no code path using it yet is dead code Rust
    will warn about — Task 4 introduces each one next to its first real use.

    None of the constants above need `pub`: unlike the pre-existing `pub
    const USB_RESET`, which `src/peripherals/mod.rs`'s own separate test
    module references as `crate::peripherals::otg_fs::OtgFs::USB_RESET`,
    every test added across Tasks 3-5 lives in `otg_fs.rs`'s own `mod tests`
    and can already see private items of its parent module.

    Add fields to `OtgFs`:

    ```rust
    #[derive(Default, Clone, Copy)]
    struct EndpointRegs {
        ctl: u32,
        int: u32,
        tsiz: u32,
    }

    pub struct OtgFs {
        bridge: Option<Rc<RefCell<UsbCdcTcp>>>,
        registers: BTreeMap<u32, u32>,
        global_interrupt_status: u32,
        global_interrupt_mask: u32,
        host_attached: bool,
        dcfg: u32,
        dctl: u32,
        diep_mask: u32,
        doep_mask: u32,
        daint_mask: u32,
        diep_empty_mask: u32,
        ep_in: [EndpointRegs; Self::NUM_ENDPOINTS],
        ep_out: [EndpointRegs; Self::NUM_ENDPOINTS],
    }
    ```

    Initialize the new fields to `0`/`[EndpointRegs::default(); Self::NUM_ENDPOINTS]`
    in both `OtgFs::new` and `OtgFs::for_test`.

    Add the endpoint decoder and interrupt-aggregation helpers:

    ```rust
    fn decode_endpoint(base: u32, offset: u32) -> Option<(usize, u32)> {
        if offset < base {
            return None;
        }
        let rel = offset - base;
        let ep = (rel / Self::EP_STRIDE) as usize;
        (ep < Self::NUM_ENDPOINTS).then_some((ep, rel % Self::EP_STRIDE))
    }

    fn daint(&self) -> u32 {
        let mut value = 0;
        for (i, ep) in self.ep_in.iter().enumerate() {
            if ep.int != 0 {
                value |= 1 << i;
            }
        }
        for (i, ep) in self.ep_out.iter().enumerate() {
            if ep.int != 0 {
                value |= 1 << (16 + i);
            }
        }
        value
    }

    fn effective_gintsts(&self) -> u32 {
        let daint = self.daint() & self.daint_mask;
        let mut status = self.global_interrupt_status;
        if daint & 0x0000_ffff != 0 {
            status |= Self::GINTSTS_IEPINT;
        }
        if daint & 0xffff_0000 != 0 {
            status |= Self::GINTSTS_OEPINT;
        }
        status
    }

    fn raise_in_endpoint_interrupt(&mut self, ep: usize, bits: u32) {
        self.ep_in[ep].int |= bits;
    }

    fn raise_out_endpoint_interrupt(&mut self, ep: usize, bits: u32) {
        self.ep_out[ep].int |= bits;
    }
    ```

    (`push_tx_fifo_word` and `complete_in_transfer` are NOT added in this
    task — Task 4 adds both together with the FIFO storage that makes them
    testable, per its own Step 3.)

    Update `interrupt_pending` to use the aggregate:

    ```rust
    pub fn interrupt_pending(&self) -> bool {
        self.effective_gintsts() & self.global_interrupt_mask != 0
    }
    ```

    Rewrite `register_read`/`register_write` to check the endpoint blocks
    first, then the named globals/device registers, then fall back to the
    existing `BTreeMap`:

    ```rust
    fn register_read(&self, offset: u32) -> u32 {
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DIEP_BASE, offset) {
            return match reg {
                Self::EP_CTL_OFFSET => self.ep_in[ep].ctl,
                Self::EP_INT_OFFSET => self.ep_in[ep].int,
                Self::EP_TSIZ_OFFSET => self.ep_in[ep].tsiz,
                Self::DTXFSTS_OFFSET => 0, // FIFO space accounting arrives in Task 4
                _ => 0,
            };
        }
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DOEP_BASE, offset) {
            return match reg {
                Self::EP_CTL_OFFSET => self.ep_out[ep].ctl,
                Self::EP_INT_OFFSET => self.ep_out[ep].int,
                Self::EP_TSIZ_OFFSET => self.ep_out[ep].tsiz,
                _ => 0,
            };
        }
        match offset {
            Self::GRSTCTL => self.registers.get(&offset).copied().unwrap_or(0) | 0x8000_0000,
            Self::GINTSTS => self.effective_gintsts(),
            Self::GINTMSK => self.global_interrupt_mask,
            Self::DCFG => self.dcfg,
            Self::DCTL => self.dctl,
            Self::DIEPMSK => self.diep_mask,
            Self::DOEPMSK => self.doep_mask,
            Self::DAINT => self.daint(),
            Self::DAINTMSK => self.daint_mask,
            Self::DIEPEMPMSK => self.diep_empty_mask,
            _ => self.registers.get(&offset).copied().unwrap_or(0),
        }
    }

    fn register_write(&mut self, offset: u32, value: u32) {
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DIEP_BASE, offset) {
            match reg {
                Self::EP_CTL_OFFSET => self.ep_in[ep].ctl = value,
                Self::EP_INT_OFFSET => self.ep_in[ep].int &= !value,
                Self::EP_TSIZ_OFFSET => self.ep_in[ep].tsiz = value,
                _ => {}
            }
            return;
        }
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DOEP_BASE, offset) {
            match reg {
                Self::EP_CTL_OFFSET => self.ep_out[ep].ctl = value,
                Self::EP_INT_OFFSET => self.ep_out[ep].int &= !value,
                Self::EP_TSIZ_OFFSET => self.ep_out[ep].tsiz = value,
                _ => {}
            }
            return;
        }
        match offset {
            Self::GINTSTS => self.global_interrupt_status &= !value,
            Self::GINTMSK => self.global_interrupt_mask = value,
            Self::GRSTCTL => {
                self.registers
                    .insert(offset, value & !Self::GRSTCTL_SELF_CLEARING);
            }
            Self::DCFG => self.dcfg = value,
            Self::DCTL => self.dctl = value,
            Self::DIEPMSK => self.diep_mask = value,
            Self::DOEPMSK => self.doep_mask = value,
            Self::DAINTMSK => self.daint_mask = value,
            Self::DIEPEMPMSK => self.diep_empty_mask = value,
            _ => {
                self.registers.insert(offset, value);
            }
        }
    }
    ```

- [ ] **Step 5: Widen the modeled range for the endpoints this project uses**

    In `src/peripherals/mod.rs`, `Peripherals::modeled_range` currently
    returns `(base, base + 0x2000)` for `"OTG_FS_GLOBAL"` — enough to reach
    `OTG_FS_DEVICE`/`OTG_FS_PWRCLK` and FIFO index 0, but not FIFO indices
    1-5, which Task 5's bulk endpoint needs. Change it to:

    ```rust
    "OTG_FS_GLOBAL" => (base, base + 0x7000),
    ```

    (`0x7000` = FIFO_BASE 0x1000 + 6 endpoints * FIFO_WINDOW 0x1000, covering
    FIFO indices 0-5.)

- [ ] **Step 6: Verify unit tests pass**

    Run: `cargo test otg_fs`
    Expected: PASS.

- [ ] **Step 7: Verify against the live firmware trace**

    Run from proteus_f7 (background is fine; this repeats the capture
    already recorded in usb_trace_notes.md, now against the new register
    model):

    cargo run --release --bin stm32-emulator -- config.yaml -vvvv --color never > usb-task3-trace.log 2>&1

    Connect one TCP client to 127.0.0.1:29000 to trigger `virtual_host_reset`,
    then check:

    grep "peri=????" usb-task3-trace.log | grep OTG

    Expected: no matches for any address in 0x5000_0000-0x5000_6fff (every
    register the boot + reset sequence touches now has a real name/offset
    reaching `OtgFs`, even though the trace's own debug-peripheral labels
    still show the old SVD names).

    Delete usb-task3-trace.log; it was a verification artifact, not evidence
    (the evidence is already transcribed into usb_trace_notes.md).

- [ ] **Step 8: Commit**

    git add proteus_f7/usb_trace_notes.md src/peripherals/otg_fs.rs src/peripherals/mod.rs
    git commit -m "feat: model OTG-FS global/device/endpoint registers from trace evidence"

### Task 4: Implement the FIFO model and a deterministic virtual host enumeration

**Deviations found during implementation, evidence-driven (see
usb_trace_notes.md's "Enumeration" section for the trace lines):**
- `register_read` must be `&mut self`, not `&self` — popping `GRXSTSP`/the
  FIFO on read is inherently mutating, a real conflict this plan's original
  code sample didn't account for.
- Nothing in this task's original text ever triggers the *first*
  `virtual_host_setup` call — `advance_virtual_host` only fires from an
  EP0 IN completion, but enumeration has to start somehow. The real trigger,
  confirmed by trace: the first time firmware writes `oe[0].DOEPCTL` with
  `USBAEP` (bit 15) newly set (trace-observed value `0x1000_8040`) — not
  `EPENA` (bit 31), which control endpoint 0 never sets for SETUP reception.

**Files:**
- Modify: src/peripherals/otg_fs.rs

**Interfaces:**
- Produces `pub fn virtual_host_setup(&mut self, endpoint: usize, packet: [u8; 8])`.
- Produces `pub fn virtual_host_control_out(&mut self, endpoint: usize, packet: [u8; 8], data: &[u8])`.
- Produces `pub fn read_fifo(&mut self, endpoint: usize) -> u32` (endpoint argument kept for interface stability; always reads the shared RX port per real DWC2 behavior).
- Produces `pub fn is_configured(&self) -> bool` — Task 5 gates the TCP bridge on this.
- Produces private helpers `push_tx_fifo_word`, `complete_in_transfer`, `fifo_endpoint`, `pop_rx_fifo_word` — Task 5 extends `complete_in_transfer` further.
- Consumes `raise_in_endpoint_interrupt`, `raise_out_endpoint_interrupt` from Task 3.

- [ ] **Step 1: Write the failing SETUP/FIFO test from the original design**

    ```rust
    #[test]
    fn endpoint_zero_setup_packet_is_read_from_fifo() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_reset();
        otg.virtual_host_setup(0, [0x80, 0x06, 0x00, 0x01, 0, 0, 18, 0]);
        assert_eq!(otg.read_fifo(0), 0x0100_0680);
    }

    #[test]
    fn firmware_completing_the_device_descriptor_response_advances_to_set_address() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_reset();
        otg.virtual_host_setup(0, OtgFs::get_device_descriptor_packet());
        // Firmware arms an 18-byte IN response and pushes it word-by-word.
        otg.register_write(OtgFs::DIEP_BASE + OtgFs::EP_TSIZ_OFFSET, 18);
        otg.register_write(
            OtgFs::DIEP_BASE + OtgFs::EP_CTL_OFFSET,
            0x1000_8040 | OtgFs::DIEPCTL_EPENA,
        );
        for _ in 0..5 {
            otg.register_write(OtgFs::FIFO_BASE, 0);
        }
        assert!(
            otg.register_read(OtgFs::DIEP_BASE + OtgFs::EP_INT_OFFSET) & OtgFs::DIEPINT_XFRC != 0
        );
        assert_eq!(
            otg.next_setup_request(),
            OtgFs::set_address_packet(OtgFs::VIRTUAL_DEVICE_ADDRESS)
        );
    }
    ```

    Like Task 3's tests, these call the existing private `register_read`/
    `register_write` helpers directly rather than the `Peripheral` trait's
    `read`/`write` methods, so no `System` value is needed at all.
    `DIEPCTL_EPENA`, `get_device_descriptor_packet`, `set_address_packet`,
    `VIRTUAL_DEVICE_ADDRESS`, and `next_setup_request` are all private
    associated items added later in this task (Steps 3-5) — private items
    are visible to a same-file `mod tests` because it is a descendant module,
    the same reason Task 3's tests could already call `register_write`
    directly.

- [ ] **Step 2: Verify failure**

    Run: `cargo test otg_fs`
    Expected: FAIL — `virtual_host_setup`'s signature changed (now takes an
    endpoint), and `get_device_descriptor_packet`/`set_address_packet`/
    `next_setup_request`/`VIRTUAL_DEVICE_ADDRESS`/`DIEPCTL_EPENA` do not
    exist yet.

- [ ] **Step 3: Add FIFO storage and the RX status queue**

    Add fields:

    ```rust
    use std::collections::VecDeque;

    pub struct OtgFs {
        // ...existing fields from Task 3...
        rx_fifo: VecDeque<u8>,
        rx_status: VecDeque<u32>,
        tx_fifo: [VecDeque<u8>; Self::NUM_ENDPOINTS],
        virtual_host_step: VirtualHostStep,
    }

    // Added to the EndpointRegs struct from Task 3 (now ctl/int/tsiz/armed_in_bytes_sent).
    struct EndpointRegs {
        ctl: u32,
        int: u32,
        tsiz: u32,
        armed_in_bytes_sent: u32,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum VirtualHostStep {
        AwaitingDeviceDescriptor,
        AwaitingSetAddressStatus,
        AwaitingSetConfigurationStatus,
        AwaitingSetLineCodingStatus,
        AwaitingSetControlLineStateStatus,
        Configured,
    }
    ```

    Initialize `rx_fifo: VecDeque::new()`, `rx_status: VecDeque::new()`,
    `tx_fifo: Default::default()`, `virtual_host_step:
    VirtualHostStep::AwaitingDeviceDescriptor` in `new` and `for_test`
    (`armed_in_bytes_sent` is already covered by `EndpointRegs`'s existing
    `#[derive(Default)]`).

    Add the FIFO decode/push/pop helpers, the IN-transfer completion
    constants, and the completion-detecting `push_tx_fifo_word`/
    `complete_in_transfer` pair (this is the first task where either is
    exercised by a test, so this is where they're introduced):

    ```rust
    const GRXSTSR: u32 = 0x001c;
    const GRXSTSP: u32 = 0x0020;
    const FIFO_BASE: u32 = 0x1000;
    const FIFO_WINDOW: u32 = 0x1000;

    const GINTSTS_RXFLVL: u32 = 1 << 4;
    const DIEPCTL_EPENA: u32 = 1 << 31;
    const DIEPINT_XFRC: u32 = 1 << 0;
    const DOEPINT_STUP: u32 = 1 << 3;
    const XFRSIZ_MASK: u32 = 0x7_ffff;

    fn fifo_endpoint(offset: u32) -> Option<usize> {
        if offset < Self::FIFO_BASE {
            return None;
        }
        let ep = ((offset - Self::FIFO_BASE) / Self::FIFO_WINDOW) as usize;
        (ep < Self::NUM_ENDPOINTS).then_some(ep)
    }

    fn pop_rx_fifo_word(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        for b in bytes.iter_mut() {
            *b = self.rx_fifo.pop_front().unwrap_or(0);
        }
        u32::from_le_bytes(bytes)
    }

    fn push_tx_fifo_word(&mut self, ep: usize, value: u32) {
        self.tx_fifo[ep].extend(value.to_le_bytes());
        let was_enabled = self.ep_in[ep].ctl & Self::DIEPCTL_EPENA != 0;
        self.ep_in[ep].armed_in_bytes_sent += 4;
        let xfrsiz = self.ep_in[ep].tsiz & Self::XFRSIZ_MASK;
        if was_enabled && self.ep_in[ep].armed_in_bytes_sent >= xfrsiz {
            self.ep_in[ep].ctl &= !Self::DIEPCTL_EPENA;
            self.complete_in_transfer(ep);
        }
    }

    // Step 5 below extends this with virtual-host advancement; Task 5
    // extends it again with bulk-endpoint TCP forwarding. Both call sites
    // (here and in the DIEPCTL write handler) funnel through here so
    // neither later task has to patch more than one method.
    fn complete_in_transfer(&mut self, ep: usize) {
        self.raise_in_endpoint_interrupt(ep, Self::DIEPINT_XFRC);
    }
    ```

    Replace Task 3's plain `Self::EP_CTL_OFFSET => self.ep_in[ep].ctl =
    value,` arm in `register_write` with the arming-aware version (a
    zero-length transfer, `xfrsiz == 0`, completes the instant `EPENA` is
    set, since `push_tx_fifo_word` will never be called for it):

    ```rust
    Self::EP_CTL_OFFSET => {
        let was_enabled = self.ep_in[ep].ctl & Self::DIEPCTL_EPENA != 0;
        self.ep_in[ep].ctl = value;
        let now_enabled = value & Self::DIEPCTL_EPENA != 0;
        if now_enabled && !was_enabled {
            self.ep_in[ep].armed_in_bytes_sent = 0;
            if self.ep_in[ep].tsiz & Self::XFRSIZ_MASK == 0 {
                self.ep_in[ep].ctl &= !Self::DIEPCTL_EPENA;
                self.complete_in_transfer(ep);
            }
        }
    }
    ```

    In `register_read`, before the endpoint-block checks:

    ```rust
    if let Some(ep) = Self::fifo_endpoint(offset) {
        return if ep == 0 { self.pop_rx_fifo_word() } else { 0 };
    }
    ```

    In `register_write`, before the endpoint-block checks:

    ```rust
    if let Some(ep) = Self::fifo_endpoint(offset) {
        self.push_tx_fifo_word(ep, value);
        return;
    }
    ```

    Add `GRXSTSP` and `GRXSTSR` (peek, non-popping) to `register_read`'s
    named-register match:

    ```rust
    Self::GRXSTSP => self.rx_status.pop_front().unwrap_or(0),
    Self::GRXSTSR => self.rx_status.front().copied().unwrap_or(0),
    ```

- [ ] **Step 4: Implement virtual_host_setup, virtual_host_control_out, and read_fifo**

    ```rust
    const RXSTS_SETUP_DATA: u32 = 6 << 17;
    const RXSTS_SETUP_COMP: u32 = 4 << 17;
    const RXSTS_OUT_DATA: u32 = 2 << 17;
    const RXSTS_OUT_COMP: u32 = 3 << 17;

    fn rx_status_word(pktsts: u32, byte_count: u32, endpoint: usize) -> u32 {
        pktsts | (byte_count << 4) | endpoint as u32
    }

    pub fn virtual_host_setup(&mut self, endpoint: usize, packet: [u8; 8]) {
        self.rx_fifo.extend(packet);
        self.rx_status
            .push_back(Self::rx_status_word(Self::RXSTS_SETUP_DATA, 8, endpoint));
        self.rx_status
            .push_back(Self::rx_status_word(Self::RXSTS_SETUP_COMP, 0, endpoint));
        self.raise_out_endpoint_interrupt(endpoint, Self::DOEPINT_STUP);
        self.set_global_interrupt_status(Self::GINTSTS_RXFLVL);
    }

    pub fn virtual_host_control_out(&mut self, endpoint: usize, packet: [u8; 8], data: &[u8]) {
        self.virtual_host_setup(endpoint, packet);
        if !data.is_empty() {
            self.rx_fifo.extend(data.iter().copied());
            self.rx_status.push_back(Self::rx_status_word(
                Self::RXSTS_OUT_DATA,
                data.len() as u32,
                endpoint,
            ));
            self.rx_status
                .push_back(Self::rx_status_word(Self::RXSTS_OUT_COMP, 0, endpoint));
        }
    }

    pub fn read_fifo(&mut self, _endpoint: usize) -> u32 {
        self.pop_rx_fifo_word()
    }

    pub fn is_configured(&self) -> bool {
        self.virtual_host_step == VirtualHostStep::Configured
    }

    const VIRTUAL_DEVICE_ADDRESS: u8 = 5;

    fn get_device_descriptor_packet() -> [u8; 8] {
        [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00]
    }

    fn set_address_packet(address: u8) -> [u8; 8] {
        [0x00, 0x05, address, 0x00, 0x00, 0x00, 0x00, 0x00]
    }

    fn set_configuration_packet(configuration: u8) -> [u8; 8] {
        [0x00, 0x09, configuration, 0x00, 0x00, 0x00, 0x00, 0x00]
    }

    fn set_line_coding_packet() -> ([u8; 8], [u8; 7]) {
        (
            [0x21, 0x20, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00],
            [0x00, 0xc2, 0x01, 0x00, 0x00, 0x00, 0x08], // 115200 8N1
        )
    }

    fn set_control_line_state_packet() -> [u8; 8] {
        [0x21, 0x22, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00] // DTR|RTS set
    }
    ```

- [ ] **Step 5: Drive the state machine from EP0 IN completion**

    Step 3 above already funnels both IN-completion call sites (the
    FIFO-fill check in `push_tx_fifo_word` and the zero-length-transfer
    check in `register_write`'s `EP_CTL_OFFSET` arm) through one
    `complete_in_transfer` method. Extend that single method body instead of
    touching either call site:

    ```rust
    fn complete_in_transfer(&mut self, ep: usize) {
        self.raise_in_endpoint_interrupt(ep, Self::DIEPINT_XFRC);
        if ep == 0 {
            self.tx_fifo[0].clear();
            self.advance_virtual_host();
        }
    }
    ```

    ```rust
    fn advance_virtual_host(&mut self) {
        self.virtual_host_step = match self.virtual_host_step {
            VirtualHostStep::AwaitingDeviceDescriptor => {
                self.virtual_host_setup(0, Self::set_address_packet(Self::VIRTUAL_DEVICE_ADDRESS));
                VirtualHostStep::AwaitingSetAddressStatus
            }
            VirtualHostStep::AwaitingSetAddressStatus => {
                self.virtual_host_setup(0, Self::set_configuration_packet(1));
                VirtualHostStep::AwaitingSetConfigurationStatus
            }
            VirtualHostStep::AwaitingSetConfigurationStatus => {
                let (packet, data) = Self::set_line_coding_packet();
                self.virtual_host_control_out(0, packet, &data);
                VirtualHostStep::AwaitingSetLineCodingStatus
            }
            VirtualHostStep::AwaitingSetLineCodingStatus => {
                self.virtual_host_setup(0, Self::set_control_line_state_packet());
                VirtualHostStep::AwaitingSetControlLineStateStatus
            }
            VirtualHostStep::AwaitingSetControlLineStateStatus => VirtualHostStep::Configured,
            VirtualHostStep::Configured => VirtualHostStep::Configured,
        };
    }
    ```

    Add a private `next_setup_request(&self) -> [u8; 8]` test helper that
    copies the first 8 bytes of `rx_fifo` without popping, used only by
    Step 1's test to assert which request was queued next.

- [ ] **Step 6: Verify unit tests pass**

    Run: `cargo test otg_fs`
    Expected: PASS.

- [ ] **Step 7: Verify against the live firmware trace**

    Run the emulator from proteus_f7 with a TCP client connected long enough
    to trigger reset, then GET_DESCRIPTOR. Confirm in the trace that
    `ie[0].DIEPCTL`/`DIEPTSIZ` writes and `FIFO_BASE` writes appear after the
    reset burst recorded in usb_trace_notes.md, and that `DAINT`/`GINTSTS`
    reads show `OEPINT` set once `virtual_host_setup` runs. Append a short
    "## Enumeration" section to usb_trace_notes.md recording what was
    observed (byte counts and register values only — do not fabricate
    descriptor content, which belongs to the firmware).

- [ ] **Step 8: Commit**

    git add src/peripherals/otg_fs.rs proteus_f7/usb_trace_notes.md
    git commit -m "feat: model OTG-FS FIFOs and a deterministic enumeration host"

### Task 5: Bridge configured bulk endpoints to the TCP client

**Files:**
- Modify: src/peripherals/otg_fs.rs

**Interfaces:**
- Consumes `UsbCdcTcp::push_from_device(&mut self, bytes: &[u8])`, `take_for_device(&mut self, maximum: usize) -> Vec<u8>`, `is_client_connected(&self) -> bool` (all already exist in src/ext_devices/usb_cdc_tcp.rs).
- Consumes `OtgFs::is_configured` from Task 4.
- Extends `Peripheral::poll` (already implemented on `OtgFs`); no `src/emulator.rs` change is needed — it already calls `d.poll()` then `p.poll(&sys)` every tick (src/emulator.rs:142-143), so `UsbCdcTcp`'s socket I/O is already current before `OtgFs::poll` runs.

- [ ] **Step 1: Write the failing bulk-forwarding test**

    Tests in this file are already in the same module tree as `OtgFs`
    (`otg_fs::tests`), so they can set private fields and call private
    consts/methods directly — no `#[cfg(test)]` wrapper methods needed,
    matching how Tasks 3-4's tests already call `register_write` and
    reference `DIEPCTL_EPENA` directly.

    ```rust
    #[test]
    fn configured_bulk_in_completion_forwards_bytes_to_the_bridge() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_step = VirtualHostStep::Configured;
        otg.set_bulk_endpoints(1, 1);
        otg.register_write(
            OtgFs::DIEP_BASE + OtgFs::EP_STRIDE + OtgFs::EP_TSIZ_OFFSET,
            3,
        );
        otg.register_write(
            OtgFs::DIEP_BASE + OtgFs::EP_STRIDE + OtgFs::EP_CTL_OFFSET,
            OtgFs::DIEPCTL_EPENA,
        );
        otg.register_write(OtgFs::FIFO_BASE + OtgFs::FIFO_WINDOW, 0x00ff0042);
        // xfrsiz=3 truncates the word-padded 4-byte push (0x42,0x00,0xff,0x00)
        // to the 3 bytes the endpoint's DIEPTSIZ actually asked to send.
        assert_eq!(otg.pending_bridge_writes, vec![0x42, 0x00, 0xff]);
    }
    ```

- [ ] **Step 2: Verify failure**

    Run: `cargo test otg_fs`
    Expected: FAIL — `set_bulk_endpoints` and the `pending_bridge_writes`
    field do not exist yet.

- [ ] **Step 3: Implement bulk endpoint tracking and TCP forwarding**

    Add fields:

    ```rust
    pub struct OtgFs {
        // ...existing fields from Tasks 3-4...
        bulk_in_endpoint: Option<usize>,
        bulk_out_endpoint: Option<usize>,
        pending_bridge_writes: Vec<u8>,
    }
    ```

    Initialize `bulk_in_endpoint`/`bulk_out_endpoint` to `None` and
    `pending_bridge_writes` to `Vec::new()` in both `new` and `for_test`.
    Add:

    ```rust
    pub fn set_bulk_endpoints(&mut self, in_ep: usize, out_ep: usize) {
        self.bulk_in_endpoint = Some(in_ep);
        self.bulk_out_endpoint = Some(out_ep);
    }
    ```

    `pending_bridge_writes` only accumulates when `bridge` is `None` (i.e.
    under test, or before a TCP client ever connects); when `bridge` is
    `Some`, bytes are forwarded straight to it instead. Extend
    `complete_in_transfer` (funnels both IN-completion call sites, per Task
    4's Step 5) with an `else if` for the bulk IN endpoint:

    ```rust
    fn complete_in_transfer(&mut self, ep: usize) {
        self.raise_in_endpoint_interrupt(ep, Self::DIEPINT_XFRC);
        if ep == 0 {
            self.tx_fifo[0].clear();
            self.advance_virtual_host();
        } else if self.is_configured() && Some(ep) == self.bulk_in_endpoint {
            // Truncate to the requested transfer size: the FIFO always fills
            // in whole 4-byte words, but the last word of a transfer whose
            // byte count isn't a multiple of 4 has trailing pad bytes that
            // must not reach the TCP client.
            let xfrsiz = (self.ep_in[ep].tsiz & Self::XFRSIZ_MASK) as usize;
            let bytes: Vec<u8> = self.tx_fifo[ep].drain(..).collect();
            let bytes = &bytes[..bytes.len().min(xfrsiz)];
            match &self.bridge {
                Some(bridge) => bridge.borrow_mut().push_from_device(bytes),
                None => self.pending_bridge_writes.extend(bytes),
            }
        }
    }
    ```

    Implement OUT forwarding in `Peripheral::poll` (already present on
    `OtgFs`), after the existing host-attach/interrupt-pending logic:

    ```rust
    if self.is_configured() {
        if let (Some(out_ep), Some(bridge)) = (self.bulk_out_endpoint, self.bridge.as_ref()) {
            let bytes = bridge.borrow_mut().take_for_device(64);
            if !bytes.is_empty() {
                self.rx_fifo.extend(bytes.iter().copied());
                self.rx_status.push_back(Self::rx_status_word(
                    Self::RXSTS_OUT_DATA,
                    bytes.len() as u32,
                    out_ep,
                ));
                self.rx_status
                    .push_back(Self::rx_status_word(Self::RXSTS_OUT_COMP, 0, out_ep));
                self.raise_out_endpoint_interrupt(out_ep, Self::DOEPINT_XFRC);
                self.set_global_interrupt_status(Self::GINTSTS_RXFLVL);
            }
        }
    }
    ```

- [ ] **Step 4: Verify unit tests pass**

    Run: `cargo test otg_fs`
    Expected: PASS.

- [ ] **Step 5: Discover the real bulk endpoint numbers from firmware**

    Run the emulator from proteus_f7 far enough (raise
    `--max-instructions` past the point Task 4 reached) with one TCP client
    connected through full enumeration. Find the first `ie[n].DIEPCTL`/
    `oe[n].DOEPCTL` writes with `n != 0` and a nonzero `USBAEP` bit — those
    `n` values are the real CDC bulk IN/OUT endpoint numbers. Record them,
    with the exact trace lines, in a new "## Bulk endpoints" section of
    usb_trace_notes.md. Call `otg_fs.set_bulk_endpoints(in_ep, out_ep)` from
    `OtgFs::new` using those literal numbers (not a config option — the
    design's non-goals don't call for configurability here, and the
    firmware's endpoint assignment is fixed at compile time).

- [ ] **Step 6: Commit**

    git add src/peripherals/otg_fs.rs proteus_f7/usb_trace_notes.md
    git commit -m "feat: bridge OTG-FS bulk endpoints to the CDC TCP client"

### Task 6: Configure and verify Proteus USB CDC over TCP

**Files:**
- Modify: proteus_f7/config.yaml
- Modify: proteus_f7/verify_boot.ps1
- Create: docs/proteus-f7-usb.md

**Interfaces:**
- Uses SVD name `OTG_FS_GLOBAL` (confirmed in Task 3's usb_trace_notes.md).
- Uses listener 127.0.0.1:29000 and max_buffered_bytes 65536.

- [ ] **Step 1: Add a failing YAML assertion**

    Require verify_boot.ps1 to find a `usb_cdc_tcp` device configuration with
    `peripheral: OTG_FS_GLOBAL`, the loopback listener, and the 65536 queue
    limit.

- [ ] **Step 2: Verify failure**

    Run: `.\proteus_f7\verify_boot.ps1`
    Expected: FAIL because config.yaml has no USB CDC TCP entry.

- [ ] **Step 3: Add the evidence-backed device configuration**

    Add under `devices` in proteus_f7/config.yaml:

        usb_cdc_tcp:
          - peripheral: OTG_FS_GLOBAL
            listen: 127.0.0.1:29000
            max_buffered_bytes: 65536

- [ ] **Step 4: Document operation**

    Create docs/proteus-f7-usb.md covering: launch setup, raw binary TCP
    semantics (no framing beyond the CDC bulk payload bytes themselves), the
    one-client rule and disconnect-then-reconnect behavior, and an explicit
    statement that this is CDC-over-TCP, not a Windows USB device or COM
    port — connecting requires a raw TCP client (e.g. `ncat 127.0.0.1
    29000`), not a serial terminal pointed at a COM port.

- [ ] **Step 5: Verify the full route**

    Run: `.\proteus_f7\verify_boot.ps1`
    Expected: PASS through the USB configuration assertions added in Step 1.

    Run the emulator, connect one local TCP client to 127.0.0.1:29000, and
    exchange a captured firmware protocol request and response. Record the
    exact bytes and result in docs/proteus-f7-usb.md.

- [ ] **Step 6: Final checks and commit**

    Run: `cargo test`
    Expected: PASS.

    Run: `cargo build --release`
    Expected: PASS.

    Run: `git diff --check`
    Expected: no whitespace errors.

    git add proteus_f7/config.yaml proteus_f7/verify_boot.ps1 docs/proteus-f7-usb.md
    git commit -m "feat: expose Proteus USB CDC over TCP"
