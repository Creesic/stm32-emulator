// SPDX-License-Identifier: GPL-3.0-or-later

use std::{cell::RefCell, collections::{BTreeMap, VecDeque}, rc::Rc};

use crate::{ext_devices::{usb_cdc_tcp::UsbCdcTcp, ExtDevices}, system::System};

use super::Peripheral;

#[derive(Default, Clone, Copy)]
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
    rx_fifo: VecDeque<u8>,
    rx_status: VecDeque<u32>,
    tx_fifo: [VecDeque<u8>; Self::NUM_ENDPOINTS],
    virtual_host_step: VirtualHostStep,
    bulk_in_endpoint: Option<usize>,
    bulk_out_endpoint: Option<usize>,
    pending_bridge_writes: Vec<u8>,
}

impl OtgFs {
    pub const USB_RESET: u32 = 1 << 12;
    const GINTSTS: u32 = 0x0014;
    const GINTMSK: u32 = 0x0018;
    const GRSTCTL: u32 = 0x0010;
    // GRSTCTL's W1-to-request, self-clearing-on-completion command bits:
    // CSRST (core soft reset), RXFFLSH (RX FIFO flush), TXFFLSH (TX FIFO
    // flush). Firmware sets one, then polls for hardware to clear it; we
    // complete every requested operation instantly, so none of these must
    // ever be observed as still set.
    const GRSTCTL_CSRST: u32 = 1 << 0;
    const GRSTCTL_RXFFLSH: u32 = 1 << 4;
    const GRSTCTL_TXFFLSH: u32 = 1 << 5;
    const GRSTCTL_SELF_CLEARING: u32 =
        Self::GRSTCTL_CSRST | Self::GRSTCTL_RXFFLSH | Self::GRSTCTL_TXFFLSH;

    // Struct-offset constants from ChibiOS's stm32_otg_t (stm32_otg.h),
    // not the SVD's per-sub-block names — see usb_trace_notes.md for why.
    const GRXSTSR: u32 = 0x001c;
    const GRXSTSP: u32 = 0x0020;

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

    const FIFO_BASE: u32 = 0x1000;
    const FIFO_WINDOW: u32 = 0x1000;

    const GINTSTS_SOF: u32 = 1 << 3;
    const GINTSTS_RXFLVL: u32 = 1 << 4;
    const GINTSTS_IEPINT: u32 = 1 << 18;
    const GINTSTS_OEPINT: u32 = 1 << 19;
    const DIEPCTL_EPENA: u32 = 1 << 31;
    const DOEPCTL_USBAEP: u32 = 1 << 15;
    const DIEPINT_XFRC: u32 = 1 << 0;
    const DIEPINT_TXFE: u32 = 1 << 7;
    // Generous fixed "always enough room" value: this project doesn't model
    // real TX FIFO exhaustion (transfers here are at most one MPS=64 packet),
    // so reporting the configured FIFO0 depth (DIEPTXF0's 16 words, see
    // usb_trace_notes.md) as permanently available is an honest
    // simplification, not a fabrication of unobserved behavior.
    const DTXFSTS_AVAILABLE_WORDS: u32 = 16;
    const DOEPINT_XFRC: u32 = 1 << 0;
    const DOEPINT_STUP: u32 = 1 << 3;
    const XFRSIZ_MASK: u32 = 0x7_ffff;

    const RXSTS_PKTSTS_MASK: u32 = 0xf << 17;
    const RXSTS_SETUP_DATA: u32 = 6 << 17;
    const RXSTS_SETUP_COMP: u32 = 4 << 17;
    const RXSTS_OUT_DATA: u32 = 2 << 17;
    const RXSTS_OUT_COMP: u32 = 3 << 17;

    const VIRTUAL_DEVICE_ADDRESS: u8 = 5;

