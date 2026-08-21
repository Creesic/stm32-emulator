// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    rc::Rc,
};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::peripherals::gpio::{GpioPorts, Pin};
use crate::system::System;

use super::embedded_ecu_io::{take_pending_device_link, EmbeddedEcuIoLink};

/// Defensive cap on `recv_buffer`/`outgoing` growth against a stalled or malicious
/// client; not user-configurable, mirroring `usb_cdc_tcp`'s `max_buffered_bytes` default.
const MAX_BUFFERED_BYTES: usize = 65536;

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
pub struct EcuIoTriggerWheelConfig {
    /// Inbound `name=value` signal that sets the wheel's RPM (e.g. `trigger_rpm`).
    pub signal: String,
    /// Existing `direction: input` pin signal whose level the wheel drives
    /// (e.g. `din1` -> PC6, the stock Proteus crank input).
    pub pin_signal: String,
    pub teeth: u32,
    pub missing: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EcuIoConfig {
    pub listen: String,
    #[serde(default)]
    pub pins: Vec<EcuIoPinConfig>,
    #[serde(default)]
    pub adc_channels: Vec<EcuIoAdcChannelConfig>,
    /// Optional instruction-clock-paced crank trigger generator; see
    /// docs/superpowers/specs/2026-07-16-ecu-io-trigger-wheel-design.md.
    #[serde(default)]
    pub trigger_wheel: Option<EcuIoTriggerWheelConfig>,
}

/// Missing-tooth crank wheel advanced on the emulator's instruction clock
/// (1 instruction = 1 CPU cycle at 216 MHz, the same convention SysTick and
/// TIM5 use), so tooth timing is exact in firmware time by construction --
/// unlike a TCP feeder, whose wall-clock pacing the firmware cannot decode.
struct TriggerWheel {
    config: EcuIoTriggerWheelConfig,
    /// Fractional revolutions, wrapped to [0, 1).
    position: f64,
    last_instructions: u64,
}

impl TriggerWheel {
    /// See tim5.rs's INSTRUCTIONS_PER_TICK: firmware's clocks all assume
    /// 216e6 instructions per second, so one revolution at `rpm` spans
    /// 60/rpm * 216e6 instructions.
    const INSTRUCTIONS_PER_MINUTE: f64 = 60.0 * 216_000_000.0;

    fn new(config: EcuIoTriggerWheelConfig) -> Self {
        Self {
            config,
            position: 0.0,
            last_instructions: 0,
        }
    }

    /// The wheel's output level at a position: teeth occupy the first half
    /// of each slot (50% duty), and the last `missing` slots have no tooth
    /// (e.g. 60-2's gap). Matches AngeES's EcuIoClient wheel model.
    fn level_at(&self, position: f64) -> bool {
        let slot_f = position.fract() * self.config.teeth as f64;
        let slot = slot_f as u32;
        let in_first_half = (slot_f - slot as f64) < 0.5;
        slot < self.config.teeth - self.config.missing && in_first_half
    }

    /// Projects the wheel angle at `now_instructions` in millidegrees
    /// [0, 360_000) without advancing the wheel, so output events recorded
    /// between `advance` calls can be stamped exactly without disturbing
    /// tooth-edge delivery.
    fn project_angle_mdeg(&self, now_instructions: u64, rpm: i32) -> i32 {
        let delta = now_instructions.saturating_sub(self.last_instructions);
        let rpm = rpm.clamp(0, 30_000) as f64;
        let position =
            (self.position + delta as f64 * rpm / Self::INSTRUCTIONS_PER_MINUTE).fract();
        (position * 360_000.0) as i32
    }

