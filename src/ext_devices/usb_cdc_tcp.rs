// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::VecDeque,
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::{Duration, Instant},
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
    // When the current client connected -- a brand new connection only
    // replaces it once this has aged past stale_client_grace_period (see
    // accept_clients()).
    client_connected_at: Option<Instant>,
    stale_client_grace_period: Duration,
    to_device: VecDeque<u8>,
    from_device: VecDeque<u8>,
    // --- temporary diagnostic instrumentation (remove after diagnosis) ---
    rx_total: usize,
    tx_total: usize,
    last_heartbeat_at: Option<Instant>,
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
            client_connected_at: None,
            stale_client_grace_period: Self::STALE_CLIENT_GRACE_PERIOD,
            to_device: VecDeque::new(),
            from_device: VecDeque::new(),
            rx_total: 0,
            tx_total: 0,
            last_heartbeat_at: None,
        })
    }

    #[cfg(test)]
    pub fn set_stale_client_grace_period_for_test(&mut self, duration: Duration) {
        self.stale_client_grace_period = duration;
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
        self.log_heartbeat();
        Ok(())
    }

    // --- temporary diagnostic instrumentation (remove after diagnosis) ---
    // Periodic proof-of-life for whichever client is currently held. If a
    // peer (e.g. TunerStudio) abandons its connection without a clean close,
    // this keeps firing "no disconnect signal seen" for the whole time the
    // dead socket wedges the bridge -- which is exactly the evidence we need.
    fn log_heartbeat(&mut self) {
        if self.client.is_none() {
            return;
        }
        let due = self
            .last_heartbeat_at
            .map_or(true, |t| t.elapsed() >= Duration::from_secs(2));
        if due {
            self.last_heartbeat_at = Some(Instant::now());
            info!(
                "[usb-instr] client alive: age {:?}, rx {}, tx {}, {} bytes queued to client, {} queued to device -- no disconnect signal seen",
                self.client_connected_at.map(|t| t.elapsed()),
                self.rx_total,
                self.tx_total,
                self.from_device.len(),
                self.to_device.len(),
            );
        }
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

    // A client that goes silent without a clean TCP close (observed live: a
    // reset that never resolves to a readable error on this end) would
    // otherwise wedge this bridge forever, rejecting every later connection
    // attempt with no recovery short of restarting the whole emulator. But
    // replacing it the instant ANY new connection shows up caused a
    // different, live-observed failure: a client that retries quickly (e.g.
    // TunerStudio re-attempting every few seconds) can repeatedly preempt
    // its OWN still-in-flight previous attempt before firmware ever
    // finishes responding to it, so no single connection ever completes a
    // round trip. Instead of connect-count based rejection, require the
    // current client to have been idle for a grace period before treating
    // it as stale enough to replace.
    const STALE_CLIENT_GRACE_PERIOD: Duration = Duration::from_secs(10);

    fn accept_clients(&mut self) -> Result<()> {
        loop {
            match self.listener.accept() {
                Ok((client, address)) => {
                    if self.client.is_some() {
                        let existing_age = self.client_connected_at.map(|t| t.elapsed());
                        let still_within_grace_period = self
                            .client_connected_at
                            .is_some_and(|connected_at| connected_at.elapsed() < self.stale_client_grace_period);
                        if still_within_grace_period {
                            info!("[usb-instr] second connection from {address} while holding a client (age {existing_age:?}, rx {} tx {}); REJECTING new one (grace period)", self.rx_total, self.tx_total);
                            continue;
                        }
                        info!("[usb-instr] second connection from {address} while holding a client (age {existing_age:?}, rx {} tx {}); REPLACING the old one", self.rx_total, self.tx_total);
                        self.mark_disconnected();
                    }
                    client
                        .set_nonblocking(true)
                        .context("Failed to make USB CDC client nonblocking")?;
                    info!("USB CDC TCP client connected from {address}");
                    self.client = Some(client);
                    self.client_connected_at = Some(Instant::now());
                    self.rx_total = 0;
                    self.tx_total = 0;
                    self.last_heartbeat_at = Some(Instant::now());
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error).context("Failed to accept USB CDC TCP client"),
            }
        }
        Ok(())
    }

    fn receive_from_client(&mut self) -> Result<()> {
        let connected_at = self.client_connected_at;
        let mut disconnected = false;
        let mut disconnect_reason = "";
        if let Some(client) = self.client.as_mut() {
            let mut buffer = [0; 1024];
            loop {
                match client.read(&mut buffer) {
                    Ok(0) => {
                        disconnected = true;
                        disconnect_reason = "peer sent FIN (read returned Ok(0))";
                        break;
                    }
                    Ok(count) => {
                        self.rx_total += count;
                        Self::push_capped(
                            &mut self.to_device,
                            &buffer[..count],
                            self.config.max_buffered_bytes,
                        );
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                        disconnected = true;
                        disconnect_reason = "peer sent RST (ConnectionReset)";
                        break;
                    }
                    Err(error) => return Err(error).context("Failed to read USB CDC TCP client"),
                }
            }
        }
        if disconnected {
            info!(
                "[usb-instr] recv disconnect: {disconnect_reason}; client age {:?}, total rx {} tx {}",
                connected_at.map(|t| t.elapsed()),
                self.rx_total,
                self.tx_total,
            );
            self.mark_disconnected();
        }
        Ok(())
    }

    fn send_to_client(&mut self) -> Result<()> {
        let connected_at = self.client_connected_at;
        let mut disconnected = false;
        let mut disconnect_reason = "";
        if let Some(client) = self.client.as_mut() {
            while !self.from_device.is_empty() {
                let (first, second) = self.from_device.as_slices();
                let bytes = if first.is_empty() { second } else { first };
                match client.write(bytes) {
                    Ok(0) => {
                        disconnected = true;
                        disconnect_reason = "write returned Ok(0)";
                        break;
                    }
                    Ok(count) => {
                        self.tx_total += count;
                        self.from_device.drain(..count);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::ConnectionReset => {
                        disconnected = true;
                        disconnect_reason = "peer sent RST (ConnectionReset)";
                        break;
                    }
                    Err(error) => return Err(error).context("Failed to write USB CDC TCP client"),
                }
            }
        }
        if disconnected {
            info!(
                "[usb-instr] send disconnect: {disconnect_reason}; client age {:?}, total rx {} tx {}",
                connected_at.map(|t| t.elapsed()),
                self.rx_total,
                self.tx_total,
            );
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
        self.client_connected_at = None;
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

    #[test]
    fn a_new_connection_is_rejected_while_the_current_client_is_within_its_grace_period() {
        // Live-observed regression: a client that retries quickly (e.g.
        // TunerStudio re-attempting every few seconds while the emulator is
        // busy enough that responses take several real seconds) could
        // repeatedly preempt its OWN still-in-flight previous connection
        // before firmware ever finished responding to it -- no single
        // connection ever completed a round trip. Replacing on every new
        // connection, with no minimum age check, caused this livelock.
        let mut bridge = UsbCdcTcp::new(test_config()).unwrap();
        bridge.set_stale_client_grace_period_for_test(Duration::from_secs(60));
        let addr = bridge.local_addr().unwrap();

        let _first_client = TcpStream::connect(addr).unwrap();
        bridge.poll().unwrap();
        assert!(bridge.is_client_connected());

        let mut second_client = TcpStream::connect(addr).unwrap();
        for _ in 0..5 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        // The first client is still the active one -- pushing a response
        // now must reach it, not the rejected second connection attempt.
        bridge.push_from_device(b"hello");
        for _ in 0..5 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut buf = [0u8; 16];
        second_client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        match second_client.read(&mut buf) {
            Ok(count) => assert_eq!(count, 0, "the rejected connection must not receive data"),
            Err(error) => assert!(
                error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut,
                "unexpected error waiting for (no) data: {error:?}"
            ),
        }
    }

    #[test]
    fn a_new_connection_replaces_a_stale_one_once_its_grace_period_has_elapsed() {
        // A client can go silent without ever completing a clean TCP
        // close (observed live: a connection reset that never resolves
        // to a readable error on this end, leaving it looking perfectly
        // healthy). Rejecting every later connection attempt forever in
        // that case wedges the bridge until the whole emulator is
        // restarted -- only one physical USB host is ever attached at a
        // time anyway, so a new connection should eventually replace a
        // truly stale old one, not be refused indefinitely.
        let mut bridge = UsbCdcTcp::new(test_config()).unwrap();
        bridge.set_stale_client_grace_period_for_test(Duration::from_millis(20));
        let addr = bridge.local_addr().unwrap();

        let _stale_client = TcpStream::connect(addr).unwrap();
        bridge.poll().unwrap();
        assert!(bridge.is_client_connected());

        // _stale_client is deliberately never dropped or disconnected --
        // it just goes quiet, exactly like the wedged state this fix
        // targets. Let its grace period fully elapse before the new
        // connection arrives.
        std::thread::sleep(Duration::from_millis(30));
        let mut new_client = TcpStream::connect(addr).unwrap();
        new_client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();

        for _ in 0..5 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(bridge.is_client_connected());

        bridge.push_from_device(b"hello");
        for _ in 0..5 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut buf = [0u8; 16];
        let count = new_client
            .read(&mut buf)
            .expect("the new client must receive data bridged after replacing the stale one");
        assert_eq!(&buf[..count], b"hello");
    }
}