    pub fn new(name: &str, ext_devices: &ExtDevices) -> Option<Box<dyn Peripheral>> {
        if name == "OTG_FS_GLOBAL" {
            Some(Box::new(Self {
                bridge: ext_devices.find_usb_cdc_tcp(name),
                registers: BTreeMap::new(),
                global_interrupt_status: 0,
                global_interrupt_mask: 0,
                host_attached: false,
                dcfg: 0,
                dctl: 0,
                diep_mask: 0,
                doep_mask: 0,
                daint_mask: 0,
                diep_empty_mask: 0,
                ep_in: [EndpointRegs::default(); Self::NUM_ENDPOINTS],
                ep_out: [EndpointRegs::default(); Self::NUM_ENDPOINTS],
                rx_fifo: VecDeque::new(),
                rx_status: VecDeque::new(),
                tx_fifo: Default::default(),
                virtual_host_step: VirtualHostStep::AwaitingDeviceDescriptor,
                // Endpoint 2 for both directions (firmware's CDC_DATA_IF
                // uses the same endpoint number for IN and OUT, distinguished
                // by direction, not the endpoint number) — from
                // firmware/hw_layer/ports/stm32/serial_over_usb/usbcfg.cpp's
                // USBD1_DATA_REQUEST_EP/USBD1_DATA_AVAILABLE_EP in the
                // epicefi_fw source tree, not live-captured (see
                // usb_trace_notes.md's "Bulk endpoints" section).
                bulk_in_endpoint: Some(2),
                bulk_out_endpoint: Some(2),
                pending_bridge_writes: Vec::new(),
            }))
        } else {
            None
        }
    }

    pub fn for_test() -> Self {
        Self {
            bridge: None,
            registers: BTreeMap::new(),
            global_interrupt_status: 0,
            global_interrupt_mask: 0,
            host_attached: false,
            dcfg: 0,
            dctl: 0,
            diep_mask: 0,
            doep_mask: 0,
            daint_mask: 0,
            diep_empty_mask: 0,
            ep_in: [EndpointRegs::default(); Self::NUM_ENDPOINTS],
            ep_out: [EndpointRegs::default(); Self::NUM_ENDPOINTS],
            rx_fifo: VecDeque::new(),
            rx_status: VecDeque::new(),
            tx_fifo: Default::default(),
            virtual_host_step: VirtualHostStep::AwaitingDeviceDescriptor,
            bulk_in_endpoint: None,
            bulk_out_endpoint: None,
            pending_bridge_writes: Vec::new(),
        }
    }

    pub fn set_bulk_endpoints(&mut self, in_ep: usize, out_ep: usize) {
        self.bulk_in_endpoint = Some(in_ep);
        self.bulk_out_endpoint = Some(out_ep);
    }

    pub fn set_global_interrupt_status(&mut self, value: u32) {
        self.global_interrupt_status |= value;
    }

    pub fn write_global_interrupt_mask(&mut self, value: u32) {
        self.global_interrupt_mask = value;
    }

    fn decode_endpoint(base: u32, offset: u32) -> Option<(usize, u32)> {
        if offset < base {
            return None;
        }
        let rel = offset - base;
        let ep = (rel / Self::EP_STRIDE) as usize;
        (ep < Self::NUM_ENDPOINTS).then_some((ep, rel % Self::EP_STRIDE))
    }