    /// Advances to `now_instructions` at `rpm` and returns the new level,
    /// or None while stopped (rpm <= 0): the level holds and time simply
    /// passes. Position stays continuous across RPM changes.
    fn advance(&mut self, now_instructions: u64, rpm: i32) -> Option<bool> {
        let delta = now_instructions.saturating_sub(self.last_instructions);
        self.last_instructions = now_instructions;
        if rpm <= 0 {
            return None;
        }
        let rpm = rpm.min(30_000) as f64;
        self.position =
            (self.position + delta as f64 * rpm / Self::INSTRUCTIONS_PER_MINUTE).fract();
        Some(self.level_at(self.position))
    }
}

pub struct EcuIo {
    pub config: EcuIoConfig,
    listener: Option<TcpListener>,
    embedded: Option<EmbeddedEcuIoLink>,
    client: Option<TcpStream>,
    recv_buffer: Vec<u8>,
    outgoing: VecDeque<u8>,
    values: HashMap<String, i32>,
    adc_channels: Vec<(Pin, String)>,
    digital_input_pins: Vec<(Pin, String)>,
    last_digital_levels: HashMap<String, bool>,
    /// Every name configured as a pin (either direction) or ADC channel;
    /// `set_value` uses this to reject typo'd/unconfigured feeder names
    /// instead of storing them unbounded (see the design doc's "unknown
    /// name ... logged and ignored" promise).
    known_names: HashSet<String>,
    trigger_wheel: Option<TriggerWheel>,
}

impl EcuIo {
    pub fn new(config: EcuIoConfig) -> Result<Self> {
        let embedded = take_pending_device_link();
        let listener = if embedded.is_some() {
            None
        } else {
            let listener = TcpListener::bind(&config.listen)
                .with_context(|| format!("Failed to listen for ECU IO at {}", config.listen))?;
            listener
                .set_nonblocking(true)
                .context("Failed to make ECU IO listener nonblocking")?;
            Some(listener)
        };

        let adc_channels = config
            .adc_channels
            .iter()
            .map(|c| (Pin::from_str(&c.pin), c.name.clone()))
            .collect();

        let digital_input_pins = config
            .pins
            .iter()
            .filter(|p| p.direction == EcuIoPinDirection::Input)
            .map(|p| (Pin::from_str(&p.pin), p.name.clone()))
            .collect();

        let known_names = config
            .pins
            .iter()
            .map(|p| p.name.clone())
            .chain(config.adc_channels.iter().map(|c| c.name.clone()))
            .chain(config.trigger_wheel.iter().map(|t| t.signal.clone()))
            .collect();

        let trigger_wheel = config.trigger_wheel.clone().map(TriggerWheel::new);

        if let Some(link) = embedded.as_ref() {
            link.attach_device();
        }

        Ok(Self {
            config,
            listener,
            embedded,
            client: None,
            recv_buffer: Vec::new(),
            outgoing: VecDeque::new(),
            values: HashMap::new(),
            adc_channels,
            digital_input_pins,
            last_digital_levels: HashMap::new(),
            known_names,
            trigger_wheel,
        })
    }

