use alloc::{boxed::Box, format, rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;

use embassy_executor::{Spawner, raw::TaskStorage};
use embassy_time::{Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use hashbrown::HashMap;
use log::{error, info, warn};

use crate::{nfc, pn532_ext::Esp32TimerAsync};

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

pub trait BambuReaderObserver {
    fn on_reader_available(&mut self, available: bool);
    fn on_reader_event(&mut self, event: &ReaderEvent);
}

pub struct BambuReader {
    observers: Vec<alloc::rc::Weak<RefCell<dyn BambuReaderObserver>>>,
}

impl BambuReader {
    pub fn subscribe(&mut self, observer: alloc::rc::Weak<RefCell<dyn BambuReaderObserver>>) {
        self.observers.push(observer);
    }

    fn notify_available(&self, available: bool) {
        for observer in &self.observers {
            if let Some(observer) = observer.upgrade() {
                observer.borrow_mut().on_reader_available(available);
            }
        }
    }

    fn notify_event(&self, event: ReaderEvent) {
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
    irq: esp_hal::gpio::Input<'static>,
    spawner: Spawner,
) -> Rc<RefCell<BambuReader>> {
    let reader = Rc::new(RefCell::new(BambuReader {
        observers: Vec::new(),
    }));

    let task = Box::leak(Box::new(TaskStorage::new()))
        .spawn(|| reader_task(reader.clone(), spi_device, irq));
    spawner.spawn(task).ok();
    reader
}

async fn reader_task(
    reader: Rc<RefCell<BambuReader>>,
    spi_device: ExclusiveDevice<
        esp_hal::spi::master::SpiDmaBus<'static, esp_hal::Async>,
        esp_hal::gpio::Output<'static>,
        embassy_time::Delay,
    >,
    irq: esp_hal::gpio::Input<'static>,
) {
    let interface = pn532::spi::SPIInterface {
        spi: spi_device,
        irq: Some(irq),
    };
    let mut pn532 = pn532::Pn532::<_, _, 64>::new(interface, Esp32TimerAsync::new());

    Timer::after_millis(500).await;
    if pn532.wake_up().await.is_err() {
        reader.borrow().notify_available(false);
        return;
    }
    Timer::after_millis(500).await;

    let mut initialized = false;
    for attempt in 0..60 {
        if pn532
            .process(
                &pn532::Request::sam_configuration(pn532::requests::SAMMode::Normal, true),
                0,
                Duration::from_secs(1),
            )
            .await
            .is_ok()
        {
            initialized = true;
            info!("PN532 initialized after {} attempt(s)", attempt + 1);
            break;
        }
        Timer::after_millis(100).await;
    }

    if !initialized {
        error!("PN532 initialization failed");
        reader.borrow().notify_available(false);
        return;
    }

    reader.borrow().notify_available(true);

    // Match SpoolEase's proven reader state model: after a failed payload read,
    // return to InListPassiveTarget and let the PN532 perform a fresh ISO-A
    // activation. Do not issue InRelease/InSelect between MIFARE attempts.
    let mut completed_uid: Option<Vec<u8>> = None;
    let mut completed_uid_last_seen = embassy_time::Instant::now();
    let mut pending_uid: Option<Vec<u8>> = None;
    let mut pending_blocks = HashMap::new();
    let mut pending_attempt = 0_u8;

    loop {
        let response = pn532
            .process(
                &pn532::Request::INLIST_ONE_ISO_A_TARGET,
                17,
                Duration::from_secs(60),
            )
            .await;

        let response = match response {
            Ok(response) => response,
            Err(_) => {
                if completed_uid.take().is_some() || pending_uid.take().is_some() {
                    reader.borrow().notify_event(ReaderEvent::TagRemoved);
                }
                pending_blocks.clear();
                pending_attempt = 0;
                continue;
            }
        };

        if response.len() < 7 || response[0] != 1 {
            continue;
        }

        let uid_len = response[5] as usize;
        if uid_len < 4 || 6 + uid_len > response.len() {
            reader.borrow().notify_event(ReaderEvent::ReadFailed {
                tag_uid: None,
                detail: format!(
                    "Invalid PN532 target response (length {}, UID length {})",
                    response.len(),
                    uid_len
                ),
            });
            continue;
        }

        let uid = response[6..6 + uid_len].to_vec();
        let atqa = [response[2], response[3]];
        let sak = response[4];

        // A continuously present tag is returned repeatedly by InList. Ignore
        // it while responses stay less than 500 ms apart. A later response is
        // treated as a new placement, matching SpoolEase's behavior.
        if completed_uid.as_ref() == Some(&uid) {
            if completed_uid_last_seen.elapsed() < Duration::from_millis(500) {
                completed_uid_last_seen = embassy_time::Instant::now();
                Timer::after_millis(100).await;
                continue;
            }
            reader.borrow().notify_event(ReaderEvent::TagRemoved);
            completed_uid = None;
        }

        if !nfc::is_mifare_classic_1k(response) {
            reader.borrow().notify_event(ReaderEvent::UnsupportedTag {
                tag_uid: uid.clone(),
                atqa,
                sak,
            });
            completed_uid = Some(uid);
            completed_uid_last_seen = embassy_time::Instant::now();
            continue;
        }

        if pending_uid.as_ref() != Some(&uid) {
            pending_uid = Some(uid.clone());
            pending_blocks.clear();
            pending_attempt = 0;
            reader.borrow().notify_event(ReaderEvent::Reading {
                tag_uid: uid.clone(),
                atqa,
                sak,
            });
        }

        pending_attempt += 1;
        match nfc::read_bambulab_payload_into(
            &mut pn532,
            Duration::from_secs(2),
            &uid,
            &mut pending_blocks,
        )
        .await
        {
            Ok(()) => {
                reader.borrow().notify_event(ReaderEvent::Spool {
                    tag_uid: uid.clone(),
                    blocks: core::mem::take(&mut pending_blocks),
                });
                completed_uid = Some(uid);
                completed_uid_last_seen = embassy_time::Instant::now();
                pending_uid = None;
                pending_attempt = 0;
            }
            Err(error) => {
                let detail = format!(
                    "RFID payload attempt {}/5 failed after {}/{} blocks: {error:?}",
                    pending_attempt,
                    pending_blocks.len(),
                    nfc::PAYLOAD_BLOCK_COUNT
                );
                warn!("{}", detail);
                if pending_attempt < 5 {
                    reader.borrow().notify_event(ReaderEvent::Retrying {
                        tag_uid: uid,
                        next_attempt: pending_attempt + 1,
                        detail,
                    });
                } else {
                    error!("{}", detail);
                    reader.borrow().notify_event(ReaderEvent::ReadFailed {
                        tag_uid: Some(uid.clone()),
                        detail,
                    });
                    completed_uid = Some(uid);
                    completed_uid_last_seen = embassy_time::Instant::now();
                    pending_uid = None;
                    pending_blocks.clear();
                    pending_attempt = 0;
                }
            }
        }

        Timer::after_millis(80).await;
    }
}
