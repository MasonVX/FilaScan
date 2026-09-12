use alloc::{format, rc::Rc, vec::Vec};
use core::cell::RefCell;

use embassy_time::{Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use hashbrown::HashMap;
use log::{error, info, warn};

use crate::{
    reader::{ReaderEvent, ReaderKind, RfidReader, TagFormat, TagIdentity, TagPayload, TagProtocol},
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

const TAG_REMOVAL_GRACE: Duration = Duration::from_millis(750);
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const TRANSIENT_ERROR_RECOVERY_INTERVAL: Duration = Duration::from_secs(2);
const MAX_PAYLOAD_ATTEMPTS: u8 = 5;
const MAX_STALLED_ATTEMPTS: u8 = 2;
const MAX_OPENPRINTTAG_MEMORY: usize = 2 * 1024;
const OPENPRINTTAG_BLOCK_ATTEMPTS: u8 = 3;

#[derive(Debug)]
struct PayloadError {
    block: u8,
    source: Pn5180Error,
}

pub async fn run(reader: Rc<RefCell<RfidReader>>, mut pn5180: Device) {
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
    let mut stalled_attempts = 0_u8;
    let mut prefetched_target = None;
    let mut missing_since: Option<Instant> = None;
    let mut transient_error_since: Option<Instant> = None;
    let mut transient_error_count = 0_u32;
    let mut completed_type_v_uid: Option<[u8; 8]> = None;
    let mut empty_iso_a_polls = 0_u8;

    loop {
        if let Some(uid) = completed_type_v_uid {
            match pn5180.inventory_type_v().await {
                Ok(Some(target)) if target.uid == uid => {
                    missing_since = None;
                    Timer::after(POLL_INTERVAL).await;
                    continue;
                }
                Ok(Some(_)) => {
                    missing_since = None;
                    Timer::after(POLL_INTERVAL).await;
                    continue;
                }
                Ok(None) | Err(_) => {
                    let first_missing = *missing_since.get_or_insert_with(Instant::now);
                    if first_missing.elapsed() < TAG_REMOVAL_GRACE {
                        Timer::after(POLL_INTERVAL).await;
                        continue;
                    }
                    reader.borrow().notify_event(ReaderEvent::TagRemoved);
                    completed_type_v_uid = None;
                    missing_since = None;
                    if let Err(error) = pn5180.initialize_iso_a().await {
                        error!("PN5180 could not return to ISO-A after NFC-V tag removal: {error:?}");
                    }
                    Timer::after(POLL_INTERVAL).await;
                    continue;
                }
            }
        }

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
            if completed_uid.is_none() && pending_uid.is_none() {
                empty_iso_a_polls = empty_iso_a_polls.saturating_add(1);
                if empty_iso_a_polls >= 2 {
                    empty_iso_a_polls = 0;
                    match try_read_openprinttag(&reader, &mut pn5180).await {
                        Ok(Some(uid)) => {
                            completed_type_v_uid = Some(uid);
                            missing_since = None;
                            Timer::after(POLL_INTERVAL).await;
                            continue;
                        }
                        Ok(None) => {}
                        Err(detail) => {
                            error!("OpenPrintTag scan failed: {detail}");
                        }
                    }
                    if let Err(error) = pn5180.initialize_iso_a().await {
                        error!("PN5180 could not return to ISO-A after NFC-V scan: {error:?}");
                    }
                }
            }
            let first_missing = *missing_since.get_or_insert_with(Instant::now);
            if first_missing.elapsed() >= TAG_REMOVAL_GRACE {
                if completed_uid.take().is_some() || pending_uid.take().is_some() {
                    reader.borrow().notify_event(ReaderEvent::TagRemoved);
                }
                pending_blocks.clear();
                pending_attempt = 0;
                stalled_attempts = 0;
                missing_since = None;
            }
            Timer::after(POLL_INTERVAL).await;
            continue;
        };
        missing_since = None;
        empty_iso_a_polls = 0;

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
        let tag = TagIdentity {
            uid: uid.clone(),
            protocol: TagProtocol::Iso14443A { atqa: target.atqa, sak: target.sak },
        };
        if !is_mifare_classic_1k(target.atqa, target.sak) {
            reader.borrow().notify_event(ReaderEvent::UnsupportedTag {
                tag,
                detail: "expected MIFARE Classic 1K",
            });
            completed_uid = Some(target.uid);
            continue;
        }

        if pending_uid != Some(target.uid) {
            pending_uid = Some(target.uid);
            pending_blocks.clear();
            pending_attempt = 0;
            stalled_attempts = 0;
            reader.borrow().notify_event(ReaderEvent::Reading {
                tag: tag.clone(),
                format: TagFormat::BambuLab,
            });
        }

        pending_attempt += 1;
        let blocks_before_attempt = pending_blocks.len();
        match read_payload(&mut pn5180, &target.uid, &mut pending_blocks).await {
            Ok(()) => {
                reader.borrow().notify_event(ReaderEvent::TagRead {
                    tag,
                    payload: TagPayload::BambuClassic { blocks: core::mem::take(&mut pending_blocks) },
                });
                completed_uid = Some(target.uid);
                pending_uid = None;
                pending_attempt = 0;
                stalled_attempts = 0;
            }
            Err(read_error) => {
                if pending_blocks.len() > blocks_before_attempt {
                    stalled_attempts = 0;
                } else {
                    stalled_attempts = stalled_attempts.saturating_add(1);
                }
                let detail = format!(
                    "RFID payload attempt {}/{} failed after {}/{} blocks: PN5180 block {}: {:?}",
                    pending_attempt,
                    MAX_PAYLOAD_ATTEMPTS,
                    pending_blocks.len(),
                    nfc::PAYLOAD_BLOCK_COUNT,
                    read_error.block,
                    read_error.source
                );
                if pending_attempt < MAX_PAYLOAD_ATTEMPTS && stalled_attempts < MAX_STALLED_ATTEMPTS {
                    reader.borrow().notify_event(ReaderEvent::Retrying {
                        tag_uid: uid,
                        next_attempt: pending_attempt + 1,
                        detail,
                    });
                    prefetched_target = reacquire_with_recovery(&mut pn5180).await;
                } else {
                    let detail = if stalled_attempts >= MAX_STALLED_ATTEMPTS {
                        format!("{detail}; stopping after {MAX_STALLED_ATTEMPTS} attempts without progress")
                    } else {
                        detail
                    };
                    error!("{detail}");
                    reader.borrow().notify_event(ReaderEvent::ReadFailed {
                        tag_uid: Some(uid),
                        detail,
                    });
                    completed_uid = Some(target.uid);
                    pending_uid = None;
                    pending_blocks.clear();
                    pending_attempt = 0;
                    stalled_attempts = 0;
                }
            }
        }

        Timer::after_millis(80).await;
    }
}

async fn try_read_openprinttag(
    reader: &Rc<RefCell<RfidReader>>,
    pn5180: &mut Device,
) -> Result<Option<[u8; 8]>, alloc::string::String> {
    pn5180
        .initialize_iso_v()
        .await
        .map_err(|error| format!("could not initialize NFC-V: {error:?}"))?;
    let Some(target) = pn5180
        .inventory_type_v()
        .await
        .map_err(|error| format!("NFC-V inventory failed: {error:?}"))?
    else {
        return Ok(None);
    };
    let info = match pn5180.type_v_system_info(&target.uid).await {
        Ok(info) => info,
        Err(error) => {
            let detail = format!("NFC-V system information failed: {error:?}");
            reader.borrow().notify_event(ReaderEvent::ReadFailed {
                tag_uid: Some(target.uid.to_vec()),
                detail,
            });
            return Ok(Some(target.uid));
        }
    };
    let Some(memory_size) = info.block_size.checked_mul(info.block_count).filter(|size| *size <= MAX_OPENPRINTTAG_MEMORY)
    else {
        let detail = format!("NFC-V memory size is unsupported: {} x {} bytes", info.block_count, info.block_size);
        reader.borrow().notify_event(ReaderEvent::ReadFailed {
            tag_uid: Some(target.uid.to_vec()),
            detail,
        });
        return Ok(Some(target.uid));
    };
    let tag = TagIdentity {
        uid: target.uid.to_vec(),
        protocol: TagProtocol::Iso15693 { block_size: info.block_size, block_count: info.block_count },
    };
    reader.borrow().notify_event(ReaderEvent::Reading { tag: tag.clone(), format: TagFormat::OpenPrintTag });

    let mut memory = Vec::with_capacity(memory_size);
    for block in 0..info.block_count {
        let mut data = [0_u8; 32];
        let mut last_error = None;
        for attempt in 1..=OPENPRINTTAG_BLOCK_ATTEMPTS {
            match pn5180.read_type_v_block(&target.uid, block as u8, &mut data[..info.block_size]).await {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(error) => {
                    last_error = Some(error);
                    if attempt < OPENPRINTTAG_BLOCK_ATTEMPTS {
                        Timer::after_millis(15).await;
                    }
                }
            }
        }
        if let Some(error) = last_error {
            let detail = format!(
                "NFC-V block {block} failed after {OPENPRINTTAG_BLOCK_ATTEMPTS} attempts: {error:?}"
            );
            reader.borrow().notify_event(ReaderEvent::ReadFailed {
                tag_uid: Some(target.uid.to_vec()),
                detail,
            });
            return Ok(Some(target.uid));
        }
        memory.extend_from_slice(&data[..info.block_size]);
    }

    reader.borrow().notify_event(ReaderEvent::TagRead {
        tag,
        payload: TagPayload::OpenPrintTag { memory },
    });
    Ok(Some(target.uid))
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
