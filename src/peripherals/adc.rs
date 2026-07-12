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
