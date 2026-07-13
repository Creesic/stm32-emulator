// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::VecDeque,
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
};

use anyhow::{bail, Context as _, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct UsbCdcTcpConfig {
    pub peripheral: String,
    pub listen: String,
    pub max_buffered_bytes: usize,
}

pub struct UsbCdcTcp {
    pub config: UsbCdcTcpConfig,
    listener: TcpListener,
    client: Option<TcpStream>,
    to_device: VecDeque<u8>,
    from_device: VecDeque<u8>,
}

impl UsbCdcTcp {
    pub fn new(config: UsbCdcTcpConfig) -> Result<Self> {
        if config.max_buffered_bytes == 0 {
            bail!("usb_cdc_tcp max_buffered_bytes must be positive");
        }

        let listener = TcpListener::bind(&config.listen)
            .with_context(|| format!("Failed to listen for USB CDC TCP at {}", config.listen))?;
        listener
            .set_nonblocking(true)
            .context("Failed to make USB CDC listener nonblocking")?;

        Ok(Self {
            config,
            listener,
            client: None,
            to_device: VecDeque::new(),
            from_device: VecDeque::new(),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.listener
            .local_addr()
            .context("Failed to read USB CDC listener address")
    }

    pub fn poll(&mut self) -> Result<()> {
        self.accept_clients()?;
        self.receive_from_client()?;
        self.send_to_client()?;
        Ok(())
    }

    pub fn push_from_device(&mut self, bytes: &[u8]) {
        Self::push_capped(
            &mut self.from_device,
            bytes,
            self.config.max_buffered_bytes,
        );
    }

    pub fn take_for_device(&mut self, maximum: usize) -> Vec<u8> {
        self.to_device.drain(..maximum.min(self.to_device.len())).collect()
    }

    pub fn is_client_connected(&self) -> bool {
        self.client.is_some()
    }

    fn accept_clients(&mut self) -> Result<()> {
        loop {
            match self.listener.accept() {
                Ok((client, address)) => {
                    if self.client.is_some() {
                        debug!("Rejecting additional USB CDC TCP client at {address}");
                        continue;
                    }
                    client
                        .set_nonblocking(true)
                        .context("Failed to make USB CDC client nonblocking")?;
                    info!("USB CDC TCP client connected from {address}");
                    self.client = Some(client);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error).context("Failed to accept USB CDC TCP client"),
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
                    Ok(count) => Self::push_capped(
                        &mut self.to_device,
                        &buffer[..count],
                        self.config.max_buffered_bytes,
                    ),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                        disconnected = true;
                        break;
                    }
                    Err(error) => return Err(error).context("Failed to read USB CDC TCP client"),
                }
            }
        }
        if disconnected {
            self.mark_disconnected();
        }
        Ok(())
    }

    fn send_to_client(&mut self) -> Result<()> {
        let mut disconnected = false;
        if let Some(client) = self.client.as_mut() {
            while !self.from_device.is_empty() {
                let (first, second) = self.from_device.as_slices();
                let bytes = if first.is_empty() { second } else { first };
                match client.write(bytes) {
                    Ok(0) => {
                        disconnected = true;
                        break;
                    }
                    Ok(count) => {
                        self.from_device.drain(..count);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                        disconnected = true;
                        break;
                    }
                    Err(error) => return Err(error).context("Failed to write USB CDC TCP client"),
                }
            }
        }
        if disconnected {
            self.mark_disconnected();
        }
        Ok(())
    }

    // A disconnected client's leftover queued bytes belong to a session
    // that no longer exists -- left uncleared, they get delivered to
    // whichever client connects next (observed live: a large stale
    // response queued for an abandoned TunerStudio session bled into an
    // unrelated later request's reply, corrupting it).
    fn mark_disconnected(&mut self) {
        info!("USB CDC TCP client disconnected");
        self.client = None;
        self.to_device.clear();
        self.from_device.clear();
    }

    fn push_capped(queue: &mut VecDeque<u8>, bytes: &[u8], capacity: usize) {
        for &byte in bytes {
            if queue.len() == capacity {
                queue.pop_front();
            }
            queue.push_back(byte);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, net::TcpStream, time::Duration};

    use super::{UsbCdcTcp, UsbCdcTcpConfig};

    fn test_config() -> UsbCdcTcpConfig {
        UsbCdcTcpConfig {
            peripheral: "OTG_FS_GLOBAL".to_owned(),
            listen: "127.0.0.1:0".to_owned(),
            max_buffered_bytes: 65536,
        }
    }

    #[test]
    fn stale_unsent_bytes_do_not_reach_the_next_client_after_a_disconnect() {
        let mut bridge = UsbCdcTcp::new(test_config()).unwrap();
        let addr = bridge.local_addr().unwrap();

        let client = TcpStream::connect(addr).unwrap();
        bridge.poll().unwrap();

        // Queue a response but let the client disconnect before ever
        // reading it -- these bytes must not survive to the next session.
        bridge.push_from_device(b"stale response bytes");
        drop(client);

        for _ in 0..20 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!bridge.is_client_connected());

        let mut new_client = TcpStream::connect(addr).unwrap();
        new_client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        for _ in 0..5 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut buf = [0u8; 64];
        match new_client.read(&mut buf) {
            Ok(count) => assert_eq!(
                count, 0,
                "a new client must not receive stale bytes queued for the old session"
            ),
            Err(error) => assert!(
                error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut,
                "unexpected error waiting for (no) data: {error:?}"
            ),
        }
    }
}