    /// Advances the optional trigger wheel to `now_instructions` and drives
    /// its pin signal's stored level. Edge delivery is NOT done here: the
    /// caller runs `check_digital_edges` right after, which detects the
    /// level change and raises the EXTI line exactly as if the level had
    /// arrived as a `name=value` line over TCP.
    pub fn advance_trigger_wheel(&mut self, now_instructions: u64) {
        let Some(wheel) = self.trigger_wheel.as_mut() else {
            return;
        };
        let rpm = self.values.get(&wheel.config.signal).copied().unwrap_or(0);
        if let Some(level) = wheel.advance(now_instructions, rpm) {
            let pin_signal = wheel.config.pin_signal.clone();
            self.values.insert(pin_signal, level as i32);
        }
    }

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
                    gpio.add_write_callback(pin, move |_sys, v| {
                        s.borrow_mut().report_output(&name, v)
                    });
                }
            }
        }

        Ok(self_)
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.listener
            .as_ref()
            .context("Embedded ECU I/O has no TCP listener")?
            .local_addr()
            .context("Failed to read ECU IO listener address")
    }

    pub fn poll(&mut self) -> Result<()> {
        if let Some(link) = self.embedded.clone() {
            for (name, value) in link.take_inputs() {
                self.set_value(&name, value);
            }
            return Ok(());
        }
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

    pub fn check_digital_edges(&mut self, sys: &System) {
        // Iterate a direct field borrow (not `self.digital_level(...)`, which would
        // reborrow all of `self`) so `self.last_digital_levels` can be updated in the
        // same pass without cloning `digital_input_pins` on every call.
        for (pin, name) in &self.digital_input_pins {
            let level = self.values.get(name).copied().unwrap_or(0) != 0;
            let previous = self.last_digital_levels.get(name).copied().unwrap_or(false);
            if level != previous {
                if let Some(irq) = sys.p.exti.borrow_mut().raise_line_if_configured(
                    pin.port(),
                    pin.number(),
                    level,
                ) {
                    sys.p.nvic.borrow_mut().set_intr_pending(irq);
                }
                self.last_digital_levels.insert(name.clone(), level);
            }
        }
    }

    pub fn report_output(&mut self, name: &str, level: bool) {
        // `values` already doubles as "last known level" for this name (as it does for
        // `check_digital_edges`'s `last_digital_levels`), so compare against it before
        // queuing: GPIO's BSRR write path calls back on every bit in the set/reset mask,
        // not just actual changes, unlike ODR's XOR'd change detection.
        let new_value = level as i32;
        let changed = self.values.get(name).copied() != Some(new_value);
        self.values.insert(name.to_string(), new_value);
        if changed {
            if let Some(link) = self.embedded.as_ref() {
                let wheel_angle_mdeg = self.trigger_wheel.as_ref().map(|wheel| {
                    let rpm = self.values.get(&wheel.config.signal).copied().unwrap_or(0);
                    wheel.project_angle_mdeg(crate::emulator::instruction_count(), rpm)
                });
                link.publish_output(name, new_value, wheel_angle_mdeg);
                return;
            }
        }
        if changed && self.client.is_some() {
            let line = format!("{name}={new_value}\n");
            Self::push_capped_deque(&mut self.outgoing, &line.into_bytes(), MAX_BUFFERED_BYTES);
        }
    }

    pub(crate) fn set_value(&mut self, name: &str, value: i32) {
        if !self.known_names.contains(name) {
            warn!("ECU IO: unknown signal name {name:?}, ignoring");
            return;
        }
        self.values.insert(name.to_string(), value);
    }

    fn accept_clients(&mut self) -> Result<()> {
        loop {
            let accepted = match self.listener.as_ref() {
                Some(listener) => listener.accept(),
                None => return Ok(()),
            };
            match accepted {
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
                    Ok(count) => Self::push_capped_vec(
                        &mut self.recv_buffer,
                        &buffer[..count],
                        MAX_BUFFERED_BYTES,
                    ),
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

    /// Appends `bytes`, dropping the oldest bytes in `buffer` if that would exceed `capacity`.
    fn push_capped_vec(buffer: &mut Vec<u8>, bytes: &[u8], capacity: usize) {
        buffer.extend_from_slice(bytes);
        if buffer.len() > capacity {
            let excess = buffer.len() - capacity;
            buffer.drain(..excess);
        }
    }

    /// Mirrors `usb_cdc_tcp::UsbCdcTcp::push_capped`: pops from the front to make room.
    fn push_capped_deque(queue: &mut VecDeque<u8>, bytes: &[u8], capacity: usize) {
        for &byte in bytes {
            if queue.len() == capacity {
                queue.pop_front();
            }
            queue.push_back(byte);
        }
    }
}

impl Drop for EcuIo {
    fn drop(&mut self) {
        if let Some(link) = self.embedded.as_ref() {
            link.detach_device();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        io::{Read, Write},
        net::TcpStream,
        rc::Rc,
        time::Duration,
    };

    use unicorn_engine::{
        unicorn_const::{Arch, Mode},
        Unicorn,
    };

    use super::{EcuIo, EcuIoAdcChannelConfig, EcuIoConfig};
    use crate::{
        ext_devices::ExtDevices,
        peripherals::{
            gpio::{GpioPorts, Pin},
            Peripherals,
        },
        system::System,
    };

    fn test_config() -> EcuIoConfig {
        EcuIoConfig {
            listen: "127.0.0.1:0".to_string(),
            pins: vec![],
            adc_channels: vec![EcuIoAdcChannelConfig {
                name: "map".to_string(),
                pin: "PC0".to_string(),
            }],
            trigger_wheel: None,
        }
    }

    #[test]
    fn a_line_sent_by_the_client_updates_the_named_value() {
        let mut ecu_io = EcuIo::new(test_config()).unwrap();
        let mut client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
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
    fn an_unknown_signal_name_is_logged_and_not_stored() {
        let mut ecu_io = EcuIo::new(test_config()).unwrap();
        let mut client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
        ecu_io.poll().unwrap();

        client.write_all(b"av12=1500\n").unwrap();
        for _ in 0..10 {
            ecu_io.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(!ecu_io.values.contains_key("av12"));
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
        ecu_io.report_output("inj1", false);
        ecu_io.poll().unwrap();

        let mut client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        ecu_io.poll().unwrap(); // accept

        // Different level than the pre-connection call, so it's a real change and gets queued.
        ecu_io.report_output("inj1", true);
        ecu_io.poll().unwrap();

        let mut buf = [0; 16];
        let n = client.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"inj1=1\n");
    }

    #[test]
    fn report_output_only_queues_a_line_when_the_level_actually_changes() {
        let mut ecu_io = EcuIo::new(test_config()).unwrap();
        let _client = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
        ecu_io.poll().unwrap(); // accept

        ecu_io.report_output("inj1", true);
        assert_eq!(
            ecu_io.outgoing.iter().copied().collect::<Vec<u8>>(),
            b"inj1=1\n".to_vec()
        );

        ecu_io.outgoing.clear();
        ecu_io.report_output("inj1", true); // same level again: must not requeue
        assert!(
            ecu_io.outgoing.is_empty(),
            "same level must not queue a second line"
        );

        ecu_io.report_output("inj1", false); // different level: must queue
        assert_eq!(
            ecu_io.outgoing.iter().copied().collect::<Vec<u8>>(),
            b"inj1=0\n".to_vec()
        );
    }

    #[test]
    fn a_second_client_is_rejected_while_one_is_already_connected() {
        let mut ecu_io = EcuIo::new(test_config()).unwrap();
        let mut client1 = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
        client1
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        ecu_io.poll().unwrap(); // accept client1

        let mut client2 = TcpStream::connect(ecu_io.local_addr().unwrap()).unwrap();
        client2
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        for _ in 0..10 {
            ecu_io.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        // The second client should have been accepted-then-dropped by `accept_clients`,
        // never served: it observes either a clean EOF or a reset, but no data.
        let mut buf = [0; 16];
        match client2.read(&mut buf) {
            Ok(0) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionReset
                    || error.kind() == std::io::ErrorKind::ConnectionAborted => {}
            other => {
                panic!("expected the rejected second client to be disconnected, got {other:?}")
            }
        }

        // The first client must be unaffected by the rejected second connection attempt.
        ecu_io.report_output("inj1", true);
        ecu_io.poll().unwrap();
        let n = client1.read(&mut buf).unwrap();
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

    fn wheel_60_2() -> super::TriggerWheel {
        super::TriggerWheel::new(super::EcuIoTriggerWheelConfig {
            signal: "trigger_rpm".to_string(),
            pin_signal: "din1".to_string(),
            teeth: 60,
            missing: 2,
        })
    }

    #[test]
    fn trigger_wheel_level_is_high_in_the_first_half_of_a_tooth_slot() {
        let wheel = wheel_60_2();
        let slot = 1.0 / 60.0;
        assert!(
            wheel.level_at(0.25 * slot),
            "first half of slot 0 carries the tooth"
        );
        assert!(!wheel.level_at(0.75 * slot), "second half of slot 0 is low");
        assert!(
            wheel.level_at(10.0 * slot + 0.25 * slot),
            "mid-wheel tooth present"
        );
    }

    #[test]
    fn trigger_wheel_missing_teeth_slots_stay_low() {
        let wheel = wheel_60_2();
        let slot = 1.0 / 60.0;
        assert!(
            wheel.level_at(57.0 * slot + 0.25 * slot),
            "slot 57 is the last real tooth"
        );
        assert!(
            !wheel.level_at(58.0 * slot + 0.25 * slot),
            "slot 58 is in the 60-2 gap"
        );
        assert!(
            !wheel.level_at(59.0 * slot + 0.25 * slot),
            "slot 59 is in the 60-2 gap"
        );
    }

    #[test]
    fn trigger_wheel_advances_one_revolution_per_expected_instruction_count() {
        let mut wheel = wheel_60_2();
        // 1200 rpm = 20 rev/s; one rev = 216e6/20 = 10.8M instructions.
        wheel.advance(0, 1200);
        wheel.advance(10_800_000, 1200);
        assert!(
            wheel.position < 1e-9 || wheel.position > 1.0 - 1e-9,
            "full revolution wraps to ~0, got {}",
            wheel.position
        );

        // Half a revolution later, position is 0.5 -- continuity across an RPM change.
        wheel.advance(10_800_000 + 2_700_000, 2400);
        assert!(
            (wheel.position - 0.5).abs() < 1e-9,
            "got {}",
            wheel.position
        );
    }

    #[test]
    fn trigger_wheel_projects_the_angle_between_advances_without_moving() {
        let mut wheel = wheel_60_2();
        // 1200 rpm = 20 rev/s; a quarter revolution = 216e6/20/4 = 2.7M instructions.
        wheel.advance(0, 1200);
        assert_eq!(wheel.project_angle_mdeg(2_700_000, 1200), 90_000);
        assert_eq!(
            wheel.last_instructions, 0,
            "projection must not advance the wheel"
        );
        // Stopped wheel projects its held position.
        assert_eq!(wheel.project_angle_mdeg(2_700_000, 0), 0);
    }

    #[test]
    fn trigger_wheel_holds_level_and_position_while_stopped() {
        let mut wheel = wheel_60_2();
        wheel.advance(0, 1200);
        wheel.advance(1_000_000, 1200);
        let position = wheel.position;
        assert_eq!(
            wheel.advance(50_000_000, 0),
            None,
            "stopped wheel produces no level"
        );
        assert_eq!(wheel.position, position, "stopped wheel does not move");
        // Restarting does not jump: time spent stopped is not integrated.
        wheel.advance(50_001_024, 1200);
        assert!(
            (wheel.position - position) < 0.001,
            "no position jump after restart"
        );
    }

    #[test]
    fn trigger_rpm_drives_the_pin_signal_through_the_values_map() {
        let mut config = test_config();
        config.pins = vec![super::EcuIoPinConfig {
            name: "din1".to_string(),
            pin: "PC6".to_string(),
            direction: super::EcuIoPinDirection::Input,
        }];
        config.trigger_wheel = Some(super::EcuIoTriggerWheelConfig {
            signal: "trigger_rpm".to_string(),
            pin_signal: "din1".to_string(),
            teeth: 60,
            missing: 2,
        });
        let mut ecu_io = EcuIo::new(config).unwrap();

        // trigger_rpm is a known name (not rejected), and the wheel drives din1:
        // at 1200 rpm a slot is 180k instructions, so 45k in (a quarter slot)
        // the level is high and 135k in (three quarters) it is low.
        ecu_io.set_value("trigger_rpm", 1200);
        assert!(
            ecu_io.values.contains_key("trigger_rpm"),
            "trigger signal must be accepted"
        );

        ecu_io.advance_trigger_wheel(45_000);
        assert_eq!(
            ecu_io.values.get("din1"),
            Some(&1),
            "first half of slot 0 is high"
        );
        assert!(ecu_io.digital_level("din1"));

        ecu_io.advance_trigger_wheel(135_000);
        assert_eq!(
            ecu_io.values.get("din1"),
            Some(&0),
            "second half of slot 0 is low"
        );
    }

    #[test]
    fn trigger_wheel_configuration_deserializes() {
        let config: EcuIoConfig = serde_yaml::from_str(
            "listen: 127.0.0.1:29002\npins:\n  - name: din1\n    pin: PC6\n    direction: input\ntrigger_wheel:\n  signal: trigger_rpm\n  pin_signal: din1\n  teeth: 60\n  missing: 2\n",
        )
        .unwrap();

        let wheel = config
            .trigger_wheel
            .expect("trigger_wheel section must parse");
        assert_eq!(wheel.signal, "trigger_rpm");
        assert_eq!(wheel.pin_signal, "din1");
        assert_eq!(wheel.teeth, 60);
        assert_eq!(wheel.missing, 2);
    }

    #[test]
    fn push_capped_vec_drops_oldest_bytes_once_over_capacity() {
        let mut buffer = vec![1u8, 2, 3];
        EcuIo::push_capped_vec(&mut buffer, &[4, 5], 3);
        assert_eq!(buffer, vec![3, 4, 5]);
    }

    #[test]
    fn push_capped_deque_drops_oldest_bytes_once_over_capacity() {
        let mut queue: std::collections::VecDeque<u8> = vec![1u8, 2, 3].into();
        EcuIo::push_capped_deque(&mut queue, &[4, 5], 3);
        assert_eq!(queue.into_iter().collect::<Vec<u8>>(), vec![3, 4, 5]);
    }

    // `System` borrows its Unicorn engine mutably (`uc: RefCell<&'a mut Unicorn<'b, ()>>`),
    // so this can't return a ready-made `System` — the engine would be a dangling
    // local. Each test constructs its own `Unicorn`/`Peripherals`/`ExtDevices` and
    // builds `System` from them locally instead.
    fn test_parts() -> (Unicorn<'static, ()>, Rc<Peripherals>, Rc<ExtDevices>) {
        let uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        (
            uc,
            Rc::new(Peripherals::default()),
            Rc::new(ExtDevices::default()),
        )
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
            trigger_wheel: None,
        };
        let ecu_io = EcuIo::register(config, &mut gpio).unwrap();
        let (mut uc, p, d) = test_parts();
        let sys = System {
            uc: RefCell::new(&mut uc),
            p,
            d,
        };

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
            trigger_wheel: None,
        };
        let ecu_io = EcuIo::register(config, &mut gpio).unwrap();
        let (mut uc, p, d) = test_parts();
        let sys = System {
            uc: RefCell::new(&mut uc),
            p,
            d,
        };

        let mut client = TcpStream::connect(ecu_io.borrow().local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        ecu_io.borrow_mut().poll().unwrap();

        let port = GpioPorts::port_index('D');
        gpio.write_port(&sys, port, 7, true);
        ecu_io.borrow_mut().poll().unwrap();

        let mut buf = [0; 16];
        let n = client.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"inj1=1\n");
    }

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
            trigger_wheel: None,
        };
        let ecu_io = EcuIo::register(config, &mut gpio).unwrap();

        let (mut uc, p, d) = test_parts();
        p.exti
            .borrow_mut()
            .write_syscfg(crate::peripherals::exti::Exti::EXTICR2, 2 << 8);
        p.exti
            .borrow_mut()
            .write_exti(crate::peripherals::exti::Exti::IMR, 1 << 6);
        p.exti
            .borrow_mut()
            .write_exti(crate::peripherals::exti::Exti::RTSR, 1 << 6);
        let sys = System {
            uc: RefCell::new(&mut uc),
            p: p.clone(),
            d,
        };

        ecu_io.borrow_mut().set_value("crank", 1);
        ecu_io.borrow_mut().check_digital_edges(&sys);

        assert_eq!(
            p.exti
                .borrow_mut()
                .read_exti(crate::peripherals::exti::Exti::PR)
                & (1 << 6),
            1 << 6
        );
    }
}
