use alloc::{format, rc::Rc, vec::Vec};
use core::cell::RefCell;

use embassy_time::{Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use hashbrown::HashMap;
use log::{error, info, warn};

use crate::{
    bambu_reader::{BambuReader, ReaderEvent, ReaderKind},
    nfc,
    pn5180::{Error as Pn5180Error, Pn5180},
};

type Device = Pn5180<
    ExclusiveDevice<
        esp_hal::spi::master::SpiDmaBus<'static, esp_hal::Async>,
        esp_hal::gpio::Output<'static>,
        embassy_time::Delay,
    >,
    esp_hal::gpio::Input<'static>,
    esp_hal::gpio::Output<'static>,
>;

const TAG_REMOVAL_GRACE: Duration = Duration::from_millis(1_500);
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const TRANSIENT_ERROR_RECOVERY_INTERVAL: Duration = Duration::from_secs(8);

#[derive(Debug)]
struct PayloadError {
    block: u8,
    source: Pn5180Error,
}

pub async fn run(reader: Rc<RefCell<BambuReader>>, mut pn5180: Device) {
    if let Err(error) = pn5180.initialize_iso_a().await {
        error!("PN5180 ISO-A initialization failed: {error:?}");
        reader.borrow().notify_available(None);
        return;
    }

    info!("PN5180 initialized for ISO-A and MIFARE Classic");
    reader.borrow().notify_available(Some(ReaderKind::Pn5180));

    let mut completed_uid: Option<[u8; 4]> = None;
    let mut pending_uid: Option<[u8; 4]> = None;
    let mut pending_blocks = HashMap::new();
    let mut pending_attempt = 0_u8;
    let mut prefetched_target = None;
    let mut missing_since: Option<Instant> = None;
    let mut transient_error_since: Option<Instant> = None;
    let mut transient_error_count = 0_u32;

    loop {
        let target = match prefetched_target.take() {
            Some(target) => {
                transient_error_since = None;
                transient_error_count = 0;
                Some(target)
            }
            None => match pn5180.activate_type_a(true).await {
                Ok(target) => {
                    transient_error_since = None;
                    transient_error_count = 0;
                    target
                }
                Err(error) if requires_hardware_reset(error) => {
                    warn!("PN5180 ISO-A activation failed: {error:?}; resetting reader");
                    if let Err(reset_error) = pn5180.reset_and_initialize_iso_a().await {
                        error!("PN5180 reader reset failed: {reset_error:?}");
                    } else {
                        info!("PN5180 reader recovered after hardware reset");
                    }
                    transient_error_since = None;
                    transient_error_count = 0;
                    missing_since = None;
                    Timer::after(POLL_INTERVAL).await;
                    continue;
                }
                Err(error) => {
                    // A partial ISO-A response is normal while a spool enters or
                    // leaves the field, and Bambu spools can expose two tags at
                    // once. It is evidence of RF activity, not confirmed tag
                    // removal, so do not tear down the RF field for every error.
                    transient_error_count = transient_error_count.saturating_add(1);
                    let first_error = *transient_error_since.get_or_insert_with(Instant::now);
                    missing_since = None;
                    if first_error.elapsed() < TRANSIENT_ERROR_RECOVERY_INTERVAL {
                        Timer::after(POLL_INTERVAL).await;
                        continue;
                    }

                    warn!(
                        "PN5180 received {} consecutive transient ISO-A activation errors; refreshing RF field (last: {error:?})",
                        transient_error_count
                    );
                    transient_error_since = None;
                    transient_error_count = 0;
                    match pn5180.reacquire_type_a().await {
                        Ok(target) => target,
                        Err(recovery_error) if requires_hardware_reset(recovery_error) => {
                            warn!("PN5180 RF refresh failed: {recovery_error:?}; resetting reader");
                            if let Err(reset_error) = pn5180.reset_and_initialize_iso_a().await {
                                error!("PN5180 reader reset failed: {reset_error:?}");
                            } else {
                                info!("PN5180 reader recovered after hardware reset");
                            }
                            Timer::after(POLL_INTERVAL).await;
                            continue;
                        }
                        Err(_) => {
                            Timer::after(POLL_INTERVAL).await;
                            continue;
                        }
                    }
                }
            }
        };

        let Some(target) = target else {
            let first_missing = *missing_since.get_or_insert_with(Instant::now);
            if first_missing.elapsed() >= TAG_REMOVAL_GRACE {
                if completed_uid.take().is_some() || pending_uid.take().is_some() {
                    reader.borrow().notify_event(ReaderEvent::TagRemoved);
                }
                pending_blocks.clear();
                pending_attempt = 0;
                missing_since = None;
            }
            Timer::after(POLL_INTERVAL).await;
            continue;
        };
        missing_since = None;

        // A completed placement stays latched while any tag remains in the RF
        // field. Bambu spools carry two tags, and a PN5180 may alternate between
        // them without the spool having moved. Only a confirmed tag-free period
        // starts a new placement.
        if completed_uid.is_some() {
            Timer::after(POLL_INTERVAL).await;
            continue;
        }

        // Do not switch to the other tag of the same spool halfway through a
        // payload read. Keep retrying the UID that started this placement.
        if pending_uid.is_some() && pending_uid != Some(target.uid) {
            Timer::after(POLL_INTERVAL).await;
            continue;
        }

        let uid = target.uid.to_vec();
        if !is_mifare_classic_1k(target.atqa, target.sak) {
            reader.borrow().notify_event(ReaderEvent::UnsupportedTag {
                tag_uid: uid,
                atqa: target.atqa,
                sak: target.sak,
            });
            completed_uid = Some(target.uid);
            continue;
        }

        if pending_uid != Some(target.uid) {
            pending_uid = Some(target.uid);
            pending_blocks.clear();
            pending_attempt = 0;
            reader.borrow().notify_event(ReaderEvent::Reading {
                tag_uid: uid.clone(),
                atqa: target.atqa,
                sak: target.sak,
            });
        }

        pending_attempt += 1;
        match read_payload(&mut pn5180, &target.uid, &mut pending_blocks).await {
            Ok(()) => {
                reader.borrow().notify_event(ReaderEvent::Spool {
                    tag_uid: uid,
                    blocks: core::mem::take(&mut pending_blocks),
                });
                completed_uid = Some(target.uid);
                pending_uid = None;
                pending_attempt = 0;
            }
            Err(read_error) => {
                let detail = format!(
                    "RFID payload attempt {}/5 failed after {}/{} blocks: PN5180 block {}: {:?}",
                    pending_attempt,
                    pending_blocks.len(),
                    nfc::PAYLOAD_BLOCK_COUNT,
                    read_error.block,
                    read_error.source
                );
                if pending_attempt < 5 {
                    reader.borrow().notify_event(ReaderEvent::Retrying {
                        tag_uid: uid,
                        next_attempt: pending_attempt + 1,
                        detail,
                    });
                    prefetched_target = reacquire_with_recovery(&mut pn5180).await;
                } else {
                    error!("{detail}");
                    reader.borrow().notify_event(ReaderEvent::ReadFailed {
                        tag_uid: Some(uid),
                        detail,
                    });
                    completed_uid = Some(target.uid);
                    pending_uid = None;
                    pending_blocks.clear();
                    pending_attempt = 0;
                }
            }
        }

        Timer::after_millis(80).await;
    }
}

async fn reacquire_with_recovery(pn5180: &mut Device) -> Option<crate::pn5180::TypeATarget> {
    match pn5180.reacquire_type_a().await {
        Ok(target) => target,
        Err(error) if requires_hardware_reset(error) => {
            warn!("PN5180 RF reacquisition failed: {error:?}; resetting reader");
            if let Err(reset_error) = pn5180.reset_and_initialize_iso_a().await {
                error!("PN5180 reader reset failed: {reset_error:?}");
                return None;
            }
            info!("PN5180 reader recovered after hardware reset");
            match pn5180.activate_type_a(true).await {
                Ok(target) => target,
                Err(retry_error) => {
                    warn!("PN5180 activation after reset failed: {retry_error:?}");
                    None
                }
            }
        }
        Err(error) => {
            warn!("PN5180 RF reacquisition failed: {error:?}");
            None
        }
    }
}

fn requires_hardware_reset(error: Pn5180Error) -> bool {
    matches!(
        error,
        Pn5180Error::BusyTimeout | Pn5180Error::Spi | Pn5180Error::Pin
    )
}

async fn read_payload(
    pn5180: &mut Device,
    uid: &[u8; 4],
    result: &mut HashMap<i32, Vec<u8>>,
) -> Result<(), PayloadError> {
    let keys = nfc::bambulab_keys(uid);
    let mut authenticated_sector = None;

    for block in nfc::PAYLOAD_BLOCKS {
        if result.contains_key(&(block as i32)) {
            continue;
        }

        let sector = block / 4;
        if authenticated_sector != Some(sector) {
            pn5180
                .authenticate(block, keys.block_key(block), uid)
                .await
                .map_err(|source| PayloadError { block, source })?;
            authenticated_sector = Some(sector);
        }

        let mut data = [0; 16];
        pn5180
            .read_mifare_block(block, &mut data)
            .await
            .map_err(|source| PayloadError { block, source })?;
        result.insert(block as i32, data.to_vec());
    }

    Ok(())
}

fn is_mifare_classic_1k(atqa: [u8; 2], sak: u8) -> bool {
    matches!((atqa, sak), ([0x00, 0x04], 0x08) | ([0x00, 0x44], 0x08))
}
