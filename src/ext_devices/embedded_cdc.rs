// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_MAX_BUFFERED_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddedCdcStage {
    Detached,
    DevicePresent,
    UsbStarted,
    AwaitingDeviceDescriptor,
    AwaitingSetAddressStatus,
    AwaitingSetConfigurationStatus,
    AwaitingSetLineCodingStatus,
    AwaitingSetControlLineStateStatus,
    ProtocolReady,
}

#[derive(Clone, Debug)]
pub struct EmbeddedCdcLink {
    inner: Arc<EmbeddedCdcInner>,
}

#[derive(Debug)]
struct EmbeddedCdcInner {
    state: Mutex<EmbeddedCdcState>,
    readable: Condvar,
}

#[derive(Debug)]
struct EmbeddedCdcState {
    host_to_device: VecDeque<u8>,
    device_to_host: VecDeque<u8>,
    host_connected: bool,
    stage: EmbeddedCdcStage,
    max_buffered_bytes: usize,
}

impl EmbeddedCdcLink {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EmbeddedCdcInner {
                state: Mutex::new(EmbeddedCdcState {
                    host_to_device: VecDeque::new(),
                    device_to_host: VecDeque::new(),
                    host_connected: false,
                    stage: EmbeddedCdcStage::Detached,
                    max_buffered_bytes: DEFAULT_MAX_BUFFERED_BYTES,
                }),
                readable: Condvar::new(),
            }),
        }
    }

    pub fn connect(&self) -> Result<(), &'static str> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        if state.stage != EmbeddedCdcStage::ProtocolReady {
            return Err("embedded firmware TunerStudio protocol is not ready");
        }
        state.host_connected = true;
        state.host_to_device.clear();
        state.device_to_host.clear();
        Ok(())
    }

    pub fn disconnect(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        state.host_connected = false;
        state.host_to_device.clear();
        state.device_to_host.clear();
        self.inner.readable.notify_all();
    }

    pub fn is_connected(&self) -> bool {
        let state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        state.host_connected && state.stage == EmbeddedCdcStage::ProtocolReady
    }

    pub fn device_present(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned")
            .stage
            != EmbeddedCdcStage::Detached
    }

    pub fn protocol_ready(&self) -> bool {
        self.protocol_stage() == EmbeddedCdcStage::ProtocolReady
    }

    pub fn protocol_stage(&self) -> EmbeddedCdcStage {
        self.inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned")
            .stage
    }

    pub fn write_from_host(&self, bytes: &[u8]) -> Result<usize, &'static str> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        if !state.host_connected || state.stage != EmbeddedCdcStage::ProtocolReady {
            return Err("embedded CDC link is not connected");
        }
        let capacity = state.max_buffered_bytes;
        push_capped(&mut state.host_to_device, bytes, capacity);
        Ok(bytes.len())
    }

    pub fn read_for_host(&self, target: &mut [u8], timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        while state.device_to_host.is_empty()
            && state.host_connected
            && state.stage == EmbeddedCdcStage::ProtocolReady
        {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (new_state, wait) = self
                .inner
                .readable
                .wait_timeout(state, deadline - now)
                .expect("embedded CDC mutex poisoned");
            state = new_state;
            if wait.timed_out() {
                break;
            }
        }
        let count = target.len().min(state.device_to_host.len());
        for slot in &mut target[..count] {
            *slot = state
                .device_to_host
                .pop_front()
                .expect("count was bounded by the queue length");
        }
        count
    }

    pub fn bytes_available(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned")
            .device_to_host
            .len()
    }

    pub(crate) fn attach_device(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        state.stage = EmbeddedCdcStage::DevicePresent;
        self.inner.readable.notify_all();
    }

    pub(crate) fn set_protocol_stage(&self, stage: EmbeddedCdcStage) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        state.stage = if state.stage == EmbeddedCdcStage::Detached {
            EmbeddedCdcStage::Detached
        } else {
            stage
        };
        if state.stage != EmbeddedCdcStage::ProtocolReady {
            state.host_connected = false;
            state.host_to_device.clear();
            state.device_to_host.clear();
        }
        self.inner.readable.notify_all();
    }

    pub(crate) fn detach_device(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        state.stage = EmbeddedCdcStage::Detached;
        state.host_connected = false;
        state.host_to_device.clear();
        state.device_to_host.clear();
        self.inner.readable.notify_all();
    }

    pub(crate) fn write_from_device(&self, bytes: &[u8]) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        if !state.host_connected {
            return;
        }
        let capacity = state.max_buffered_bytes;
        push_capped(&mut state.device_to_host, bytes, capacity);
        self.inner.readable.notify_all();
    }

    pub(crate) fn read_for_device(&self, maximum: usize) -> Vec<u8> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("embedded CDC mutex poisoned");
        let count = maximum.min(state.host_to_device.len());
        state.host_to_device.drain(..count).collect()
    }

    fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Default for EmbeddedCdcLink {
    fn default() -> Self {
        Self::new()
    }
}

