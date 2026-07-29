// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_PENDING_INPUTS: usize = 1024;
const MAX_PENDING_OUTPUT_EVENTS: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedOutputEvent {
    pub sequence: u64,
    pub instruction_count: u64,
    pub name: String,
    pub previous_value: Option<i32>,
    pub value: i32,
}

#[derive(Clone, Debug)]
pub struct EmbeddedEcuIoLink {
    inner: Arc<Mutex<EmbeddedEcuIoState>>,
}

#[derive(Debug, Default)]
struct EmbeddedEcuIoState {
    device_present: bool,
    pending_inputs: VecDeque<(String, i32)>,
    outputs: HashMap<String, i32>,
    output_events: VecDeque<EmbeddedOutputEvent>,
    next_output_sequence: u64,
}

impl EmbeddedEcuIoLink {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EmbeddedEcuIoState::default())),
        }
    }

    pub fn set_input(&self, name: impl Into<String>, value: i32) {
        let mut state = self.inner.lock().expect("embedded ECU I/O mutex poisoned");
        if state.pending_inputs.len() == MAX_PENDING_INPUTS {
            state.pending_inputs.pop_front();
        }
        state.pending_inputs.push_back((name.into(), value));
    }

    pub fn outputs(&self) -> HashMap<String, i32> {
        self.inner
            .lock()
            .expect("embedded ECU I/O mutex poisoned")
            .outputs
            .clone()
    }

    pub fn take_output_events(&self) -> Vec<EmbeddedOutputEvent> {
        self.inner
            .lock()
            .expect("embedded ECU I/O mutex poisoned")
            .output_events
            .drain(..)
            .collect()
    }

    pub fn device_present(&self) -> bool {
        self.inner
            .lock()
            .expect("embedded ECU I/O mutex poisoned")
            .device_present
    }

    pub(crate) fn attach_device(&self) {
        self.inner
            .lock()
            .expect("embedded ECU I/O mutex poisoned")
            .device_present = true;
    }

    pub(crate) fn detach_device(&self) {
        let mut state = self.inner.lock().expect("embedded ECU I/O mutex poisoned");
        state.device_present = false;
        state.pending_inputs.clear();
        state.outputs.clear();
        state.output_events.clear();
    }

    pub(crate) fn take_inputs(&self) -> Vec<(String, i32)> {
        self.inner
            .lock()
            .expect("embedded ECU I/O mutex poisoned")
            .pending_inputs
            .drain(..)
            .collect()
    }

    pub(crate) fn publish_output(&self, name: &str, value: i32) {
        let mut state = self.inner.lock().expect("embedded ECU I/O mutex poisoned");
        let previous_value = state.outputs.insert(name.to_string(), value);
        if previous_value == Some(value) {
            return;
        }
        if state.output_events.len() == MAX_PENDING_OUTPUT_EVENTS {
            state.output_events.pop_front();
        }
        let sequence = state.next_output_sequence;
        state.next_output_sequence = state.next_output_sequence.wrapping_add(1);
        state.output_events.push_back(EmbeddedOutputEvent {
            sequence,
            instruction_count: crate::emulator::instruction_count(),
            name: name.to_string(),
            previous_value,
            value,
        });
    }

    fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Default for EmbeddedEcuIoLink {
    fn default() -> Self {
        Self::new()
    }
}

static PENDING_DEVICE_LINK: OnceLock<Mutex<Option<EmbeddedEcuIoLink>>> = OnceLock::new();
static ACTIVE_HOST_LINK: OnceLock<Mutex<Option<EmbeddedEcuIoLink>>> = OnceLock::new();

pub(crate) fn install_embedded_link(link: EmbeddedEcuIoLink) {
    *PENDING_DEVICE_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded ECU I/O registry poisoned") = Some(link.clone());
    *ACTIVE_HOST_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded ECU I/O registry poisoned") = Some(link);
}

pub(crate) fn take_pending_device_link() -> Option<EmbeddedEcuIoLink> {
    PENDING_DEVICE_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded ECU I/O registry poisoned")
        .take()
}

pub(crate) fn clear_embedded_link(link: &EmbeddedEcuIoLink) {
    let mut active = ACTIVE_HOST_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded ECU I/O registry poisoned");
    if active.as_ref().is_some_and(|current| current.ptr_eq(link)) {
        active.take();
    }
    let mut pending = PENDING_DEVICE_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded ECU I/O registry poisoned");
    if pending.as_ref().is_some_and(|current| current.ptr_eq(link)) {
        pending.take();
    }
}

pub fn active_embedded_link() -> Option<EmbeddedEcuIoLink> {
    ACTIVE_HOST_LINK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("embedded ECU I/O registry poisoned")
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchanges_named_inputs_and_outputs() {
        let link = EmbeddedEcuIoLink::new();
        link.attach_device();
        link.set_input("trigger_rpm", 1250);
        assert_eq!(link.take_inputs(), vec![("trigger_rpm".to_string(), 1250)]);
        link.publish_output("ign1", 1);
        assert_eq!(link.outputs().get("ign1"), Some(&1));
        link.publish_output("ign1", 0);
        let events = link.take_output_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, "ign1");
        assert_eq!(events[0].previous_value, None);
        assert_eq!(events[0].value, 1);
        assert_eq!(events[1].previous_value, Some(1));
        assert_eq!(events[1].value, 0);
    }
}
