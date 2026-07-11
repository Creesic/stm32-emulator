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

### Task 3: Model the trace-proven OTG-FS device subset

**Files:**
- Create: src/peripherals/otg_fs.rs
- Modify: src/peripherals/mod.rs
- Modify: src/emulator.rs

**Interfaces:**
- Produces OtgFs::new(name: &str, ext_devices: &ExtDevices) -> Option<Box<dyn Peripheral>>.
- Produces OtgFs::for_test(), virtual_host_reset(), virtual_host_setup(packet), read_fifo(endpoint), and interrupt_pending().
- Consumes UsbCdcTcp queues and calls Nvic::set_intr_pending using the IRQ recorded in usb_trace_notes.md.

- [ ] **Step 1: Write failing controller tests**

    #[test]
    fn masked_usb_reset_interrupt_does_not_become_pending() {
        let mut otg = OtgFs::for_test();
        otg.set_global_interrupt_status(OtgFs::USB_RESET);
        assert!(!otg.interrupt_pending());
        otg.write_global_interrupt_mask(OtgFs::USB_RESET);
        assert!(otg.interrupt_pending());
    }

    #[test]
    fn endpoint_zero_setup_packet_is_read_from_fifo() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_reset();
        otg.virtual_host_setup([0x80, 0x06, 0x00, 0x01, 0, 0, 18, 0]);
        assert_eq!(otg.read_fifo(0), 0x0100_0680);
    }

- [ ] **Step 2: Verify failure**

    Run: cargo test otg_fs
    Expected: FAIL because peripherals::otg_fs does not exist.

- [ ] **Step 3: Implement device registers and FIFOs**

    Use only offsets named in usb_trace_notes.md: global/device configuration,
    GINTSTS/GINTMSK, endpoint-zero and observed bulk endpoint control/size/
    interrupt registers, FIFO ports, and power-clock register. Store state for
    writable registers and apply write-one-to-clear only where trace proves it.

- [ ] **Step 4: Implement deterministic virtual host**

    Queue bus reset, GET_DESCRIPTOR, SET_ADDRESS, SET_CONFIGURATION,
    SET_LINE_CODING, and SET_CONTROL_LINE_STATE. Advance to the next setup
    packet only after firmware completes the preceding endpoint-zero response.
    Raise global and endpoint interrupts only when their masks allow it.

- [ ] **Step 5: Connect TCP and emulator polling**

    At the existing SDL event-pump interval in the emulator code hook, invoke
    sys.d.poll. OtgFs forwards completed bulk IN bytes to push_from_device and
    exposes take_for_device bytes as bulk OUT packets with the observed
    endpoint interrupt status.

- [ ] **Step 6: Verify and commit**

    Run: cargo test otg_fs
    Expected: PASS.

    Run: cargo test
    Expected: PASS with only existing unrelated warnings.

    git add src/peripherals/otg_fs.rs src/peripherals/mod.rs src/emulator.rs
    git commit -m "feat: model Proteus F7 OTG-FS device controller"

### Task 4: Configure and verify Proteus USB CDC over TCP

**Files:**
- Modify: proteus_f7/config.yaml
- Modify: proteus_f7/verify_boot.ps1
- Create: docs/proteus-f7-usb.md

**Interfaces:**
- Uses the exact OTG SVD name recorded in usb_trace_notes.md.
- Uses listener 127.0.0.1:29000 and max_buffered_bytes 65536.

- [ ] **Step 1: Add a failing YAML assertion**

    Require verify_boot.ps1 to find an OTG-attached usb_cdc_tcp configuration,
    the loopback listener, and the 65536 queue limit.

- [ ] **Step 2: Verify failure**

    Run: .\proteus_f7\verify_boot.ps1
    Expected: FAIL because config.yaml has no USB CDC TCP entry.

- [ ] **Step 3: Add the evidence-backed device configuration**

    Add the following under devices, replacing the SVD name with the exact
    recorded OTG peripheral name:

        usb_cdc_tcp:
          - peripheral: recorded OTG-FS SVD name
            listen: 127.0.0.1:29000
            max_buffered_bytes: 65536

- [ ] **Step 4: Document operation**

    Document launch setup, raw binary TCP semantics, one-client rule,
    disconnect behavior, and the distinction between CDC-over-TCP and a
    Windows USB or COM device.

- [ ] **Step 5: Verify the full route**

    Run: .\proteus_f7\verify_boot.ps1
    Expected: PASS through recorded USB initialization assertions.

    Run the emulator, connect one local TCP client to 127.0.0.1:29000, and
    exchange a captured firmware protocol request and response. Record exact
    bytes and result in docs/proteus-f7-usb.md.

- [ ] **Step 6: Final checks and commit**

    Run: cargo test
    Expected: PASS.

    Run: cargo build --release
    Expected: PASS.

    Run: git diff --check
    Expected: no whitespace errors.

    git add proteus_f7/config.yaml proteus_f7/verify_boot.ps1 docs/proteus-f7-usb.md
    git commit -m "feat: expose Proteus USB CDC over TCP"
