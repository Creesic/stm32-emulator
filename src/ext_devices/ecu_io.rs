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
