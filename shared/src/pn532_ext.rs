use core::{cmp::min, future::Future};

use embassy_time::{Instant, with_deadline};

#[derive(Debug)]
pub enum Error<E: core::fmt::Debug> {
    Pn532(pn532::Error<E>),
    Device(u8),
    Authentication,
}

impl<E: core::fmt::Debug> From<pn532::Error<E>> for Error<E> {
    fn from(value: pn532::Error<E>) -> Self {
        Self::Pn532(value)
    }
}

#[derive(Default)]
pub struct Esp32TimerAsync {
    deadline: Option<Instant>,
}

impl Esp32TimerAsync {
    pub fn new() -> Self {
        Self { deadline: None }
    }
}

impl pn532::CountDown for Esp32TimerAsync {
    type Time = embassy_time::Duration;

    fn start<D: Into<Self::Time>>(&mut self, count: D) {
        self.deadline = Instant::now().checked_add(count.into());
    }

    async fn until_timeout<F: Future>(
        &self,
        future: F,
    ) -> Result<F::Output, embassy_time::TimeoutError> {
        with_deadline(self.deadline.unwrap_or_else(Instant::now), future).await
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn mifare_read_with_retries<I>(
    pn532: &mut pn532::Pn532<I, Esp32TimerAsync>,
    uid: &[u8],
    block_number: u8,
    authenticated_sector: &mut Option<u8>,
    key: &[u8; 6],
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<usize, Error<I::Error>>
where
    I: pn532::Interface,
{
    let sector = block_number / 4;
    if *authenticated_sector != Some(sector) {
        loop {
            if Instant::now() > deadline {
                return Err(Error::Pn532(pn532::Error::TimeoutResponse));
            }

            let response = pn532
                .process(
                    &pn532::Request::mifare_classic_authenticate_block(
                        uid,
                        block_number,
                        pn532::requests::MifareAuthKey::A(key),
                    ),
                    7,
                    deadline - Instant::now(),
                )
                .await?;

            match response[0] {
                0 => {
                    *authenticated_sector = Some(sector);
                    break;
                }
                0x14 => return Err(Error::Authentication),
                _ => continue,
            }
        }
    }

    loop {
        if Instant::now() > deadline {
            return Err(Error::Pn532(pn532::Error::TimeoutResponse));
        }

        let response = pn532
            .process(
                &pn532::Request::mifare_classic_read_data_block(block_number),
                17,
                deadline - Instant::now(),
            )
            .await?;

        if response[0] == 0 {
            let count = min(response.len() - 1, buffer.len());
            buffer[..count].copy_from_slice(&response[1..count + 1]);
            return Ok(count);
        }

        if response[0] == 0x14 {
            return Err(Error::Device(response[0]));
        }
    }
}
