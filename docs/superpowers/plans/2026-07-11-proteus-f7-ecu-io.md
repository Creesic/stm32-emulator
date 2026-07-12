# Proteus F7 ECU I/O Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an external process drive the Proteus F7 rusEFI firmware's digital trigger inputs (crank/cam), analog sensor inputs (MAP/TPS/CLT/IAT/vbatt), and observe its injector/ignition output pins, over a TCP text protocol — with no physical hardware and no waveform generation inside the emulator itself.

**Architecture:** A new `EcuIo` ext_device owns a single-client TCP bridge (same nonblocking pattern as `usb_cdc_tcp.rs`) and a `name=value` line protocol; it reuses the *existing* named-pin GPIO callback mechanism in `gpio.rs` for digital pins, and a new minimal `Adc` peripheral (ADC1 only, modeled after ChibiOS's actual scan-conversion sequence) looks up analog channel values from it by pin. Because rusEFI's trigger input is genuinely EXTI-driven (confirmed by reading ChibiOS's actual digital-input source, not assumed), a new minimal `Exti`/`SYSCFG` peripheral (Task 4) is also required so an external level change on `crank`/`cam` actually raises the interrupt firmware is waiting for, rather than only being visible the next time firmware happens to poll GPIO — this was discovered during planning, not anticipated in the original design spec's architecture sketch.

**Tech Stack:** Rust, `std::net::{TcpListener, TcpStream}` (nonblocking), `serde`/`serde_yaml` for config, existing `Peripheral`/`ExtDevice` traits.

## Global Constraints

- Digital input pins read low (0) and ADC channels read 0mV until a value is received for that name — no separate "default value" config field.
- ADC millivolt values are clamped to 0–3300mV (VREF+) before conversion to a 12-bit count, never wrapped or panicked on.
- One TCP client at a time; a second connection attempt is accepted then immediately dropped, matching `UsbCdcTcp`.
- Malformed protocol lines are logged and ignored, never fatal.
- No trigger-wheel pattern generation, no CPU-clock-rate modeling, and no real-time instruction pacing — explicitly out of scope (see `docs/superpowers/specs/2026-07-11-proteus-f7-ecu-io-design.md`).
- Only ADC1 is modeled; ADC2/ADC3 are untouched (Proteus uses ADC3 for knock sensing, out of scope for this milestone).

---

### Task 1: `EcuIo` core — TCP bridge and `name=value` protocol

**Ground truth used by this task:** `src/ext_devices/usb_cdc_tcp.rs` is the exact pattern to mirror for a single-client nonblocking TCP bridge (bind, `set_nonblocking(true)`, `accept_clients`/`receive_from_client`/`send_to_client` split, `ErrorKind::WouldBlock`/`ConnectionReset` handling). `src/peripherals/gpio.rs:17-26`'s `Pin::from_str` is the existing pin-name parser (accepts `"PC6"` or `"C6"`, panics on invalid input — matching how `SoftwareSpiConfig`'s pins are parsed in `src/peripherals/sw_spi.rs:41-44`).

**Files:**
- Create: `src/ext_devices/ecu_io.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/ext_devices/ecu_io.rs`

**Interfaces:**
- Produces `EcuIoConfig { listen: String, pins: Vec<EcuIoPinConfig>, adc_channels: Vec<EcuIoAdcChannelConfig> }`, `EcuIoPinConfig { name: String, pin: String, direction: EcuIoPinDirection }`, `EcuIoPinDirection` (`Input`/`Output`, deserialized from lowercase YAML), `EcuIoAdcChannelConfig { name: String, pin: String }` — all `#[derive(Debug, Deserialize, Clone)]`.
- Produces `EcuIo::new(config: EcuIoConfig) -> anyhow::Result<Self>`, `EcuIo::local_addr(&self) -> anyhow::Result<SocketAddr>`, `EcuIo::poll(&mut self) -> anyhow::Result<()>`.
- Produces `EcuIo::adc_millivolts(&self, pin: Pin) -> i32` (0 if the pin has no configured ADC channel or no value received yet) — Task 3 (`Adc` peripheral) consumes this.
- Produces `EcuIo::digital_level(&self, name: &str) -> bool` and `EcuIo::report_output(&mut self, name: &str, level: bool)` — Task 2 consumes both from GPIO callback closures.
- Produces `pub(crate) fn set_value(&mut self, name: &str, value: i32)` (also used internally by line parsing) — Task 3's tests use this directly to seed ADC channel values without a real TCP round-trip.
- Consumes: `crate::peripherals::gpio::Pin` (parsing only in this task; no `GpioPorts` dependency yet).

- [x] **Step 1: Write the failing tests**

    Create `src/ext_devices/ecu_io.rs`:

    ```rust
    // SPDX-License-Identifier: GPL-3.0-or-later

    use std::{
        collections::{HashMap, VecDeque},
        io::{ErrorKind, Read, Write},
        net::{SocketAddr, TcpListener, TcpStream},
    };

    use anyhow::{Context, Result};
    use serde::Deserialize;

    use crate::peripherals::gpio::Pin;

    #[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum EcuIoPinDirection {
        Input,
        Output,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct EcuIoPinConfig {
        pub name: String,
        pub pin: String,
        pub direction: EcuIoPinDirection,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct EcuIoAdcChannelConfig {
        pub name: String,
        pub pin: String,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct EcuIoConfig {
        pub listen: String,
        #[serde(default)]
        pub pins: Vec<EcuIoPinConfig>,
        #[serde(default)]
        pub adc_channels: Vec<EcuIoAdcChannelConfig>,
    }

    pub struct EcuIo {
        pub config: EcuIoConfig,
        listener: TcpListener,
        client: Option<TcpStream>,
        recv_buffer: Vec<u8>,
        outgoing: VecDeque<u8>,
        values: HashMap<String, i32>,
        adc_channels: Vec<(Pin, String)>,
    }

    #[cfg(test)]
    mod tests {
        use std::{io::{Read, Write}, net::TcpStream, time::Duration};

        use super::{EcuIo, EcuIoAdcChannelConfig, EcuIoConfig};
        use crate::peripherals::gpio::Pin;

        fn test_config() -> EcuIoConfig {
            EcuIoConfig {
                listen: "127.0.0.1:0".to_string(),
                pins: vec![],
                adc_channels: vec![
                    EcuIoAdcChannelConfig { name: "map".to_string(), pin: "PC0".to_string() },
                ],
            }
        }

        #[test]
        fn a_line_sent_by_the_client_updates_the_named_value() {
            let mut ecu_io = EcuIo::new(test_config()).unwrap();
            let mut client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
            client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            ecu_io.poll().unwrap(); // accept

            client.write_all(b"map=1500\n").unwrap();
            for _ in 0..10 {
                ecu_io.poll().unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }

            assert_eq!(ecu_io.adc_millivolts(Pin::from_str("PC0")), 1500);
        }

        #[test]
        fn a_malformed_line_is_ignored_not_fatal() {
            let mut ecu_io = EcuIo::new(test_config()).unwrap();
            let mut client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
            ecu_io.poll().unwrap();

            client.write_all(b"not-a-valid-line\nmap=750\n").unwrap();
            for _ in 0..10 {
                ecu_io.poll().unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }

            assert_eq!(ecu_io.adc_millivolts(Pin::from_str("PC0")), 750);
        }

        #[test]
        fn an_adc_millivolt_value_is_clamped_to_the_valid_range() {
            let mut ecu_io = EcuIo::new(test_config()).unwrap();
            ecu_io.set_value("map", 999_999);
            assert_eq!(ecu_io.adc_millivolts(Pin::from_str("PC0")), 3300);

            ecu_io.set_value("map", -50);
            assert_eq!(ecu_io.adc_millivolts(Pin::from_str("PC0")), 0);
        }

        #[test]
        fn an_unconfigured_pin_reads_zero() {
            let ecu_io = EcuIo::new(test_config()).unwrap();
            assert_eq!(ecu_io.adc_millivolts(Pin::from_str("PA4")), 0);
        }

        #[test]
        fn report_output_pushes_a_line_only_when_a_client_is_connected() {
            let mut ecu_io = EcuIo::new(test_config()).unwrap();

            // No client connected: must not queue anything or panic.
            ecu_io.report_output("inj1", true);
            ecu_io.poll().unwrap();

            let mut client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
            client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            ecu_io.poll().unwrap(); // accept

            ecu_io.report_output("inj1", true);
            ecu_io.poll().unwrap();

            let mut buf = [0; 16];
            let n = client.read(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"inj1=1\n");
        }

        #[test]
        fn ecu_io_configuration_deserializes() {
            let config: EcuIoConfig = serde_yaml::from_str(
                "listen: 127.0.0.1:29002\npins:\n  - name: crank\n    pin: PC6\n    direction: input\nadc_channels:\n  - name: map\n    pin: PC0\n",
            )
            .unwrap();

            assert_eq!(config.pins[0].name, "crank");
            assert_eq!(config.adc_channels[0].name, "map");
        }
    }
    ```

- [x] **Step 2: Run tests to verify they fail**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator ecu_io`
    Expected: FAIL to compile — `EcuIo::new`, `poll`, `local_addr`, `adc_millivolts`, `set_value`, `report_output` don't exist yet (only the config structs and field layout do).

- [x] **Step 3: Implement `EcuIo`**

    Add to `src/ext_devices/ecu_io.rs`, above the `#[cfg(test)]` block:

    ```rust
    impl EcuIo {
        pub fn new(config: EcuIoConfig) -> Result<Self> {
            let listener = TcpListener::bind(&config.listen)
                .with_context(|| format!("Failed to listen for ECU IO at {}", config.listen))?;
            listener
                .set_nonblocking(true)
                .context("Failed to make ECU IO listener nonblocking")?;

            let adc_channels = config
                .adc_channels
                .iter()
                .map(|c| (Pin::from_str(&c.pin), c.name.clone()))
                .collect();

            Ok(Self {
                config,
                listener,
                client: None,
                recv_buffer: Vec::new(),
                outgoing: VecDeque::new(),
                values: HashMap::new(),
                adc_channels,
            })
        }

        pub fn local_addr(&self) -> Result<SocketAddr> {
            self.listener
                .local_addr()
                .context("Failed to read ECU IO listener address")
        }

        pub fn poll(&mut self) -> Result<()> {
            self.accept_clients()?;
            self.receive_from_client()?;
            self.send_to_client()?;
            Ok(())
        }

        pub fn adc_millivolts(&self, pin: Pin) -> i32 {
            self.adc_channels
                .iter()
                .find(|(p, _)| *p == pin)
                .map(|(_, name)| self.values.get(name).copied().unwrap_or(0))
                .unwrap_or(0)
                .clamp(0, 3300)
        }

        pub fn digital_level(&self, name: &str) -> bool {
            self.values.get(name).copied().unwrap_or(0) != 0
        }

        pub fn report_output(&mut self, name: &str, level: bool) {
            self.values.insert(name.to_string(), level as i32);
            if self.client.is_some() {
                let line = format!("{name}={}\n", level as i32);
                self.outgoing.extend(line.into_bytes());
            }
        }

        pub(crate) fn set_value(&mut self, name: &str, value: i32) {
            self.values.insert(name.to_string(), value);
        }

        fn accept_clients(&mut self) -> Result<()> {
            loop {
                match self.listener.accept() {
                    Ok((client, address)) => {
                        if self.client.is_some() {
                            debug!("Rejecting additional ECU IO client at {address}");
                            continue;
                        }
                        client
                            .set_nonblocking(true)
                            .context("Failed to make ECU IO client nonblocking")?;
                        info!("ECU IO client connected from {address}");
                        self.client = Some(client);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error).context("Failed to accept ECU IO client"),
                }
            }
            Ok(())
        }

        fn receive_from_client(&mut self) -> Result<()> {
            let mut disconnected = false;
            if let Some(client) = self.client.as_mut() {
                let mut buffer = [0; 1024];
                loop {
                    match client.read(&mut buffer) {
                        Ok(0) => {
                            disconnected = true;
                            break;
                        }
                        Ok(count) => self.recv_buffer.extend_from_slice(&buffer[..count]),
                        Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                            disconnected = true;
                            break;
                        }
                        Err(error) => return Err(error).context("Failed to read ECU IO client"),
                    }
                }
            }
            if disconnected {
                info!("ECU IO client disconnected");
                self.client = None;
            }
            self.process_complete_lines();
            Ok(())
        }

        fn process_complete_lines(&mut self) {
            while let Some(pos) = self.recv_buffer.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.recv_buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]);
                let line = line.trim();
                if !line.is_empty() {
                    self.handle_line(line);
                }
            }
        }

        fn handle_line(&mut self, line: &str) {
            match line.split_once('=') {
                Some((name, value)) => match value.trim().parse::<i32>() {
                    Ok(value) => self.set_value(name.trim(), value),
                    Err(_) => warn!("ECU IO: malformed value in line {line:?}"),
                },
                None => warn!("ECU IO: malformed line {line:?}"),
            }
        }

        fn send_to_client(&mut self) -> Result<()> {
            let mut disconnected = false;
            if let Some(client) = self.client.as_mut() {
                while !self.outgoing.is_empty() {
                    let (first, second) = self.outgoing.as_slices();
                    let bytes = if first.is_empty() { second } else { first };
                    match client.write(bytes) {
                        Ok(0) => {
                            disconnected = true;
                            break;
                        }
                        Ok(count) => {
                            self.outgoing.drain(..count);
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                            disconnected = true;
                            break;
                        }
                        Err(error) => return Err(error).context("Failed to write ECU IO client"),
                    }
                }
            }
            if disconnected {
                info!("ECU IO client disconnected");
                self.client = None;
            }
            Ok(())
        }
    }
    ```

    Also add `#[derive(Clone, Copy, PartialEq)]` to `Pin` in `src/peripherals/gpio.rs:10` (currently just `#[derive(Clone, Copy)]`) — needed for the `*p == pin` comparison in `adc_millivolts`.

- [x] **Step 4: Run tests to verify they pass**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator ecu_io`
    Expected: PASS (all 6 tests).

- [x] **Step 5: Commit**

    ```bash
    git add src/ext_devices/ecu_io.rs src/peripherals/gpio.rs
    git commit -m "feat: add EcuIo TCP bridge and name=value protocol"
    ```

---

### Task 2: Wire `EcuIo` into GPIO and `ExtDevices`

**Ground truth used by this task:** `src/peripherals/sw_spi.rs:39-72`'s `SoftwareSpi::register` is the exact pattern for a device that needs a self-referencing `Rc<RefCell<Self>>` to register GPIO callbacks against its own state: parse pin names into local `Pin` values *before* moving the config into `Self`, construct `Rc::new(RefCell::new(Self { .. }))`, then loop registering callbacks that each clone the `Rc` and move in whatever the closure needs. `src/ext_devices/mod.rs:91-119`'s `ExtDevicesConfig::into_ext_devices` is the exact wiring point — it already threads `gpio: &mut GpioPorts` through for `Touchscreen::new`, so `EcuIo::register` fits the same call site.

**Files:**
- Modify: `src/ext_devices/ecu_io.rs`
- Modify: `src/ext_devices/mod.rs`
- Test: inline in `src/ext_devices/ecu_io.rs`

**Interfaces:**
- Consumes: `EcuIo::new`, `digital_level`, `report_output` (Task 1); `GpioPorts::add_read_callback`/`add_write_callback` (`src/peripherals/gpio.rs:43-49`, unchanged).
- Produces `EcuIo::register(config: EcuIoConfig, gpio: &mut GpioPorts) -> anyhow::Result<Rc<RefCell<EcuIo>>>` — Task 4's config wiring and any future `find_ecu_io`-style lookup depend on this being how `EcuIo` instances always get constructed once GPIO is involved.
- Produces `ExtDevices.ecu_ios: Vec<Rc<RefCell<EcuIo>>>` and `ExtDevices::ecu_io(&self) -> Option<Rc<RefCell<EcuIo>>>` (returns the first configured instance, or `None`) — Task 3's `Adc` peripheral consumes `ecu_io()`.
- Produces `ExtDevicesConfig.ecu_io: Option<Vec<EcuIoConfig>>` field.

- [x] **Step 1: Write the failing test**

    Add to `src/ext_devices/ecu_io.rs`'s `mod tests`:

    ```rust
    use std::{cell::RefCell, rc::Rc};

    use unicorn_engine::{unicorn_const::{Arch, Mode}, Unicorn};

    use crate::{ext_devices::ExtDevices, peripherals::{gpio::GpioPorts, Peripherals}, system::System};

    // `System` borrows its Unicorn engine mutably (`uc: RefCell<&'a mut Unicorn<'b, ()>>`),
    // so this can't return a ready-made `System` — the engine would be a dangling
    // local. Each test constructs its own `Unicorn`/`Peripherals`/`ExtDevices` and
    // builds `System` from them locally instead.
    fn test_parts() -> (Unicorn<'static, ()>, Rc<Peripherals>, Rc<ExtDevices>) {
        let uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        (uc, Rc::new(Peripherals::default()), Rc::new(ExtDevices::default()))
    }

    #[test]
    fn registered_input_pin_reflects_the_last_received_value_on_gpio_read() {
        let mut gpio = GpioPorts::default();
        let config = EcuIoConfig {
            listen: "127.0.0.1:0".to_string(),
            pins: vec![super::EcuIoPinConfig {
                name: "crank".to_string(),
                pin: "PC6".to_string(),
                direction: super::EcuIoPinDirection::Input,
            }],
            adc_channels: vec![],
        };
        let ecu_io = EcuIo::register(config, &mut gpio).unwrap();
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };

        ecu_io.borrow_mut().set_value("crank", 1);
        let port = GpioPorts::port_index('C');
        assert_eq!(gpio.read_port(&sys, port), 1 << 6);

        ecu_io.borrow_mut().set_value("crank", 0);
        assert_eq!(gpio.read_port(&sys, port), 0);
    }

    #[test]
    fn registered_output_pin_reports_a_gpio_write_over_the_connection() {
        let mut gpio = GpioPorts::default();
        let config = EcuIoConfig {
            listen: "127.0.0.1:0".to_string(),
            pins: vec![super::EcuIoPinConfig {
                name: "inj1".to_string(),
                pin: "PD7".to_string(),
                direction: super::EcuIoPinDirection::Output,
            }],
            adc_channels: vec![],
        };
        let ecu_io = EcuIo::register(config, &mut gpio).unwrap();
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };

        let mut client = TcpStream::connect(ecu_io.borrow().local_addr().unwrap()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        ecu_io.borrow_mut().poll().unwrap();

        let port = GpioPorts::port_index('D');
        gpio.write_port(&sys, port, 7, true);
        ecu_io.borrow_mut().poll().unwrap();

        let mut buf = [0; 16];
        let n = client.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"inj1=1\n");
    }
    ```

    No existing test in this codebase constructs a real `System` (every other peripheral keeps its testable logic `&System`-free, per this task's own `digital_level`/`report_output` design) — `test_parts()`, defined above in this same code block, is a new helper. `System<'a, 'b>` borrows its Unicorn engine mutably (`uc: RefCell<&'a mut Unicorn<'b, ()>>`), so `test_parts()` returns the owned `(Unicorn, Rc<Peripherals>, Rc<ExtDevices>)` triple rather than a ready-made `System` — each test builds `System { uc: RefCell::new(&mut uc), p, d }` locally, right before use, exactly as shown above.

- [x] **Step 2: Run test to verify it fails**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator ecu_io`
    Expected: FAIL to compile — `EcuIo::register` doesn't exist yet.

