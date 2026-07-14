// SPDX-License-Identifier: GPL-3.0-or-later

use super::Peripheral;
use super::Peripherals;
use crate::system::System;
use crate::util::UniErr;

#[derive(Default)]
pub struct Dma {
    name: String,
    streams: [Stream; 8],
    // DMA_LISR/DMA_HISR: one 6-bit flag group per stream (streams 0-3 in
    // lisr, 4-7 in hisr). Only TCIF (transfer complete) is ever set here --
    // nothing in this codebase needs HTIF/TEIF/DMEIF/FEIF yet. ChibiOS's
    // blocking ADC conversion (adcConvert(), used by rusEFI's slow-ADC
    // reads) waits on this stream's transfer-complete interrupt to signal
    // a semaphore; without it, that wait never returns and firmware hangs
    // forever right after arming the transfer.
    lisr: u32,
    hisr: u32,
}

impl Dma {
    pub fn new(name: &str) -> Option<Box<dyn Peripheral>> {
        if name.starts_with("DMA") {
            let name = name.to_string();
            Some(Box::new(Self {
                name,
                ..Self::default()
            }))
        } else {
            None
        }
    }

    // TCIF sits at bit 5 of each stream's 6-bit flag group; the groups
    // themselves are packed at bit offsets 0, 6, 16, 22 (not evenly spaced
    // by 6 throughout -- bits 12-15 are reserved), per the STM32F4/F7
    // reference manual's DMA_LISR/DMA_HISR layout.
    fn tcif_bit(local_index: usize) -> u32 {
        let base = if local_index < 2 {
            local_index * 6
        } else {
            16 + (local_index - 2) * 6
        };
        1 << (base + 5)
    }

    fn set_transfer_complete_flag(&mut self, stream_index: usize) {
        let bit = Self::tcif_bit(stream_index % 4);
        if stream_index < 4 {
            self.lisr |= bit;
        } else {
            self.hisr |= bit;
        }
    }

    // DMA1/DMA2 stream global interrupts are at fixed NVIC positions across
    // the whole STM32F4/F7 family (RM0410's vector table).
    fn stream_irq(dma_name: &str, stream_index: usize) -> Option<i32> {
        match (dma_name, stream_index) {
            ("DMA1", 0) => Some(11),
            ("DMA1", 1) => Some(12),
            ("DMA1", 2) => Some(13),
            ("DMA1", 3) => Some(14),
            ("DMA1", 4) => Some(15),
            ("DMA1", 5) => Some(16),
            ("DMA1", 6) => Some(17),
            ("DMA1", 7) => Some(47),
            ("DMA2", 0) => Some(56),
            ("DMA2", 1) => Some(57),
            ("DMA2", 2) => Some(58),
            ("DMA2", 3) => Some(59),
            ("DMA2", 4) => Some(60),
            ("DMA2", 5) => Some(68),
            ("DMA2", 6) => Some(69),
            ("DMA2", 7) => Some(70),
            _ => None,
        }
    }
}

impl Peripheral for Dma {
    fn read(&mut self, sys: &System, offset: u32) -> u32 {
        match Access::from_offset(offset) {
            Access::Reg(0x00) => self.lisr,
            Access::Reg(0x04) => self.hisr,
            Access::StreamReg(i, offset) => self.streams[i].read(&self.name, sys, offset),
            _ => 0,
        }
    }