fn push_capped(queue: &mut VecDeque<u8>, bytes: &[u8], capacity: usize) {
    let overflow = queue
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(capacity);
    queue.drain(..overflow.min(queue.len()));
    if bytes.len() >= capacity {
        queue.extend(bytes[bytes.len() - capacity..].iter().copied());
    } else {
        queue.extend(bytes.iter().copied());
    }
}

static PENDING_DEVICE_LINK: OnceLock<Mutex<Option<EmbeddedCdcLink>>> = OnceLock::new();
static ACTIVE_HOST_LINK: OnceLock<Mutex<Option<EmbeddedCdcLink>>> = OnceLock::new();

pub(crate) fn install_embedded_link(link: EmbeddedCdcLink) {
    *PENDING_DEVICE_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded CDC registry poisoned") = Some(link.clone());
    *ACTIVE_HOST_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded CDC registry poisoned") = Some(link);
}

pub(crate) fn take_pending_device_link() -> Option<EmbeddedCdcLink> {
    PENDING_DEVICE_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded CDC registry poisoned")
        .take()
}

pub(crate) fn clear_embedded_link(link: &EmbeddedCdcLink) {
    let mut active = ACTIVE_HOST_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded CDC registry poisoned");
    if active.as_ref().is_some_and(|current| current.ptr_eq(link)) {
        active.take();
    }
    let mut pending = PENDING_DEVICE_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded CDC registry poisoned");
    if pending.as_ref().is_some_and(|current| current.ptr_eq(link)) {
        pending.take();
    }
}

pub fn active_embedded_link() -> Option<EmbeddedCdcLink> {
    ACTIVE_HOST_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded CDC registry poisoned")
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_device_exchange_binary_bytes_without_a_socket() {
        let link = EmbeddedCdcLink::new();
        link.attach_device();
        link.set_protocol_stage(EmbeddedCdcStage::ProtocolReady);
        link.connect().unwrap();

        assert_eq!(link.write_from_host(&[0x00, 0xff, 0x42]), Ok(3));
        assert_eq!(link.read_for_device(64), vec![0x00, 0xff, 0x42]);

        link.write_from_device(&[0x10, 0x20]);
        let mut received = [0; 4];
        let count = link.read_for_host(&mut received, Duration::ZERO);
        assert_eq!(&received[..count], &[0x10, 0x20]);
    }

    #[test]
    fn host_cannot_connect_until_the_firmware_protocol_is_ready() {
        let link = EmbeddedCdcLink::new();
        assert!(!link.device_present());
        assert_eq!(
            link.connect(),
            Err("embedded firmware TunerStudio protocol is not ready")
        );
        link.attach_device();
        assert!(link.device_present());
        assert!(!link.protocol_ready());
        assert_eq!(
            link.connect(),
            Err("embedded firmware TunerStudio protocol is not ready")
        );
        link.set_protocol_stage(EmbeddedCdcStage::ProtocolReady);
        assert!(link.protocol_ready());
        assert_eq!(link.connect(), Ok(()));
        link.detach_device();
        assert!(!link.device_present());
        assert!(!link.protocol_ready());
        assert!(!link.is_connected());
    }
}
