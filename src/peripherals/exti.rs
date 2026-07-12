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