- [x] **Step 3: Implement `EcuIo::register`**

    Add to `src/ext_devices/ecu_io.rs`'s `impl EcuIo` block:

    ```rust
    pub fn register(config: EcuIoConfig, gpio: &mut GpioPorts) -> Result<Rc<RefCell<Self>>> {
        let pins: Vec<_> = config
            .pins
            .iter()
            .map(|p| (Pin::from_str(&p.pin), p.name.clone(), p.direction))
            .collect();

        let self_ = Rc::new(RefCell::new(Self::new(config)?));

        for (pin, name, direction) in pins {
            match direction {
                EcuIoPinDirection::Input => {
                    let s = self_.clone();
                    gpio.add_read_callback(pin, move |_sys| s.borrow().digital_level(&name));
                }
                EcuIoPinDirection::Output => {
                    let s = self_.clone();
                    gpio.add_write_callback(pin, move |_sys, v| s.borrow_mut().report_output(&name, v));
                }
            }
        }

        Ok(self_)
    }
    ```

    Add the needed imports at the top of `src/ext_devices/ecu_io.rs`:

    ```rust
    use std::{cell::RefCell, rc::Rc};

    use crate::peripherals::gpio::GpioPorts;
    ```

    Modify `src/ext_devices/mod.rs`:

    ```rust
    pub mod ecu_io;
    ```

    (added alongside the other `mod` declarations at the top — `pub`, unlike
    most of its neighbors, because Task 3's `Adc` peripheral needs to name
    `crate::ext_devices::ecu_io::EcuIo` directly, the same reason
    `usb_cdc_tcp` is already `pub mod` there.)

    ```rust
    use ecu_io::{EcuIoConfig, EcuIo};
    ```

    In `ExtDevicesConfig` (`src/ext_devices/mod.rs:24-32`), add:

    ```rust
    pub ecu_io: Option<Vec<EcuIoConfig>>,
    ```

    In `ExtDevices` (`src/ext_devices/mod.rs:34-41`), add `#[derive(Default)]`
    above the struct (all its fields are `Vec`s, so this is free) and add a
    new field:

    ```rust
    pub ecu_ios: Vec<Rc<RefCell<EcuIo>>>,
    ```

    The `#[derive(Default)]` addition is what lets test code elsewhere
    construct a bare `ExtDevices::default()` without listing every field —
    used by the `test_parts()` helper in `src/ext_devices/ecu_io.rs`'s own
    tests (this task's Step 1).

    In `ExtDevices::poll` (`src/ext_devices/mod.rs:82-88`), add alongside the `usb_cdc_tcps` loop:

    ```rust
    for ecu_io in &self.ecu_ios {
        if let Err(error) = ecu_io.borrow_mut().poll() {
            warn!("ECU IO bridge error: {error:#}");
        }
    }
    ```

    Add a new method on `impl ExtDevices`:

    ```rust
    pub fn ecu_io(&self) -> Option<Rc<RefCell<EcuIo>>> {
        self.ecu_ios.first().cloned()
    }
    ```

    In `ExtDevicesConfig::into_ext_devices` (`src/ext_devices/mod.rs:91-119`), add:

    ```rust
    let ecu_ios = self.ecu_io.unwrap_or_default().into_iter()
        .map(|config| EcuIo::register(config, gpio))
        .collect::<Result<_>>()?;
    ```

    and add `ecu_ios` to the final `Ok(ExtDevices { .. })` construction.

- [x] **Step 4: Run tests to verify they pass**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator ecu_io`
    Expected: PASS (all tests from Task 1 and Task 2).

- [x] **Step 5: Run the full test suite**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator`
    Expected: PASS — no other test broken by the `Pin: PartialEq` derive or the new `ExtDevices` field.

- [x] **Step 6: Commit**

    ```bash
    git add src/ext_devices/ecu_io.rs src/ext_devices/mod.rs
    git commit -m "feat: wire EcuIo into GPIO callbacks and ExtDevices"
    ```

---

### Task 3: `Adc` peripheral (ADC1 only)

**Ground truth used by this task:** ChibiOS's STM32F7 ADC driver (`firmware/ChibiOS/os/hal/ports/STM32/LLD/ADCv2/hal_adc_lld.c` in the `epicefi_fw` checkout) confirms:
- Completion is **DMA-driven, not ADC-interrupt-driven**: the ADC's own IRQ handler only reports `SR_OVR`/`SR_AWD` errors; normal conversion completion comes entirely from the DMA stream's transfer-complete interrupt after it reads the configured number of halfwords from `DR`. This project's existing `Dma` peripheral (`src/peripherals/dma.rs`) already raises that completion interrupt once it finishes reading the transfer — `Adc` itself never needs to raise an interrupt.
- rusEFI's Proteus port reads all five sensors (MAP/TPS/CLT/IAT/vbatt) through **ADC1 only**, as one scan-mode conversion group covering channels 0–15 in order (`firmware/hw_layer/ports/stm32/stm32_adc_v2.cpp`), oversampled 8x by simply restarting the 16-channel sequence 8 times in a row into a 128-halfword DMA buffer (`CONT` bit set) — not by any special "oversampling" register field. `STM32_ADC_USE_ADC3=TRUE` is set only for knock sensing on the Proteus board, unrelated to these five sensors, and is out of scope.
- Channel-to-pin mapping is the standard, fixed STM32F4/F7 shared ADC1/2/3 mapping (confirmed against the channel numbers found for each sensor: MAP=channel 10=PC0, TPS=channel 11=PC1, CLT=channel 8=PB0, IAT=channel 15=PC5, vbatt=channel 7=PA7): channels 0–7 are PA0–PA7, 8–9 are PB0–PB1, 10–15 are PC0–PC5.
- **Register sequence** (`adc_lld_start_conversion`): DMA is configured and enabled first (not modeled here — `Dma` already handles it generically), then `SQR1`/`SQR2`/`SQR3` (channel sequence), `CR1`, then `CR2` — critically, `CR2`'s `SWSTART` bit (bit 30) is what triggers a fresh scan; on real hardware this always restarts the sequence at the first configured channel.
- `Peripherals::MEMORY_MAPS`/`get_peripheral` dispatch already handles per-instance address ranges automatically — `ADC1`'s SVD base is `0x40012000` (`proteus_f7/STM32F767.svd:11955-11968`), `ADC2`/`ADC3` are `derivedFrom="ADC1"` at `0x40012100`/`0x40012200`. No `Peripherals::modeled_range` override is needed since each instance's own register set (through `DR` at offset `0x4C`) doesn't extend into the next instance's base.
- **A real, non-obvious bug this task must avoid**: `Peripheral::read_dma`'s *default* implementation (`src/peripherals/mod.rs:457-463`) calls `self.read(sys, offset)` once per **byte** of the transfer, truncating each 32-bit register read to its low byte. `Dma::do_xfer` (`src/peripherals/dma.rs:94`) computes `size = word_size() * ndtr` in **bytes**, and ADC-to-memory transfers use `word_size=2` (halfword). Relying on the default `read_dma` would call the channel-advancing conversion logic **twice per intended conversion** (once per byte, not once per halfword) and would never reconstruct a correct little-endian 16-bit sample. `Adc` must override `read_dma` to advance its channel sequence exactly once per 2 bytes.

**Files:**
- Create: `src/peripherals/adc.rs`
- Modify: `src/peripherals/mod.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/peripherals/adc.rs`

**Interfaces:**
- Consumes: `EcuIo::adc_millivolts(&self, pin: Pin) -> i32` (Task 1), `ExtDevices::ecu_io()` (Task 2), `Pin::from_str` (`src/peripherals/gpio.rs`), the `Peripheral` trait (`src/peripherals/mod.rs:451-469`).
- Produces `Adc::new(name: &str, ext_devices: &ExtDevices) -> Option<Box<dyn Peripheral>>` (matches `Peripherals::register_peripheral`'s existing constructor-chain signature, e.g. `OtgFs::new`/`Spi::new`).
- Produces `pub(crate) fn Adc::for_test(ecu_io: Option<Rc<RefCell<EcuIo>>>) -> Self`, `Adc::register_read(&mut self, offset: u32) -> u32`, `Adc::register_write(&mut self, offset: u32, value: u32)`, `Adc::dma_read_bytes(&mut self, offset: u32, size: usize) -> VecDeque<u8>`, `pub(crate) fn Adc::millivolts_to_counts(millivolts: i32) -> u32`, and register offset constants `Adc::SR`, `Adc::CR1`, `Adc::CR2`, `Adc::SMPR1`, `Adc::SMPR2`, `Adc::SQR1`, `Adc::SQR2`, `Adc::SQR3`, `Adc::DR`.

- [x] **Step 1: Write the failing tests**

    Create `src/peripherals/adc.rs`:

    ```rust
    // SPDX-License-Identifier: GPL-3.0-or-later

    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    use super::Peripheral;
    use crate::{ext_devices::{ExtDevices, ecu_io::EcuIo}, peripherals::gpio::Pin, system::System};

    pub struct Adc {
        ecu_io: Option<Rc<RefCell<EcuIo>>>,
        channel_pins: [Pin; 16],
        cr1: u32,
        cr2: u32,
        smpr1: u32,
        smpr2: u32,
        sqr1: u32,
        sqr2: u32,
        sqr3: u32,
        sr: u32,
        sequence_index: u32,
    }

    #[cfg(test)]
    mod tests {
        use std::{cell::RefCell, rc::Rc};

        use super::Adc;
        use crate::ext_devices::ecu_io::{EcuIo, EcuIoAdcChannelConfig, EcuIoConfig};

        fn ecu_io_with(channels: &[(&str, &str, i32)]) -> Rc<RefCell<EcuIo>> {
            let config = EcuIoConfig {
                listen: "127.0.0.1:0".to_string(),
                pins: vec![],
                adc_channels: channels
                    .iter()
                    .map(|(name, pin, _)| EcuIoAdcChannelConfig {
                        name: name.to_string(),
                        pin: pin.to_string(),
                    })
                    .collect(),
            };
            let ecu_io = Rc::new(RefCell::new(EcuIo::new(config).unwrap()));
            for (name, _, value) in channels {
                ecu_io.borrow_mut().set_value(name, *value);
            }
            ecu_io
        }

        #[test]
        fn dr_reads_cycle_through_the_configured_channel_sequence_and_wrap() {
            let ecu_io = ecu_io_with(&[
                ("map", "PC0", 1500),
                ("tps", "PC1", 3300),
                ("clt", "PB0", 0),
            ]);
            let mut adc = Adc::for_test(Some(ecu_io));

            adc.register_write(Adc::SQR1, 2 << 20); // L=2 -> 3 channels
            adc.register_write(Adc::SQR3, 10 | (11 << 5) | (8 << 10)); // SQ1=10(map) SQ2=11(tps) SQ3=8(clt)

            assert_eq!(adc.register_read(Adc::DR), Adc::millivolts_to_counts(1500));
            assert_eq!(adc.register_read(Adc::DR), Adc::millivolts_to_counts(3300));
            assert_eq!(adc.register_read(Adc::DR), Adc::millivolts_to_counts(0));
            assert_eq!(adc.register_read(Adc::DR), Adc::millivolts_to_counts(1500)); // wraps
        }

        #[test]
        fn swstart_resets_the_sequence_to_the_first_channel() {
            let ecu_io = ecu_io_with(&[("map", "PC0", 1500), ("tps", "PC1", 3300)]);
            let mut adc = Adc::for_test(Some(ecu_io));
            adc.register_write(Adc::SQR1, 1 << 20); // L=1 -> 2 channels
            adc.register_write(Adc::SQR3, 10 | (11 << 5));

            adc.register_read(Adc::DR); // advance past the first channel
            adc.register_write(Adc::CR2, 1 << 30); // SWSTART

            assert_eq!(adc.register_read(Adc::DR), Adc::millivolts_to_counts(1500));
        }

        #[test]
        fn an_unconfigured_channel_reads_zero() {
            let ecu_io = ecu_io_with(&[("map", "PC0", 1500)]);
            let mut adc = Adc::for_test(Some(ecu_io));
            adc.register_write(Adc::SQR1, 0); // L=0 -> 1 channel
            adc.register_write(Adc::SQR3, 3); // channel 3 = PA3, unconfigured

            assert_eq!(adc.register_read(Adc::DR), 0);
        }

        #[test]
        fn dma_read_advances_the_sequence_once_per_halfword_not_once_per_byte() {
            let ecu_io = ecu_io_with(&[("map", "PC0", 1500), ("tps", "PC1", 3300)]);
            let mut adc = Adc::for_test(Some(ecu_io));
            adc.register_write(Adc::SQR1, 1 << 20); // L=1 -> 2 channels
            adc.register_write(Adc::SQR3, 10 | (11 << 5));

            let bytes = adc.dma_read_bytes(Adc::DR, 4); // 4 bytes = 2 halfwords = 2 conversions

            let map_counts = Adc::millivolts_to_counts(1500);
            let tps_counts = Adc::millivolts_to_counts(3300);
            let expected: Vec<u8> = vec![
                (map_counts & 0xFF) as u8,
                (map_counts >> 8) as u8,
                (tps_counts & 0xFF) as u8,
                (tps_counts >> 8) as u8,
            ];
            assert_eq!(bytes, expected.into_iter().collect::<std::collections::VecDeque<u8>>());
        }

        #[test]
        fn millivolts_to_counts_clamps_and_scales_across_the_full_range() {
            assert_eq!(Adc::millivolts_to_counts(0), 0);
            assert_eq!(Adc::millivolts_to_counts(3300), 4095);
            assert_eq!(Adc::millivolts_to_counts(-10), 0);
            assert_eq!(Adc::millivolts_to_counts(10_000), 4095);
        }
    }
    ```

- [x] **Step 2: Run tests to verify they fail**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator adc::`
    Expected: FAIL to compile — `Adc::for_test`, `register_read`, `register_write`, `dma_read_bytes`, `millivolts_to_counts`, and the offset constants don't exist yet.

- [x] **Step 3: Implement the register model**

    Add to `src/peripherals/adc.rs`, above the `#[cfg(test)]` block:

    ```rust
    impl Adc {
        pub const SR: u32 = 0x00;
        pub const CR1: u32 = 0x04;
        pub const CR2: u32 = 0x08;
        pub const SMPR1: u32 = 0x0C;
        pub const SMPR2: u32 = 0x10;
        pub const SQR1: u32 = 0x2C;
        pub const SQR2: u32 = 0x30;
        pub const SQR3: u32 = 0x34;
        pub const DR: u32 = 0x4C;

        const CR2_SWSTART: u32 = 1 << 30;

        const CHANNEL_PIN_NAMES: [&'static str; 16] = [
            "PA0", "PA1", "PA2", "PA3", "PA4", "PA5", "PA6", "PA7",
            "PB0", "PB1",
            "PC0", "PC1", "PC2", "PC3", "PC4", "PC5",
        ];

        pub fn new(name: &str, ext_devices: &ExtDevices) -> Option<Box<dyn Peripheral>> {
            if name == "ADC1" {
                Some(Box::new(Self::for_test(ext_devices.ecu_io())))
            } else {
                None
            }
        }

        pub(crate) fn for_test(ecu_io: Option<Rc<RefCell<EcuIo>>>) -> Self {
            Self {
                ecu_io,
                channel_pins: Self::CHANNEL_PIN_NAMES.map(Pin::from_str),
                cr1: 0,
                cr2: 0,
                smpr1: 0,
                smpr2: 0,
                sqr1: 0,
                sqr2: 0,
                sqr3: 0,
                sr: 0,
                sequence_index: 0,
            }
        }

        pub(crate) fn millivolts_to_counts(millivolts: i32) -> u32 {
            let clamped = millivolts.clamp(0, 3300) as u32;
            (clamped * 4095) / 3300
        }

        fn num_channels(&self) -> u32 {
            (((self.sqr1 >> 20) & 0xF) + 1).min(16)
        }

        fn channel_at(&self, n: u32) -> u32 {
            if n < 6 {
                (self.sqr3 >> (n * 5)) & 0x1F
            } else if n < 12 {
                (self.sqr2 >> ((n - 6) * 5)) & 0x1F
            } else {
                (self.sqr1 >> ((n - 12) * 5)) & 0x1F
            }
        }

        fn next_conversion_value(&mut self) -> u32 {
            let n = self.num_channels();
            let channel = self.channel_at(self.sequence_index % n) as usize;
            self.sequence_index = (self.sequence_index + 1) % n;

            let millivolts = if channel < 16 {
                self.ecu_io
                    .as_ref()
                    .map(|e| e.borrow().adc_millivolts(self.channel_pins[channel]))
                    .unwrap_or(0)
            } else {
                0
            };

            Self::millivolts_to_counts(millivolts)
        }

        pub(crate) fn register_read(&mut self, offset: u32) -> u32 {
            match offset {
                Self::SR => self.sr,
                Self::CR1 => self.cr1,
                Self::CR2 => self.cr2,
                Self::SMPR1 => self.smpr1,
                Self::SMPR2 => self.smpr2,
                Self::SQR1 => self.sqr1,
                Self::SQR2 => self.sqr2,
                Self::SQR3 => self.sqr3,
                Self::DR => self.next_conversion_value(),
                _ => 0,
            }
        }

        pub(crate) fn register_write(&mut self, offset: u32, value: u32) {
            match offset {
                Self::SR => self.sr = value,
                Self::CR1 => self.cr1 = value,
                Self::CR2 => {
                    if value & Self::CR2_SWSTART != 0 {
                        self.sequence_index = 0;
                    }
                    self.cr2 = value;
                }
                Self::SMPR1 => self.smpr1 = value,
                Self::SMPR2 => self.smpr2 = value,
                Self::SQR1 => self.sqr1 = value,
                Self::SQR2 => self.sqr2 = value,
                Self::SQR3 => self.sqr3 = value,
                _ => {}
            }
        }

        pub(crate) fn dma_read_bytes(&mut self, offset: u32, size: usize) -> VecDeque<u8> {
            if offset != Self::DR {
                let mut v = VecDeque::with_capacity(size);
                for _ in 0..size {
                    v.push_back(self.register_read(offset) as u8);
                }
                return v;
            }

            let mut v = VecDeque::with_capacity(size);
            let mut remaining = size;
            while remaining >= 2 {
                let value = self.next_conversion_value();
                v.push_back((value & 0xFF) as u8);
                v.push_back(((value >> 8) & 0xFF) as u8);
                remaining -= 2;
            }
            v
        }
    }

    impl Peripheral for Adc {
        fn read(&mut self, _sys: &System, offset: u32) -> u32 {
            self.register_read(offset)
        }

        fn write(&mut self, _sys: &System, offset: u32, value: u32) {
            self.register_write(offset, value);
        }

        fn read_dma(&mut self, _sys: &System, offset: u32, size: usize) -> VecDeque<u8> {
            self.dma_read_bytes(offset, size)
        }
    }
    ```

- [x] **Step 4: Run tests to verify they pass**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator adc::`
    Expected: PASS (all 6 tests).

- [x] **Step 5: Wire `Adc` into the peripheral registration chain**

    Modify `src/peripherals/mod.rs`:

    Add `pub mod adc;` alongside the other `pub mod` declarations (`src/peripherals/mod.rs:3-19`), and `use adc::*;` alongside the other `use` statements (`src/peripherals/mod.rs:21-38`).

    In `register_peripheral`'s constructor chain (`src/peripherals/mod.rs:191-206`), add:

    ```rust
            .or_else(|| Adc::new(&name, ext_devices))
    ```

    anywhere after `.or_else(|| Spi::new(&name, ext_devices))` (order among the `ext_devices`-consuming constructors doesn't matter — each only matches its own SVD name).

- [x] **Step 6: Run the full test suite**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator`
    Expected: PASS.

- [x] **Step 7: Commit**

    ```bash
    git add src/peripherals/adc.rs src/peripherals/mod.rs
    git commit -m "feat: add minimal ADC1 peripheral sourced from EcuIo"
    ```

---

### Task 4: Model EXTI/SYSCFG so crank/cam edges actually reach firmware

**Ground truth used by this task:** rusEFI's trigger input is genuinely EXTI-driven, not polled — confirmed by reading `firmware/hw_layer/digital_input/digital_input_exti.cpp` in `epicefi_fw`: `efiExtiEnablePin` calls ChibiOS's `palEnableLineEvent`, which configures the real STM32 EXTI peripheral (line unmask + edge selection) and the SYSCFG `EXTICRx` registers (which GPIO port is routed to which EXTI line — STM32 EXTI lines are shared across ports, e.g. line 6 could be PA6, PB6, or PC6, selected by a 4-bit field in `SYSCFG_EXTICRx`). Without this task, Task 2's GPIO read-callback approach makes `crank`/`cam` visible only if firmware happens to poll GPIO IDR — which it never does; it waits for an EXTI interrupt that nothing currently generates.

Register layout confirmed directly against `proteus_f7/STM32F767.svd` (not guessed): EXTI peripheral base `0x40013C00` (`STM32F767.svd:27421`), `IMR` at offset `0x00` (`:27520`), `PR` at offset `0x14` (`:27686`) — the standard, full STM32F4/F7 EXTI layout (`EMR`=0x04, `RTSR`=0x08, `FTSR`=0x0C, `SWIER`=0x10 follow the same well-known spacing). SYSCFG peripheral base `0x40013800` (`:10329`), with `EXTICR1`-`EXTICR4` at offsets `0x08`/`0x0C`/`0x10`/`0x14` (`:10455`,`:10495`,`:10535`,`:10574`), each a 4-bit field per line selecting the port (0=PA, 1=PB, 2=PC, ... — matching `GpioPorts::port_index`'s existing 0-indexed-by-letter encoding exactly, no translation needed). Interrupt vector numbers, also read directly from the SVD: EXTI0=6, EXTI1=7, EXTI2=8, EXTI3=9, EXTI4=10 (`:27439-27462`), EXTI9_5=23 (`:27464-27467`), EXTI15_10=40 (`:27469-27472`).

**Architectural precedent used by this task:** `NvicWrapper` (`src/peripherals/nvic.rs:305-324`) is the exact pattern for a peripheral whose real state must be reachable from *other* code, not just MMIO dispatch: the real state lives in a named field on `Peripherals` (`pub nvic: RefCell<Nvic>`, alongside the existing `pub gpio: RefCell<GpioPorts>`), and a stateless marker struct implementing `Peripheral` is registered in the normal constructor chain, with `read`/`write` simply delegating to `sys.p.nvic.borrow_mut().read(sys, offset)`/`write(...)`. This task adds `pub exti: RefCell<Exti>` the same way, with two thin wrapper types (`ExtiWrapper` for the "EXTI" SVD name, `SyscfgWrapper` for "SYSCFG") delegating into the one shared `Exti` instance's `read_exti`/`write_exti` and `read_syscfg`/`write_syscfg` methods respectively — this is why SYSCFG is folded into `exti.rs` rather than becoming its own peripheral file: the only part of SYSCFG modeled is `EXTICRx`, which exists purely to answer "which port is this EXTI line routed to," a question only `Exti`'s own logic ever needs to answer.

**Why the interrupt-raising logic stays `&System`-free:** every unit-tested peripheral in this codebase keeps its real logic in `&System`-free inherent methods, with the `Peripheral` trait impl (which does take `&System`) as thin, untested glue — e.g. `OtgFs::register_read`/`register_write` vs. its `Peripheral::read`/`write`. `Exti::raise_line_if_configured` follows the same shape: it takes no `&System`, returns `Option<i32>` (the IRQ to raise, or `None`), and is fully unit-testable. The one line of code that actually calls `sys.p.nvic.borrow_mut().set_intr_pending(irq)` lives in `EcuIo::check_digital_edges`, verified live rather than by unit test — matching how `OtgFs::poll()`'s own `sys.p.nvic.borrow_mut().set_intr_pending(67)` call is verified live, not unit tested.

**Timing note:** per this project's ECU I/O design spec, precise real-time edge timing is explicitly out of scope. `EcuIo::check_digital_edges` runs once per `poll()` tick (the same ~100,000-instruction/"~1-10ms" cadence already used for the TCP bridge and ADC bridge), not immediately on receipt — an edge is noticed within about one poll tick, not instantly. This is consistent with, not a regression from, the already-agreed scope boundary.

**Files:**
- Create: `src/peripherals/exti.rs`
- Modify: `src/peripherals/mod.rs`
- Modify: `src/peripherals/gpio.rs`
- Modify: `src/ext_devices/ecu_io.rs`
- Modify: `src/ext_devices/mod.rs`
- Modify: `src/emulator.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/peripherals/exti.rs` and `src/ext_devices/ecu_io.rs`

**Interfaces:**
- Consumes: `GpioPorts::port_index` (`src/peripherals/gpio.rs:36-41`, unchanged), `Nvic::set_intr_pending` (`src/peripherals/nvic.rs:35-40`, unchanged), `EcuIo::digital_level` (Task 1).
- Produces `Pin::port(&self) -> u8` and `Pin::number(&self) -> u8` (small accessors on the existing `Pin` type) — consumed by `EcuIo::check_digital_edges`.
- Produces `Exti::raise_line_if_configured(&mut self, port: u8, pin: u8, rising: bool) -> Option<i32>`, `Exti::read_exti`/`write_exti`/`read_syscfg`/`write_syscfg` (all `pub(crate)`), and `Peripherals.exti: RefCell<Exti>`.
- Produces `EcuIo::check_digital_edges(&mut self, sys: &System)` — called from `ExtDevices::poll`, which changes signature from `poll(&self)` to `poll(&self, sys: &System)` (Task 2's version had no `&System` parameter; this task adds it). `EcuIo::poll()` itself (Task 1's TCP I/O) is unchanged.

- [x] **Step 1: Write the failing `Exti` tests**

    Create `src/peripherals/exti.rs`:

    ```rust
    // SPDX-License-Identifier: GPL-3.0-or-later

    use super::Peripheral;
    use crate::system::System;

    #[derive(Default)]
    pub struct Exti {
        imr: u32,
        emr: u32,
        rtsr: u32,
        ftsr: u32,
        swier: u32,
        pr: u32,
        exticr1: u32,
        exticr2: u32,
        exticr3: u32,
        exticr4: u32,
    }

    #[cfg(test)]
    mod tests {
        use super::Exti;

        fn route_line_6_to_port_c() -> Exti {
            let mut exti = Exti::default();
            // EXTICR2 covers lines 4-7; line 6 is bits [11:8]; port C = 2.
            exti.write_syscfg(Exti::EXTICR2, 2 << 8);
            exti.write_exti(Exti::IMR, 1 << 6);
            exti.write_exti(Exti::RTSR, 1 << 6);
            exti
        }

        #[test]
        fn a_rising_edge_on_an_unmasked_correctly_routed_line_raises_the_shared_irq() {
            let mut exti = route_line_6_to_port_c();
            let port_c = 2;
            assert_eq!(exti.raise_line_if_configured(port_c, 6, true), Some(23)); // EXTI9_5
            assert_eq!(exti.read_exti(Exti::PR) & (1 << 6), 1 << 6);
        }

        #[test]
        fn a_falling_edge_does_not_fire_when_only_rising_is_selected() {
            let mut exti = route_line_6_to_port_c();
            assert_eq!(exti.raise_line_if_configured(2, 6, false), None);
        }

        #[test]
        fn a_line_routed_to_a_different_port_does_not_fire() {
            let mut exti = route_line_6_to_port_c();
            let port_a = 0;
            assert_eq!(exti.raise_line_if_configured(port_a, 6, true), None);
        }

        #[test]
        fn a_masked_line_does_not_fire() {
            let mut exti = route_line_6_to_port_c();
            exti.write_exti(Exti::IMR, 0);
            assert_eq!(exti.raise_line_if_configured(2, 6, true), None);
        }

        #[test]
        fn pending_bit_clears_on_write_one() {
            let mut exti = route_line_6_to_port_c();
            exti.raise_line_if_configured(2, 6, true);
            exti.write_exti(Exti::PR, 1 << 6);
            assert_eq!(exti.read_exti(Exti::PR) & (1 << 6), 0);
        }

        #[test]
        fn irq_numbers_match_the_svd_for_each_line_group() {
            // Line 0-4 each have a dedicated vector; 5-9 share EXTI9_5; 10-15 share EXTI15_10.
            let mut exti = Exti::default();
            exti.write_syscfg(Exti::EXTICR1, 0); // lines 0-3 -> port A
            exti.write_exti(Exti::IMR, 0b1111);
            exti.write_exti(Exti::RTSR, 0b1111);
            assert_eq!(exti.raise_line_if_configured(0, 0, true), Some(6));
            assert_eq!(exti.raise_line_if_configured(0, 1, true), Some(7));
            assert_eq!(exti.raise_line_if_configured(0, 2, true), Some(8));
            assert_eq!(exti.raise_line_if_configured(0, 3, true), Some(9));
        }
    }
    ```

- [x] **Step 2: Run tests to verify they fail**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator exti::`
    Expected: FAIL to compile — `write_syscfg`, `write_exti`, `read_exti`, `raise_line_if_configured`, and the offset constants don't exist yet.

- [x] **Step 3: Implement `Exti`**

    Add to `src/peripherals/exti.rs`, above the `#[cfg(test)]` block:

    ```rust
    impl Exti {
        pub const IMR: u32 = 0x00;
        pub const EMR: u32 = 0x04;
        pub const RTSR: u32 = 0x08;
        pub const FTSR: u32 = 0x0C;
        pub const SWIER: u32 = 0x10;
        pub const PR: u32 = 0x14;

        pub const EXTICR1: u32 = 0x08;
        pub const EXTICR2: u32 = 0x0C;
        pub const EXTICR3: u32 = 0x10;
        pub const EXTICR4: u32 = 0x14;

        pub(crate) fn read_exti(&mut self, offset: u32) -> u32 {
            match offset {
                Self::IMR => self.imr,
                Self::EMR => self.emr,
                Self::RTSR => self.rtsr,
                Self::FTSR => self.ftsr,
                Self::SWIER => self.swier,
                Self::PR => self.pr,
                _ => 0,
            }
        }

        pub(crate) fn write_exti(&mut self, offset: u32, value: u32) {
            match offset {
                Self::IMR => self.imr = value,
                Self::EMR => self.emr = value,
                Self::RTSR => self.rtsr = value,
                Self::FTSR => self.ftsr = value,
                Self::SWIER => self.swier = value,
                Self::PR => self.pr &= !value,
                _ => {}
            }
        }

        pub(crate) fn read_syscfg(&mut self, offset: u32) -> u32 {
            match offset {
                Self::EXTICR1 => self.exticr1,
                Self::EXTICR2 => self.exticr2,
                Self::EXTICR3 => self.exticr3,
                Self::EXTICR4 => self.exticr4,
                _ => 0,
            }
        }

        pub(crate) fn write_syscfg(&mut self, offset: u32, value: u32) {
            match offset {
                Self::EXTICR1 => self.exticr1 = value,
                Self::EXTICR2 => self.exticr2 = value,
                Self::EXTICR3 => self.exticr3 = value,
                Self::EXTICR4 => self.exticr4 = value,
                _ => {}
            }
        }

        fn exticr_port_for_line(&self, line: u8) -> u8 {
            let (reg, shift) = match line {
                0..=3 => (self.exticr1, line * 4),
                4..=7 => (self.exticr2, (line - 4) * 4),
                8..=11 => (self.exticr3, (line - 8) * 4),
                _ => (self.exticr4, (line - 12) * 4),
            };
            ((reg >> shift) & 0xF) as u8
        }

        fn irq_for_line(line: u8) -> i32 {
            match line {
                0 => 6,
                1 => 7,
                2 => 8,
                3 => 9,
                4 => 10,
                5..=9 => 23,
                _ => 40,
            }
        }

        pub fn raise_line_if_configured(&mut self, port: u8, pin: u8, rising: bool) -> Option<i32> {
            if pin > 15 {
                return None;
            }
            if self.exticr_port_for_line(pin) != port {
                return None;
            }
            if self.imr & (1 << pin) == 0 {
                return None;
            }
            let edge_matches = (rising && self.rtsr & (1 << pin) != 0)
                || (!rising && self.ftsr & (1 << pin) != 0);
            if !edge_matches {
                return None;
            }

            self.pr |= 1 << pin;
            Some(Self::irq_for_line(pin))
        }
    }

    pub struct ExtiWrapper;

    impl ExtiWrapper {
        pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
            if name == "EXTI" {
                Some(Box::new(Self))
            } else {
                None
            }
        }
    }

    impl Peripheral for ExtiWrapper {
        fn read(&mut self, sys: &System, offset: u32) -> u32 {
            sys.p.exti.borrow_mut().read_exti(offset)
        }

        fn write(&mut self, sys: &System, offset: u32, value: u32) {
            sys.p.exti.borrow_mut().write_exti(offset, value);
        }
    }

    pub struct SyscfgWrapper;

    impl SyscfgWrapper {
        pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
            if name == "SYSCFG" {
                Some(Box::new(Self))
            } else {
                None
            }
        }
    }

    impl Peripheral for SyscfgWrapper {
        fn read(&mut self, sys: &System, offset: u32) -> u32 {
            sys.p.exti.borrow_mut().read_syscfg(offset)
        }

        fn write(&mut self, sys: &System, offset: u32, value: u32) {
            sys.p.exti.borrow_mut().write_syscfg(offset, value);
        }
    }
    ```

- [x] **Step 4: Run tests to verify they pass**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator exti::`
    Expected: PASS (all 6 tests).

- [x] **Step 5: Wire `Exti` into `Peripherals`**

    Modify `src/peripherals/mod.rs`:

    Add `pub mod exti;` alongside the other `pub mod` declarations, and `use exti::*;` alongside the other `use` statements.

    Add to the `Peripherals` struct (`src/peripherals/mod.rs:53-59`):

    ```rust
        pub exti: RefCell<Exti>,
    ```

    In `register_peripheral`'s constructor chain (`src/peripherals/mod.rs:191-206`), add:

    ```rust
            .or_else(|| ExtiWrapper::new(&name))
            .or_else(|| SyscfgWrapper::new(&name))
    ```

- [x] **Step 6: Add `Pin::port`/`Pin::number` accessors**

    Modify `src/peripherals/gpio.rs`, in `impl Pin` (`src/peripherals/gpio.rs:16-27`), add:

    ```rust
        pub fn port(&self) -> u8 {
            self.port
        }

        pub fn number(&self) -> u8 {
            self.pin
        }
    ```

- [x] **Step 7: Write the failing `EcuIo::check_digital_edges` test**

    Add to `src/ext_devices/ecu_io.rs`'s `mod tests`:

    ```rust
    #[test]
    fn a_digital_input_level_change_raises_the_configured_exti_line() {
        let mut gpio = GpioPorts::default();
        let config = EcuIoConfig {
            listen: "127.0.0.1:0".to_string(),
            pins: vec![super::EcuIoPinConfig {
                name: "crank".to_string(),
                pin: "PC6".to_string(),
                direction: super::EcuIoPinDirection::Input,
            }],
            adc_channels: vec![],
        };
        let ecu_io = EcuIo::register(config, &mut gpio).unwrap();

        let (mut uc, p, d) = test_parts();
        p.exti.borrow_mut().write_syscfg(crate::peripherals::exti::Exti::EXTICR2, 2 << 8);
        p.exti.borrow_mut().write_exti(crate::peripherals::exti::Exti::IMR, 1 << 6);
        p.exti.borrow_mut().write_exti(crate::peripherals::exti::Exti::RTSR, 1 << 6);
        let sys = System { uc: RefCell::new(&mut uc), p: p.clone(), d };

        ecu_io.borrow_mut().set_value("crank", 1);
        ecu_io.borrow_mut().check_digital_edges(&sys);

        assert_eq!(
            p.exti.borrow_mut().read_exti(crate::peripherals::exti::Exti::PR) & (1 << 6),
            1 << 6
        );
    }
    ```

    This depends on the same `test_parts()` helper Task 2 added — reuse it (do not write a second one). Note the config (`EXTICR2`/`IMR`/`RTSR`) is written directly on `p.exti` *before* constructing `sys` (since `sys` holds a mutable borrow of `uc` for the rest of the test, `p` itself — an `Rc`, cheaply cloneable — remains freely usable either side of that point).

- [x] **Step 8: Run test to verify it fails**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator ecu_io`
    Expected: FAIL to compile — `check_digital_edges` doesn't exist yet.

- [x] **Step 9: Implement `check_digital_edges`**

    Modify `src/ext_devices/ecu_io.rs`'s `EcuIo` struct to add two fields:

    ```rust
        digital_input_pins: Vec<(Pin, String)>,
        last_digital_levels: HashMap<String, bool>,
    ```

    Modify `EcuIo::new` (Task 1) to also compute `digital_input_pins`, and include both new fields in the returned `Self`:

    ```rust
        let digital_input_pins = config
            .pins
            .iter()
            .filter(|p| p.direction == EcuIoPinDirection::Input)
            .map(|p| (Pin::from_str(&p.pin), p.name.clone()))
            .collect();
    ```

    ```rust
            digital_input_pins,
            last_digital_levels: HashMap::new(),
    ```

    Add a new method to `impl EcuIo`:

    ```rust
        pub fn check_digital_edges(&mut self, sys: &System) {
            for (pin, name) in self.digital_input_pins.clone() {
                let level = self.digital_level(&name);
                let previous = self.last_digital_levels.get(&name).copied().unwrap_or(false);
                if level != previous {
                    if let Some(irq) = sys.p.exti.borrow_mut().raise_line_if_configured(pin.port(), pin.number(), level) {
                        sys.p.nvic.borrow_mut().set_intr_pending(irq);
                    }
                    self.last_digital_levels.insert(name, level);
                }
            }
        }
    ```

    Add `use crate::system::System;` to `src/ext_devices/ecu_io.rs`'s imports.

    Modify `ExtDevices::poll` (`src/ext_devices/mod.rs:82-88`) to accept `sys: &System` and call the new method:

    ```rust
    pub fn poll(&self, sys: &System) {
        for bridge in &self.usb_cdc_tcps {
            if let Err(error) = bridge.borrow_mut().poll() {
                warn!("USB CDC TCP bridge error: {error:#}");
            }
        }
        for ecu_io in &self.ecu_ios {
            if let Err(error) = ecu_io.borrow_mut().poll() {
                warn!("ECU IO bridge error: {error:#}");
            }
            ecu_io.borrow_mut().check_digital_edges(sys);
        }
    }
    ```

    Add `use crate::system::System;` to `src/ext_devices/mod.rs`'s imports if not already present.

    Modify `src/emulator.rs:142`, changing `d.poll();` to `d.poll(&sys);` (the `sys` local is already constructed on the preceding lines for `p.poll(&sys)`).

- [x] **Step 10: Run tests to verify they pass**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator`
    Expected: PASS — all tests, including the full suite (no other call site broken by `ExtDevices::poll`'s new parameter; `emulator.rs` was the only other caller).

- [x] **Step 11: Commit**

    ```bash
    git add src/peripherals/exti.rs src/peripherals/mod.rs src/peripherals/gpio.rs src/ext_devices/ecu_io.rs src/ext_devices/mod.rs src/emulator.rs
    git commit -m "feat: model EXTI/SYSCFG so external digital inputs reach firmware"
    ```

---

### Task 5: Configure Proteus F7 and verify live

**Ground truth used by this task:** `proteus_f7/config.yaml`'s current `devices:` section only has `usb_cdc_tcp` (see `docs/proteus-f7-usb.md`'s existing "Launch"/"Connecting" sections for the doc style to match). `proteus_f7/verify_boot.ps1:22-24` is the existing pattern for a YAML-content assertion (grepping the config file for expected literal strings) — mirror that, don't invent a new verification mechanism.

The exact pin for the cam trigger input is genuinely unconfirmed — rusEFI's cam signal usage is itself a user-configurable trigger setting (unlike the crank input, which this project's earlier research already confirmed defaults to `PROTEUS_DIGITAL_1` = PC6). This task uses `PE11` (`PROTEUS_DIGITAL_2`) as a reasonable default and calls this out explicitly in the doc as something to confirm against whatever trigger configuration is actually set in TunerStudio, rather than presenting it as verified fact.

**Files:**
- Modify: `proteus_f7/config.yaml`
- Modify: `proteus_f7/verify_boot.ps1`
- Create: `docs/proteus-f7-ecu-io.md`
- Modify: `docs/superpowers/plans/2026-07-11-proteus-f7-ecu-io.md` (this file — check off steps as completed, per this project's established convention from the virtual-usb plan)

**Interfaces:**
- Consumes: `EcuIoConfig` YAML schema (Task 1/2), nothing new produced for later tasks — this is the final task.

- [x] **Step 1: Add a failing YAML assertion**

    In `proteus_f7/verify_boot.ps1`, alongside the existing `usb_cdc_tcp` assertions (near line 22-24), add a check that `config.yaml` contains:

    ```
    'ecu_io:'
    'listen: 127.0.0.1:29002'
    'name: crank'
    ```

    (match the exact style already used for the `usb_cdc_tcp` checks in that file — read the surrounding lines first to mirror the pattern precisely, since the exact assertion mechanism should already be established there.)

- [x] **Step 2: Verify failure**

    Run: `.\proteus_f7\verify_boot.ps1`
    Expected: FAIL — `config.yaml` has no `ecu_io` device yet.

- [x] **Step 3: Add the ECU I/O device configuration**

    Add under `devices:` in `proteus_f7/config.yaml`, alongside the existing `usb_cdc_tcp` entry:

    ```yaml
      ecu_io:
        - listen: 127.0.0.1:29002
          pins:
            - { name: crank, pin: PC6, direction: input }
            - { name: cam,   pin: PE11, direction: input }
            - { name: inj1,  pin: PD7, direction: output }
            - { name: ign1,  pin: PD4, direction: output }
          adc_channels:
            - { name: map, pin: PC0 }
            - { name: tps, pin: PC1 }
            - { name: clt, pin: PB0 }
            - { name: iat, pin: PC5 }
            - { name: vbatt, pin: PA7 }
    ```

- [x] **Step 4: Document operation**

    Create `docs/proteus-f7-ecu-io.md`:

    ```markdown
    # Proteus F7 ECU I/O

    A second TCP bridge, independent of the USB CDC one, lets an external
    process drive the digital and analog signals rusEFI reads as engine
    sensor inputs, and observe the outputs it drives in response. The
    emulator does not generate any of these signals itself — it only
    reflects whatever it's told.

    ## Launch

    Same emulator process as `docs/proteus-f7-usb.md` — this is a second,
    independent listener on the same run.

    ## Protocol

    Connect with any raw TCP client, e.g. `ncat 127.0.0.1 29002`. One line
    per message, `name=value`:

    - Digital input pins (`crank`, `cam`): `value` is `0` or `1`.
    - ADC channels (`map`, `tps`, `clt`, `iat`, `vbatt`): `value` is
      millivolts, clamped to 0-3300 (VREF+).
    - Output pins (`inj1`, `ign1`): the emulator sends `name=value` lines
      to the connected client whenever firmware drives that pin to a new
      level. There is nothing to send for these — they are observed, not
      driven.

    Only one client at a time, same rule as the USB CDC bridge
    (`docs/proteus-f7-usb.md`'s "One-client rule and disconnects").

    ## Current signal set

    Digital inputs: `crank` (PC6, confirmed as this board's default crank
    trigger pin). `cam` (PE11) is a best-effort default — confirm it
    against whatever trigger configuration is actually set in TunerStudio
    for your engine, since cam wiring is itself a user-configurable rusEFI
    setting, not a board-fixed default like crank.

    Analog inputs: `map`, `tps`, `clt`, `iat`, `vbatt` — all routed through
    ADC1's real channel-to-pin mapping (channels 10, 11, 8, 15, 7
    respectively), confirmed from rusEFI's Proteus board source.

    Observed outputs: `inj1` (PD7, injector 1 low-side driver), `ign1`
    (PD4, ignition 1 driver) — a small starting set; extending to more
    injector/ignition channels is a config-only change (add more `pins`
    entries), no code change needed.
    ```

- [ ] **Step 5: Verify the full route** (partial — see `docs/proteus-f7-ecu-io.md`'s "Verification" section: config assertions and TCP wiring confirmed live; firmware-side ADC/EXTI activation for crank/MAP was not observed within a multi-billion-instruction capture, isolated from a general regression since the same run's USB CDC/TunerStudio path responded normally)

    Run: `.\proteus_f7\verify_boot.ps1`
    Expected: PASS through the new `ecu_io` assertions.

    Run the emulator against the real firmware (`cargo run --release --bin stm32-emulator -- config.yaml -v` from `proteus_f7/`), connect one TCP client to `127.0.0.1:29002`, and:

    1. Send `map=1500\n`.
    2. Over the *already-working* TunerStudio USB CDC connection (`docs/proteus-f7-usb.md`), query rusEFI's live sensor data and confirm it reports back a MAP reading consistent with 1500mV (convert via whatever MAP sensor curve is configured — this project isn't asserting an exact rusEFI-computed physical unit, just that the raw ADC reading changed in the expected direction and magnitude when the input changed).
    3. Send `crank=1\n` then `crank=0\n` a few times with a `-vvvv` capture running (same Monitor-gated-connect workflow used for the OTG-FS verification in `proteus_f7/usb_trace_notes.md`) and confirm the EXTI9_5 interrupt (vector 23) actually fires — grep the capture log for the NVIC entering that vector, or for firmware's `NVIC->STIR = I2C1_EV_IRQn` software-trigger write (`digital_input_exti.cpp:110`) shortly after. Full trigger *sync* (rusEFI reporting a computed RPM) is not expected or required for this milestone — a fixed number of toggles is not a real tooth pattern — only that the interrupt path genuinely fires is being confirmed here.

    Append a "## Verification" section to `docs/proteus-f7-ecu-io.md` with the exact observed bytes/values from both checks above.

- [x] **Step 6: Final checks and commit**

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --bin stm32-emulator`
    Expected: PASS.

    Run: `CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo build --release --bin stm32-emulator`
    Expected: PASS.

    Run: `git diff --check`
    Expected: no whitespace errors.

    ```bash
    git add proteus_f7/config.yaml proteus_f7/verify_boot.ps1 docs/proteus-f7-ecu-io.md docs/superpowers/plans/2026-07-11-proteus-f7-ecu-io.md
    git commit -m "feat: configure and verify Proteus F7 ECU I/O over TCP"
    ```
