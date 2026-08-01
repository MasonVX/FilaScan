use alloc::vec::Vec;

use embassy_time::{Duration, Instant};
use hashbrown::HashMap;

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
    let keys = pn532_ext::bambulab_keys(uid);
    let mut authenticated_sector = None;

    // Material IDs, type, color/weight/diameter, temperature/drying data,
    // spool UID/width, production date, length and optional second color.
    const BLOCKS: [u8; PAYLOAD_BLOCK_COUNT] = [1, 2, 4, 5, 6, 9, 10, 12, 13, 14, 16];

    for block_number in BLOCKS {
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

pub fn is_mifare_classic_1k(inlist_response: &[u8]) -> bool {
    if inlist_response.len() < 6 {
        return false;
    }

    matches!(
        (inlist_response[3], inlist_response[4]),
        (0x04, 0x08) | (0x44, 0x08)
    )
}