    fn write(&mut self, sys: &System, offset: u32, value: u32) {
        match Access::from_offset(offset) {
            // LIFCR/HIFCR: write-1-to-clear.
            Access::Reg(0x08) => self.lisr &= !value,
            Access::Reg(0x0c) => self.hisr &= !value,
            Access::StreamReg(i, offset) => {
                if self.streams[i].write(&self.name, sys, offset, value) {
                    self.set_transfer_complete_flag(i);
                    let tcie = self.streams[i].cr & (1 << 4) != 0;
                    if tcie {
                        if let Some(irq) = Self::stream_irq(&self.name, i) {
                            sys.p.nvic.borrow_mut().set_intr_pending(irq);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct Stream {
    pub cr: u32,
    pub next_cr: Option<u32>,
    pub ndtr: u32,
    pub par: u32,
    pub m0ar: u32,
    pub m1ar: u32,
    pub fcr: u32,
}

impl Stream {
    fn channel(&self) -> u8 {
        ((self.cr >> 25) & 0b111) as u8
    }

    fn dir(&self) -> Dir {
        match (self.cr >> 6) & 0b11 {
            0b00 => Dir::Read,
            0b01 => Dir::Write,
            0b10 => Dir::MemCopy,
            _ => Dir::Invalid,
        }
    }

    // 1, 2, 4 (8bit, 16bit, 32bit)
    fn word_size(&self) -> usize {
        match (self.cr >> 11) & 0b11 {
            0b00 => 1,
            0b01 => 2,
            0b10 => 4,
            _ => 1,
        }
    }

    fn data_size(&self) -> usize {
        self.word_size() * self.ndtr as usize
    }

    fn data_addr(&self) -> u32 {
        if (self.cr >> 19) & 1 != 0 {
            self.m1ar
        } else {
            self.m0ar
        }
    }

    fn do_xfer(&self, name: &str, sys: &System) {
        let dir = self.dir();
        let data_addr = self.data_addr();
        let size = self.data_size();
        let peri_addr = self.par;

        let peri = Peripherals::get_peripheral(&sys.p.peripherals, peri_addr);

        let (src, dst) = match dir {
            Dir::Read => (peri_addr, data_addr),
            Dir::Write => (data_addr, peri_addr),
            Dir::MemCopy => (peri_addr, data_addr),
            Dir::Invalid => (0, 0),
        };

        if log::log_enabled!(log::Level::Trace) {
            // Firmware-driven peripherals like the slow-ADC sampling loop
            // (500Hz+) re-arm a DMA transfer continuously once it's actually
            // completing (see the transfer-complete interrupt fix) -- this
            // fires far too often for the default -v/Debug level the
            // launcher GUI always runs at.
            let peri_desc = sys.p.addr_desc(peri_addr);
            trace!(
                "{} xfer initiated channel={} peri_{} dir={:?} addr=0x{:08x} size={}",
                name,
                self.channel(),
                peri_desc,
                dir,
                data_addr,
                size
            );
        }

        let buf = match dir {
            Dir::Read => peri.map(|p| {
                p.peripheral
                    .borrow_mut()
                    .read_dma(sys, peri_addr - p.start, size)
            }),
            Dir::Write | Dir::MemCopy => sys
                .uc
                .borrow()
                .mem_read_as_vec(src.into(), size)
                .map_err(|e| {
                    warn!(
                        "DMA read failed addr=0x{:08x} size={} e={}",
                        src,
                        size,
                        UniErr(e)
                    )
                })
                .map(|v| v.into())
                .ok(),
            Dir::Invalid => Some(vec![].into()),
        };

        let mut buf = buf.unwrap_or_else(|| {
            let mut rx = vec![];
            rx.resize(size, 0);
            rx.into()
        });

        trace!("{} xfer buf={:x?}", name, buf);

        match dir {
            Dir::Write => {
                peri.map(|p| {
                    p.peripheral
                        .borrow_mut()
                        .write_dma(sys, peri_addr - p.start, buf)
                });
            }
            Dir::Read | Dir::MemCopy => {
                if let Err(e) = sys
                    .uc
                    .borrow_mut()
                    .mem_write(dst.into(), buf.make_contiguous())
                {
                    warn!(
                        "DMA read failed addr=0x{:08x} size={} e={}",
                        dst,
                        size,
                        UniErr(e)
                    );
                }
            }
            Dir::Invalid => {}
        }
    }

    pub fn read(&mut self, _name: &str, _sys: &System, offset: u32) -> u32 {
        match offset {
            0x0000 => {
                let v = self.cr;
                if let Some(next_cr) = self.next_cr.take() {
                    self.cr = next_cr;
                }

                // The saturn firmware is a bit buggy. When doing a DMA write
                // with size=0, they don't enable the DMA channel, but they
                // wait for it to go to 1 and then 0, with a timeout. So they
                // are consistently hitting the timeout.
                // We'll do toggles on the ready flag to speed things up avoiding the timeout.
                if self.dir() == Dir::Write && self.data_size() == 0 {
                    self.next_cr = Some(self.cr ^ 1)
                }

                v
            }
            0x0004 => self.ndtr,
            0x0008 => self.par,
            0x000c => self.m0ar,
            0x0010 => self.m1ar,
            0x0014 => self.fcr,
            _ => 0,
        }
    }

    /// Returns whether a transfer was just completed (enable bit was set),
    /// so the caller (which owns the shared LISR/HISR/NVIC state) can raise
    /// the completion flag and interrupt.
    pub fn write(&mut self, name: &str, sys: &System, offset: u32, mut value: u32) -> bool {
        match offset {
            0x0000 => {
                self.cr = value;

                // CRx register
                if value & 1 != 0 {
                    // Enable is on. do the transfer.
                    self.do_xfer(name, sys);

                    value &= !1;
                    self.ndtr = 0;
                    self.next_cr = Some(value);
                    return true;
                }
                false
            }
            0x0004 => {
                self.ndtr = value & 0xFFFF;
                false
            }
            0x0008 => {
                self.par = value;
                false
            }
            0x000c => {
                self.m0ar = value;
                false
            }
            0x0010 => {
                self.m1ar = value;
                false
            }
            0x0014 => {
                self.fcr = value;
                false
            }
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Dir {
    Read,
    Write,
    MemCopy,
    Invalid,
}

enum Access {
    Reg(u32),
    /// CR0, CR1, etc.
    StreamReg(usize, u32),
}

impl Access {
    pub fn from_offset(offset: u32) -> Self {
        if offset < 0x28 {
            Access::Reg(offset)
        } else {
            let stride = 0x18;
            let start = 0x10;

            let offset = offset - start;
            Access::StreamReg((offset / stride) as usize, offset % stride)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use unicorn_engine::{
        unicorn_const::{Arch, Mode},
        ArmCpuModel, Unicorn,
    };

    use super::{Dma, Peripheral};
    use crate::{ext_devices::ExtDevices, peripherals::Peripherals, system::System};

    fn test_parts() -> (Unicorn<'static, ()>, Rc<Peripherals>, Rc<ExtDevices>) {
        let mut uc = Unicorn::new(Arch::ARM, Mode::THUMB | Mode::LITTLE_ENDIAN).unwrap();
        uc.ctl_set_cpu_model(ArmCpuModel::CORTEX_M4 as i32).unwrap();
        (uc, Rc::new(Peripherals::default()), Rc::new(ExtDevices::default()))
    }

    fn dma2() -> Dma {
        Dma { name: "DMA2".to_owned(), ..Dma::default() }
    }

    // Stream 4's CR register offset: start (0x10) + index * stride (0x18).
    const STREAM4_CR: u32 = 0x10 + 4 * 0x18;
    const EN: u32 = 1;
    const TCIE: u32 = 1 << 4;

    #[test]
    fn stream_irq_matches_the_reference_manual_vector_table() {
        // DMA1 Stream7 is the one entry that breaks the otherwise-regular
        // spacing (jumps to 47 instead of continuing the DMA1 sequence) --
        // exactly the kind of thing a copy-paste table is likely to get
        // wrong.
        assert_eq!(Dma::stream_irq("DMA1", 7), Some(47));
        // DMA2 Stream4 is what ADC1's slow (blocking) conversions use.
        assert_eq!(Dma::stream_irq("DMA2", 4), Some(60));
        assert_eq!(Dma::stream_irq("DMA3", 0), None);
    }

    #[test]
    fn enabling_a_stream_transfer_raises_its_completion_interrupt_when_tcie_is_set() {
        // ChibiOS's blocking ADC conversion (adcConvert(), used by rusEFI's
        // slow-ADC reads) waits on exactly this interrupt to signal a
        // semaphore; without it, firmware hangs forever right after
        // arming the transfer.
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p: p.clone(), d };
        let mut dma = dma2();

        dma.write(&sys, STREAM4_CR + 0x04, 0); // NDTR = 0, nothing to move
        dma.write(&sys, STREAM4_CR, EN | TCIE);

        assert_eq!(
            p.nvic.borrow_mut().get_and_clear_next_intr_pending(),
            Some(60)
        );
    }

    #[test]
    fn transfer_complete_flag_sets_but_is_not_forwarded_to_nvic_without_tcie() {
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p: p.clone(), d };
        let mut dma = dma2();

        dma.write(&sys, STREAM4_CR + 0x04, 0);
        dma.write(&sys, STREAM4_CR, EN);

        assert_eq!(p.nvic.borrow_mut().get_and_clear_next_intr_pending(), None);
        assert_ne!(dma.read(&sys, 0x04) & Dma::tcif_bit(0), 0);
    }

    #[test]
    fn interrupt_flag_clear_register_clears_the_flag() {
        let (mut uc, p, d) = test_parts();
        let sys = System { uc: RefCell::new(&mut uc), p, d };
        let mut dma = dma2();

        dma.write(&sys, STREAM4_CR + 0x04, 0);
        dma.write(&sys, STREAM4_CR, EN);
        let bit = Dma::tcif_bit(0);
        assert_ne!(dma.read(&sys, 0x04) & bit, 0);

        dma.write(&sys, 0x0c, bit); // HIFCR
        assert_eq!(dma.read(&sys, 0x04) & bit, 0);
    }
}