    fn fifo_endpoint(offset: u32) -> Option<usize> {
        if offset < Self::FIFO_BASE {
            return None;
        }
        let ep = ((offset - Self::FIFO_BASE) / Self::FIFO_WINDOW) as usize;
        (ep < Self::NUM_ENDPOINTS).then_some(ep)
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
        // RXFLVL is read-only/level-triggered on real silicon ("RX FIFO
        // non-empty"), not a firmware-clearable status bit — computing it
        // fresh here means register_write's GINTSTS W1C handling correctly
        // has no effect on it, matching hardware, instead of letting
        // firmware's blanket ack clear a bit it can't actually clear.
        if !self.rx_status.is_empty() {
            status |= Self::GINTSTS_RXFLVL;
        }
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

    pub fn interrupt_pending(&self) -> bool {
        self.effective_gintsts() & self.global_interrupt_mask != 0
    }

    pub fn virtual_host_reset(&mut self) {
        self.set_global_interrupt_status(Self::USB_RESET);
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

    // Task 5 extends this again with bulk-endpoint TCP forwarding. Both call
    // sites (here and in the DIEPCTL write handler) funnel through here so
    // neither later task has to patch more than one method.
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

    fn rx_status_word(pktsts: u32, byte_count: u32, endpoint: usize) -> u32 {
        pktsts | (byte_count << 4) | endpoint as u32
    }

    pub fn virtual_host_setup(&mut self, endpoint: usize, packet: [u8; 8]) {
        self.rx_fifo.extend(packet);
        self.rx_status
            .push_back(Self::rx_status_word(Self::RXSTS_SETUP_DATA, 8, endpoint));
        self.rx_status
            .push_back(Self::rx_status_word(Self::RXSTS_SETUP_COMP, 0, endpoint));
        // DOEPINT.STUP is NOT raised here: on real hardware it only fires
        // once the RXFIFO's setup delivery is actually complete, i.e. once
        // firmware pops the SETUP_COMP status word via GRXSTSP (see
        // register_read's GRXSTSP arm below). Raising it immediately here
        // caused firmware's _usb_ep0setup — which ChibiOS's ISR dispatches
        // from OEPINT/STUP *before* it processes RXFLVL in the same
        // interrupt pass — to read stale/unpopulated setup bytes (confirmed
        // by cross-referencing ChibiOS's hal_usb_lld.c against a live
        // trace; see usb_trace_notes.md's "Open question found while
        // reading the source"). GINTSTS.RXFLVL is computed dynamically from
        // rx_status non-emptiness (effective_gintsts), so no explicit set
        // is needed here either.
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

    // Real USB control-IN transfers end with a zero-length OUT status
    // packet the HOST sends to acknowledge the device's IN data — ChibiOS's
    // `_usb_ep0in` arms EP0 to receive exactly this via `usbStartReceiveI`,
    // which sets `usbp->receiving` and asserts it's not already set. This
    // project's virtual host previously skipped the status stage entirely
    // and went straight to the next SETUP, leaving that flag permanently
    // set — harmless until the first later stage that itself calls
    // `usbStartReceiveI` (SET_LINE_CODING, the first OUT-direction stage
    // with a data phase), which then hit the "already receiving" assertion
    // and halted firmware (`chSysHalt`). Root-caused by cross-referencing
    // ChibiOS's exact source (`hal_usb.c:476`) against a live capture using
    // debug symbols from a freshly built, matching firmware image.
    fn virtual_host_control_in_status_ack(&mut self, endpoint: usize) {
        self.rx_status
            .push_back(Self::rx_status_word(Self::RXSTS_OUT_DATA, 0, endpoint));
        self.rx_status
            .push_back(Self::rx_status_word(Self::RXSTS_OUT_COMP, 0, endpoint));
    }

    pub fn read_fifo(&mut self, _endpoint: usize) -> u32 {
        self.pop_rx_fifo_word()
    }

    pub fn is_configured(&self) -> bool {
        self.virtual_host_step == VirtualHostStep::Configured
    }

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

    fn advance_virtual_host(&mut self) {
        self.virtual_host_step = match self.virtual_host_step {
            VirtualHostStep::AwaitingDeviceDescriptor => {
                // GET_DESCRIPTOR is DEV2HOST (IN data stage) — unlike every
                // other stage here, it needs the zero-length OUT status ack
                // before the next SETUP (see virtual_host_control_in_status_ack).
                self.virtual_host_control_in_status_ack(0);
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

    #[cfg(test)]
    fn next_setup_request(&self) -> [u8; 8] {
        let mut packet = [0u8; 8];
        for (i, b) in self.rx_fifo.iter().take(8).enumerate() {
            packet[i] = *b;
        }
        packet
    }

    // &mut self (not &self, as the register model used before this task):
    // reading FIFO_BASE or GRXSTSP is a popping read on real hardware, so
    // this method has always had to mutate once those existed.
    fn register_read(&mut self, offset: u32) -> u32 {
        if let Some(ep) = Self::fifo_endpoint(offset) {
            return if ep == 0 { self.pop_rx_fifo_word() } else { 0 };
        }
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DIEP_BASE, offset) {
            return match reg {
                Self::EP_CTL_OFFSET => self.ep_in[ep].ctl,
                Self::EP_INT_OFFSET => self.ep_in[ep].int,
                Self::EP_TSIZ_OFFSET => self.ep_in[ep].tsiz,
                Self::DTXFSTS_OFFSET => Self::DTXFSTS_AVAILABLE_WORDS,
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
            Self::GRXSTSP => {
                let word = self.rx_status.pop_front().unwrap_or(0);
                // Real hardware only asserts DOEPINT.STUP/XFRC once the
                // RXFIFO's delivery is complete — i.e. right here, when
                // firmware pops the SETUP_COMP/OUT_COMP entry, not when the
                // virtual host first queues the packet (see
                // virtual_host_setup and the bulk OUT forwarding in poll).
                match word & Self::RXSTS_PKTSTS_MASK {
                    Self::RXSTS_SETUP_COMP => {
                        self.raise_out_endpoint_interrupt((word & 0xf) as usize, Self::DOEPINT_STUP)
                    }
                    Self::RXSTS_OUT_COMP => {
                        self.raise_out_endpoint_interrupt((word & 0xf) as usize, Self::DOEPINT_XFRC)
                    }
                    _ => {}
                }
                word
            }
            Self::GRXSTSR => self.rx_status.front().copied().unwrap_or(0),
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
        if let Some(ep) = Self::fifo_endpoint(offset) {
            self.push_tx_fifo_word(ep, value);
            return;
        }
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DIEP_BASE, offset) {
            match reg {
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
                Self::EP_INT_OFFSET => self.ep_in[ep].int &= !value,
                Self::EP_TSIZ_OFFSET => self.ep_in[ep].tsiz = value,
                _ => {}
            }
            return;
        }
        if let Some((ep, reg)) = Self::decode_endpoint(Self::DOEP_BASE, offset) {
            match reg {
                Self::EP_CTL_OFFSET => {
                    let was_active = self.ep_out[ep].ctl & Self::DOEPCTL_USBAEP != 0;
                    self.ep_out[ep].ctl = value;
                    let now_active = value & Self::DOEPCTL_USBAEP != 0;
                    // Real hardware waits for a SETUP token from the host;
                    // our virtual host supplies its own initiative instead,
                    // and this is the first point firmware signals it's
                    // ready to receive one — marking EP0 OUT active during
                    // reset handling (trace-observed write 0x1000_8040 sets
                    // USBAEP, not EPENA — EP0's control transfers don't use
                    // EPENA the way bulk/interrupt endpoints do). Fire only
                    // once, not on every later re-arm.
                    if ep == 0
                        && now_active
                        && !was_active
                        && self.virtual_host_step == VirtualHostStep::AwaitingDeviceDescriptor
                    {
                        self.virtual_host_setup(0, Self::get_device_descriptor_packet());
                    }
                }
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
            Self::DIEPEMPMSK => {
                // Real hardware raises DIEPINT.TXFE as soon as an endpoint's
                // FIFO-empty interrupt is unmasked while a transfer is
                // armed and the FIFO has room (which it always does here —
                // see DTXFSTS_AVAILABLE_WORDS). Firmware's otg_epin_handler
                // dispatches on this bit to actually push the transfer's
                // bytes (otg_txfifo_handler); without raising it here,
                // firmware waits forever for an interrupt that never comes.
                let newly_unmasked = value & !self.diep_empty_mask;
                self.diep_empty_mask = value;
                for ep in 0..Self::NUM_ENDPOINTS {
                    if newly_unmasked & (1 << ep) != 0 && self.ep_in[ep].ctl & Self::DIEPCTL_EPENA != 0 {
                        self.raise_in_endpoint_interrupt(ep, Self::DIEPINT_TXFE);
                    }
                }
            }
            _ => {
                self.registers.insert(offset, value);
            }
        }
    }
}

impl Peripheral for OtgFs {
    fn read(&mut self, _sys: &System, offset: u32) -> u32 {
        self.register_read(offset)
    }

    fn write(&mut self, _sys: &System, offset: u32, value: u32) {
        self.register_write(offset, value);
    }

    fn poll(&mut self, sys: &System) {
        let connected = self
            .bridge
            .as_ref()
            .is_some_and(|bridge| bridge.borrow().is_client_connected());
        if connected && !self.host_attached {
            info!("Virtual USB host attached");
            self.host_attached = true;
            self.virtual_host_reset();
        } else if !connected {
            self.host_attached = false;
        }

        if self.host_attached {
            // A real USB host emits a Start-of-Frame every 1ms regardless of
            // data activity. ChibiOS's serial-over-USB driver depends on
            // this: it's the only thing that flushes a partially-filled TX
            // buffer (sduSOFHookI -> obqTryFlushI), since TsChannelBase's
            // USB channel doesn't override flush(). Without it, any
            // response shorter than the USB buffer size sits in the queue
            // forever and is never transmitted.
            self.set_global_interrupt_status(Self::GINTSTS_SOF);
        }

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
                    // DOEPINT.XFRC and GINTSTS.RXFLVL are NOT raised here for
                    // the same reason virtual_host_setup no longer raises
                    // STUP eagerly: GRXSTSP's OUT_COMP pop (register_read)
                    // raises XFRC once firmware actually consumes the data,
                    // and RXFLVL is computed dynamically from rx_status
                    // non-emptiness (effective_gintsts).
                }
            }
        }

        if self.interrupt_pending() {
            sys.p.nvic.borrow_mut().set_intr_pending(67);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OtgFs, VirtualHostStep};

    #[test]
    fn grstctl_core_soft_reset_clears_immediately() {
        let mut otg = OtgFs::for_test();
        otg.register_write(OtgFs::GRSTCTL, OtgFs::GRSTCTL_CSRST);
        assert_eq!(otg.register_read(OtgFs::GRSTCTL) & OtgFs::GRSTCTL_CSRST, 0);
    }

    #[test]
    fn grstctl_fifo_flush_requests_clear_immediately() {
        let mut otg = OtgFs::for_test();
        otg.register_write(
            OtgFs::GRSTCTL,
            OtgFs::GRSTCTL_RXFFLSH | OtgFs::GRSTCTL_TXFFLSH,
        );
        assert_eq!(
            otg.register_read(OtgFs::GRSTCTL) & OtgFs::GRSTCTL_SELF_CLEARING,
            0
        );
    }

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

    #[test]
    fn sof_bit_raises_masked_interrupt_and_clears_on_write_one() {
        // poll() sets GINTSTS.SOF once per tick while a virtual host is
        // attached, mirroring the continuous 1ms SOF cadence of a real USB
        // bus. ChibiOS's serial-over-USB driver enables GINTMSK.SOFM and
        // never disables it (usbcfg.cpp registers a sof_cb), relying on it
        // to auto-flush partially-filled TX buffers via sduSOFHookI.
        let mut otg = OtgFs::for_test();
        otg.write_global_interrupt_mask(OtgFs::GINTSTS_SOF);
        assert!(!otg.interrupt_pending());
        otg.set_global_interrupt_status(OtgFs::GINTSTS_SOF);
        assert!(otg.interrupt_pending());
        assert_eq!(
            otg.register_read(OtgFs::GINTSTS) & OtgFs::GINTSTS_SOF,
            OtgFs::GINTSTS_SOF
        );
        otg.register_write(OtgFs::GINTSTS, OtgFs::GINTSTS_SOF);
        assert!(!otg.interrupt_pending());
    }

    #[test]
    fn endpoint_zero_setup_packet_is_read_from_fifo() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_reset();
        otg.virtual_host_setup(0, [0x80, 0x06, 0x00, 0x01, 0, 0, 18, 0]);
        assert_eq!(otg.read_fifo(0), 0x0100_0680);
    }

    #[test]
    fn enabling_endpoint_zero_out_after_reset_queues_the_first_get_descriptor_setup() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_reset();
        // Trace-observed value (usb_trace_notes.md): firmware writes
        // oe[0].DOEPCTL = 0x1000_8040 while handling reset — this is the
        // real bit pattern (sets USBAEP, bit 15; NOT EPENA, bit 31, which
        // control endpoint 0 never uses this way), and is the signal our
        // virtual host uses to know firmware is ready for the first SETUP.
        otg.register_write(OtgFs::DOEP_BASE + OtgFs::EP_CTL_OFFSET, 0x1000_8040);
        assert_eq!(
            otg.next_setup_request(),
            OtgFs::get_device_descriptor_packet()
        );
    }

    #[test]
    fn firmware_completing_the_device_descriptor_response_advances_to_set_address() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_reset();
        otg.virtual_host_setup(0, OtgFs::get_device_descriptor_packet());
        // Firmware drains the 8-byte SETUP packet via GRXSTSP + two FIFO
        // words before it ever responds, exactly like the real ChibiOS
        // driver does — without this, the packet would still be sitting at
        // the front of rx_fifo when the next SETUP is queued below.
        otg.register_read(OtgFs::GRXSTSP);
        otg.register_read(OtgFs::FIFO_BASE);
        otg.register_read(OtgFs::FIFO_BASE);
        otg.register_read(OtgFs::GRXSTSP);
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
        // Regression test for a real firmware halt: real USB hosts ack an
        // IN-direction control transfer's data stage with a zero-length OUT
        // status packet before sending the next SETUP. ChibiOS's
        // usbStartReceiveI (hal_usb.c:476) asserts the endpoint isn't
        // already marked "receiving" — if this project's virtual host
        // skips straight to the next SETUP without that status ack, the
        // flag it set while waiting is still set the next time firmware
        // calls usbStartReceiveI (SET_LINE_CODING, the first OUT-direction
        // stage with a data phase), and real firmware halts
        // (chSysHalt) on the failed assertion. Confirmed against a live
        // capture with matching debug symbols before this fix, and again
        // after.
        assert_eq!(
            otg.register_read(OtgFs::GRXSTSP) & OtgFs::RXSTS_PKTSTS_MASK,
            OtgFs::RXSTS_OUT_DATA
        );
        assert_eq!(
            otg.register_read(OtgFs::GRXSTSP) & OtgFs::RXSTS_PKTSTS_MASK,
            OtgFs::RXSTS_OUT_COMP
        );
        assert!(
            otg.register_read(OtgFs::DOEP_BASE + OtgFs::EP_INT_OFFSET) & OtgFs::DOEPINT_XFRC != 0
        );
        assert_eq!(
            otg.register_read(OtgFs::GRXSTSP) & OtgFs::RXSTS_PKTSTS_MASK,
            OtgFs::RXSTS_SETUP_DATA
        );
    }

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

    #[test]
    fn stup_interrupt_fires_only_after_setup_comp_is_popped_not_immediately() {
        let mut otg = OtgFs::for_test();
        otg.virtual_host_setup(0, [0x80, 0x06, 0x00, 0x01, 0, 0, 18, 0]);
        // Queuing the packet must not raise STUP yet — real hardware only
        // does so once the RXFIFO's setup delivery is actually complete.
        assert_eq!(
            otg.register_read(OtgFs::DOEP_BASE + OtgFs::EP_INT_OFFSET),
            0
        );
        otg.register_read(OtgFs::GRXSTSP); // pops SETUP_DATA
        assert_eq!(
            otg.register_read(OtgFs::DOEP_BASE + OtgFs::EP_INT_OFFSET),
            0
        );
        otg.register_read(OtgFs::GRXSTSP); // pops SETUP_COMP
        assert!(
            otg.register_read(OtgFs::DOEP_BASE + OtgFs::EP_INT_OFFSET) & OtgFs::DOEPINT_STUP != 0
        );
    }

    #[test]
    fn rxflvl_reflects_rx_status_occupancy_and_is_not_firmware_clearable() {
        let mut otg = OtgFs::for_test();
        otg.write_global_interrupt_mask(OtgFs::GINTSTS_RXFLVL);
        assert!(!otg.interrupt_pending());
        otg.virtual_host_setup(0, [0x80, 0x06, 0x00, 0x01, 0, 0, 18, 0]);
        assert!(otg.interrupt_pending());
        // A firmware "ack everything" write must not clear RXFLVL — on real
        // silicon it's read-only, level-triggered on FIFO occupancy.
        otg.register_write(OtgFs::GINTSTS, 0xffff_ffff);
        assert!(otg.interrupt_pending());
        otg.register_read(OtgFs::GRXSTSP);
        otg.register_read(OtgFs::GRXSTSP);
        assert!(!otg.interrupt_pending());
    }

    #[test]
    fn enabling_diepempmsk_for_an_armed_nonzero_transfer_raises_txfe() {
        let mut otg = OtgFs::for_test();
        // Arm an 18-byte EP0 IN transfer, matching real firmware's
        // GET_DESCRIPTOR(DEVICE) response sequence: DIEPTSIZ first, then
        // DIEPCTL with EPENA newly set.
        otg.register_write(OtgFs::DIEP_BASE + OtgFs::EP_TSIZ_OFFSET, 18);
        otg.register_write(
            OtgFs::DIEP_BASE + OtgFs::EP_CTL_OFFSET,
            0x1000_8040 | OtgFs::DIEPCTL_EPENA,
        );
        assert_eq!(
            otg.register_read(OtgFs::DIEP_BASE + OtgFs::EP_INT_OFFSET) & OtgFs::DIEPINT_TXFE,
            0
        );
        otg.register_write(OtgFs::DIEPEMPMSK, 0x0000_0001);
        assert_ne!(
            otg.register_read(OtgFs::DIEP_BASE + OtgFs::EP_INT_OFFSET) & OtgFs::DIEPINT_TXFE,
            0
        );
    }

    #[test]
    fn dtxfsts_reports_room_for_a_full_control_packet() {
        let mut otg = OtgFs::for_test();
        assert!(
            otg.register_read(OtgFs::DIEP_BASE + OtgFs::DTXFSTS_OFFSET) * 4 >= 64
        );
    }
}
