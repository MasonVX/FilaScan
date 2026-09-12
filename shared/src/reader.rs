use alloc::{boxed::Box, rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;

use embassy_executor::{Spawner, raw::TaskStorage};
use embedded_hal_bus::spi::ExclusiveDevice;
use hashbrown::HashMap;
use log::{error, info};

use crate::{pn5180::Pn5180, pn5180_reader, pn532_reader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderKind {
    Pn532,
    Pn5180,
}

impl ReaderKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pn532 => "PN532",
            Self::Pn5180 => "PN5180",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderMode {
    Auto,
    Pn532,
    Pn5180,
}

pub enum ReaderEvent {
    Reading {
        tag_uid: Vec<u8>,
        atqa: [u8; 2],
        sak: u8,
    },
    Retrying {
        tag_uid: Vec<u8>,
        next_attempt: u8,
        detail: String,
    },
    Spool {
        tag_uid: Vec<u8>,
        blocks: HashMap<i32, Vec<u8>>,
    },
    UnsupportedTag {
        tag_uid: Vec<u8>,
        atqa: [u8; 2],
        sak: u8,
    },
    ReadFailed {
        tag_uid: Option<Vec<u8>>,
        detail: String,
    },
    TagRemoved,
}

pub trait RfidReaderObserver {
    fn on_reader_available(&mut self, reader: Option<ReaderKind>);
    fn on_reader_event(&mut self, event: &ReaderEvent);
}

pub struct RfidReader {
    observers: Vec<alloc::rc::Weak<RefCell<dyn RfidReaderObserver>>>,
}

impl RfidReader {
    pub fn subscribe(&mut self, observer: alloc::rc::Weak<RefCell<dyn RfidReaderObserver>>) {
        self.observers.push(observer);
    }

    pub(crate) fn notify_available(&self, reader: Option<ReaderKind>) {
        for observer in &self.observers {
            if let Some(observer) = observer.upgrade() {
                observer.borrow_mut().on_reader_available(reader);
            }
        }
    }

    pub(crate) fn notify_event(&self, event: ReaderEvent) {
        for observer in &self.observers {
            if let Some(observer) = observer.upgrade() {
                observer.borrow_mut().on_reader_event(&event);
            }
        }
    }
}

pub fn init(
    spi_device: ExclusiveDevice<
        esp_hal::spi::master::SpiDmaBus<'static, esp_hal::Async>,
        esp_hal::gpio::Output<'static>,
        embassy_time::Delay,
    >,
    signal: esp_hal::gpio::Input<'static>,
    reset: esp_hal::gpio::Output<'static>,
    mode: ReaderMode,
    spawner: Spawner,
) -> Rc<RefCell<RfidReader>> {
    let reader = Rc::new(RefCell::new(RfidReader {
        observers: Vec::new(),
    }));

    let task = Box::leak(Box::new(TaskStorage::new()))
        .spawn(|| reader_task(reader.clone(), spi_device, signal, reset, mode));
    spawner.spawn(task).ok();
    reader
}

async fn reader_task(
    reader: Rc<RefCell<RfidReader>>,
    spi_device: ExclusiveDevice<
        esp_hal::spi::master::SpiDmaBus<'static, esp_hal::Async>,
        esp_hal::gpio::Output<'static>,
        embassy_time::Delay,
    >,
    signal: esp_hal::gpio::Input<'static>,
    reset: esp_hal::gpio::Output<'static>,
    mode: ReaderMode,
) {
    let mut pn5180 = Pn5180::new(spi_device, signal, reset);
    if mode != ReaderMode::Pn532 {
        match pn5180.probe().await {
            Ok(version) => {
                info!(
                    "PN5180 detected (product {}.{}, firmware {}.{}, EEPROM {}.{})",
                    version.product[1],
                    version.product[0],
                    version.firmware[1],
                    version.firmware[0],
                    version.eeprom[1],
                    version.eeprom[0]
                );
                pn5180_reader::run(reader, pn5180).await;
                return;
            }
            Err(error) if mode == ReaderMode::Pn5180 => {
                error!("Configured PN5180 reader was not detected: {error:?}");
                reader.borrow().notify_available(None);
                return;
            }
            Err(error) => info!("PN5180 probe did not match ({error:?}); trying PN532"),
        }
    }

    let (spi_device, irq, _reset) = pn5180.into_parts();
    pn532_reader::run(reader, spi_device, irq).await;
}
