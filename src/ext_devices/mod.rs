// SPDX-License-Identifier: GPL-3.0-or-later

mod spi_flash;
mod usart_probe;
mod display;
mod lcd;
mod touchscreen;
pub mod usb_cdc_tcp;
pub mod ecu_io;

use spi_flash::{SpiFlashConfig, SpiFlash};
use usart_probe::{UsartProbeConfig, UsartProbe};
use display::{DisplayConfig, Display};
use lcd::{LcdConfig, Lcd};
use touchscreen::{TouchscreenConfig, Touchscreen};
use usb_cdc_tcp::{UsbCdcTcpConfig, UsbCdcTcp};
use ecu_io::{EcuIoConfig, EcuIo};

use std::{rc::Rc, cell::RefCell};
use serde::Deserialize;
use anyhow::Result;

use crate::{system::System, framebuffers::Framebuffers, peripherals::gpio::GpioPorts};


#[derive(Debug, Deserialize, Default)]
pub struct ExtDevicesConfig {
    pub spi_flash: Option<Vec<SpiFlashConfig>>,
    pub usart_probe: Option<Vec<UsartProbeConfig>>,
    pub display: Option<Vec<DisplayConfig>>,
    pub lcd: Option<Vec<LcdConfig>>,
    pub touchscreen: Option<Vec<TouchscreenConfig>>,
    pub usb_cdc_tcp: Option<Vec<UsbCdcTcpConfig>>,
    pub ecu_io: Option<Vec<EcuIoConfig>>,
}

#[derive(Default)]
pub struct ExtDevices {
    pub spi_flashes: Vec<Rc<RefCell<SpiFlash>>>,
    pub usart_probes: Vec<Rc<RefCell<UsartProbe>>>,
    pub displays: Vec<Rc<RefCell<Display>>>,
    pub lcds: Vec<Rc<RefCell<Lcd>>>,
    pub touchscreens: Vec<Rc<RefCell<Touchscreen>>>,
    pub usb_cdc_tcps: Vec<Rc<RefCell<UsbCdcTcp>>>,
    pub ecu_ios: Vec<Rc<RefCell<EcuIo>>>,
}

impl ExtDevices {
    pub fn find_serial_device(&self, peri_name: &str) -> Option<Rc<RefCell<dyn ExtDevice<(), u8>>>> {
        self.spi_flashes.iter()
            .filter(|d| d.borrow().config.peripheral == peri_name)
            .next()
            .map(|d| d.clone() as Rc<RefCell<dyn ExtDevice<(), u8>>>)
        .or_else(||
        self.usart_probes.iter()
            .filter(|d| d.borrow().config.peripheral == peri_name)
            .next()
            .map(|d| d.clone() as Rc<RefCell<dyn ExtDevice<(), u8>>>)
       )
        .or_else(||
        self.lcds.iter()
            .filter(|d| d.borrow().config.peripheral == peri_name)
            .next()
            .map(|d| d.clone() as Rc<RefCell<dyn ExtDevice<(), u8>>>)
       )
        .or_else(||
        self.touchscreens.iter()
            .filter(|d| d.borrow().config.peripheral == peri_name)
            .next()
            .map(|d| d.clone() as Rc<RefCell<dyn ExtDevice<(), u8>>>)
       )
    }

    pub fn find_mem_device(&self, peri_name: &str) -> Option<Rc<RefCell<dyn ExtDevice<u32, u32>>>> {
        self.displays.iter()
            .filter(|d| d.borrow().config.peripheral == peri_name)
            .next()
            .map(|d| d.clone() as Rc<RefCell<dyn ExtDevice<u32, u32>>>)
    }

    pub fn find_usb_cdc_tcp(&self, peri_name: &str) -> Option<Rc<RefCell<UsbCdcTcp>>> {
        self.usb_cdc_tcps.iter()
            .find(|d| d.borrow().config.peripheral == peri_name)
            .cloned()
    }

