use alloc::vec::Vec;
use core::convert::TryInto;

use embassy_time::{Duration, Instant};
use hashbrown::HashMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::pn532_ext::{self, Esp32TimerAsync};

#[derive(Debug)]
pub enum Error<E: core::fmt::Debug> {
    Reader {
        block: u8,
        source: pn532_ext::Error<E>,
    },
    Authentication(u8),
    IncompleteBlock {
        block: u8,
        received: usize,
    },
}

/// Reads only the blocks used by FilaScan's Bambu spool overview.
pub async fn read_bambulab_payload_into<I>(
    pn532: &mut pn532::Pn532<I, Esp32TimerAsync>,
    timeout: Duration,
    uid: &[u8],
    result: &mut HashMap<i32, Vec<u8>>,
) -> Result<(), Error<I::Error>>
where
    I: pn532::Interface,
{
    let deadline = Instant::now() + timeout;
    let keys = bambulab_keys(uid);
    let mut authenticated_sector = None;

    // Material IDs, type, color/weight/diameter, temperature/drying data,
    // spool UID/width, production date, length and optional second color.
    for block_number in PAYLOAD_BLOCKS {
        if result.contains_key(&(block_number as i32)) {
            continue;
        }

        let mut block = alloc::vec![0_u8; 16];
        match pn532_ext::mifare_read_with_retries(
            pn532,
            uid,
            block_number,
            &mut authenticated_sector,
            keys.block_key(block_number),
            &mut block,
            deadline,
        )
        .await
        {
            Ok(16) => {
                result.insert(block_number as i32, block);
            }
            Ok(received) => {
                return Err(Error::IncompleteBlock {
                    block: block_number,
                    received,
                });
            }
            Err(pn532_ext::Error::Authentication) => {
                return Err(Error::Authentication(block_number));
            }
            Err(error) => {
                return Err(Error::Reader {
                    block: block_number,
                    source: error,
                });
            }
        }
    }

    Ok(())
}

pub const PAYLOAD_BLOCK_COUNT: usize = 11;
pub const PAYLOAD_BLOCKS: [u8; PAYLOAD_BLOCK_COUNT] = [1, 2, 4, 5, 6, 9, 10, 12, 13, 14, 16];

pub struct BambuLabKeys {
    bytes: Vec<u8>,
}

impl BambuLabKeys {
    pub fn block_key(&self, block_number: u8) -> &[u8; 6] {
        let sector = block_number as usize / 4;
        self.bytes[sector * 6..(sector + 1) * 6]
            .try_into()
            .expect("Bambu key must contain six bytes")
    }
}

pub fn bambulab_keys(uid: &[u8]) -> BambuLabKeys {
    const MASTER_KEY: [u8; 16] = [
        0x9a, 0x75, 0x9c, 0xf2, 0xc4, 0xf7, 0xca, 0xff, 0x22, 0x2c, 0xb9, 0x76, 0x9b, 0x41, 0xbc,
        0x96,
    ];
    const CONTEXT: &[u8] = b"RFID-A\0";
    const TOTAL_LENGTH: usize = 16 * 6;

    let mut extract = Hmac::<Sha256>::new_from_slice(&MASTER_KEY).unwrap();
    extract.update(uid);
    let pseudo_random_key = extract.finalize().into_bytes();

    let mut bytes = Vec::with_capacity(TOTAL_LENGTH);
    let mut previous = Vec::new();
    for index in 1..=TOTAL_LENGTH.div_ceil(32) {
        let mut expand = Hmac::<Sha256>::new_from_slice(&pseudo_random_key).unwrap();
        expand.update(&previous);
        expand.update(CONTEXT);
        expand.update(&[index as u8]);
        previous = expand.finalize().into_bytes().to_vec();
        bytes.extend_from_slice(&previous);
    }
    bytes.truncate(TOTAL_LENGTH);

    BambuLabKeys { bytes }
}

pub fn is_mifare_classic_1k(inlist_response: &[u8]) -> bool {
    if inlist_response.len() < 6 {
        return false;
    }

    matches!(
        (inlist_response[3], inlist_response[4]),
        (0x04, 0x08) | (0x44, 0x08)
    )
}