    pub fn poll(&self, sys: &System) {
        for bridge in &self.usb_cdc_tcps {
            if let Err(error) = bridge.borrow_mut().poll() {
                warn!("USB CDC TCP bridge error: {error:#}");
            }
        }
        for ecu_io in &self.ecu_ios {
            if let Err(error) = ecu_io.borrow_mut().poll() {
                warn!("ECU IO bridge error: {error:#}");
            }
            ecu_io.borrow_mut().check_digital_edges(sys);
        }
    }

    pub fn ecu_io(&self) -> Option<Rc<RefCell<EcuIo>>> {
        self.ecu_ios.first().cloned()
    }
}

impl ExtDevicesConfig {
    pub fn into_ext_devices(self, gpio: &mut GpioPorts, framebuffers: &Framebuffers) -> Result<ExtDevices> {
        let spi_flashes = self.spi_flash.unwrap_or_default().into_iter()
            .map(|config| SpiFlash::new(config).map(RefCell::new).map(Rc::new))
            .collect::<Result<_>>()?;

        let usart_probes = self.usart_probe.unwrap_or_default().into_iter()
            .map(|config| UsartProbe::new(config).map(RefCell::new).map(Rc::new))
            .collect::<Result<_>>()?;

        let displays = self.display.unwrap_or_default().into_iter()
            .map(|config| Display::new(config, framebuffers).map(RefCell::new).map(Rc::new))
            .collect::<Result<_>>()?;

        let lcds = self.lcd.unwrap_or_default().into_iter()
            .map(|config| Lcd::new(config, framebuffers).map(RefCell::new).map(Rc::new))
            .collect::<Result<_>>()?;

        let touchscreens = self.touchscreen.unwrap_or_default().into_iter()
            .map(|config| Touchscreen::new(config, gpio, framebuffers).map(RefCell::new).map(Rc::new))
            .collect::<Result<_>>()?;

        let usb_cdc_tcps = self.usb_cdc_tcp.unwrap_or_default().into_iter()
            .map(|config| UsbCdcTcp::new(config).map(RefCell::new).map(Rc::new))
            .collect::<Result<_>>()?;

        let ecu_ios = self.ecu_io.unwrap_or_default().into_iter()
            .map(|config| EcuIo::register(config, gpio))
            .collect::<Result<_>>()?;

        Ok(ExtDevices { spi_flashes, usart_probes, displays, lcds, touchscreens, usb_cdc_tcps, ecu_ios })
    }
}

///////////////////////////////////////////////////////////////////////////////////////

pub trait ExtDevice<A, T> {
    /// Should returns "{peri_name} {ext_device_name}"
    fn connect_peripheral<'a>(&mut self, peri_name: &str) -> String;
    fn read(&mut self, sys: &System, addr: A) -> T;
    fn write(&mut self, sys: &System, addr: A, v: T);
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };

    use super::usb_cdc_tcp::{UsbCdcTcp, UsbCdcTcpConfig};

    #[test]
    fn tcp_client_exchanges_binary_bytes() {
        let mut bridge = UsbCdcTcp::new(UsbCdcTcpConfig {
            peripheral: "OTG_FS_GLOBAL".to_owned(),
            listen: "127.0.0.1:0".to_owned(),
            max_buffered_bytes: 64,
        })
        .unwrap();
        let mut client = TcpStream::connect(bridge.local_addr().unwrap()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();

        bridge.poll().unwrap();
        bridge.push_from_device(&[0x00, 0xff, 0x42]);
        bridge.poll().unwrap();

        let mut received = [0; 3];
        client.read_exact(&mut received).unwrap();
        assert_eq!(received, [0x00, 0xff, 0x42]);

        client.write_all(&[0x10, 0x20]).unwrap();
        for _ in 0..10 {
            bridge.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(bridge.take_for_device(64), vec![0x10, 0x20]);
    }

    #[test]
    fn usb_cdc_tcp_configuration_deserializes() {
        let config: super::ExtDevicesConfig = serde_yaml::from_str(
            "usb_cdc_tcp:\n  - peripheral: OTG_FS_GLOBAL\n    listen: 127.0.0.1:29000\n    max_buffered_bytes: 64\n",
        )
        .unwrap();

        assert_eq!(config.usb_cdc_tcp.unwrap()[0].peripheral, "OTG_FS_GLOBAL");
    }
}
