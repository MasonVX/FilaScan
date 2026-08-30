use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::spi::SpiDevice;

const SYSTEM_CONFIG: u8 = 0x00;
const IRQ_STATUS: u8 = 0x02;
const IRQ_CLEAR: u8 = 0x03;
const CRC_RX_CONFIG: u8 = 0x12;
const RX_STATUS: u8 = 0x13;
const CRC_TX_CONFIG: u8 = 0x19;
const RF_STATUS: u8 = 0x1d;

const RX_IRQ: u32 = 1 << 0;
const RX_ERROR_MASK: u32 = 0x0007_0000;

const PRODUCT_VERSION: u8 = 0x10;
const FIRMWARE_VERSION: u8 = 0x12;
const EEPROM_VERSION: u8 = 0x14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    Pin,
    BusyTimeout,
    ResponseTimeout,
    InvalidResponse,
    ReceiveStatus(u32),
    Authentication(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct Version {
    pub product: [u8; 2],
    pub firmware: [u8; 2],
    pub eeprom: [u8; 2],
}

#[derive(Debug, Clone)]
pub struct TypeATarget {
    pub uid: [u8; 4],
    pub atqa: [u8; 2],
    pub sak: u8,
}

pub struct Pn5180<SPI, BUSY, RESET> {
    spi: SPI,
    busy: BUSY,
    reset: RESET,
}

impl<SPI, BUSY, RESET> Pn5180<SPI, BUSY, RESET>
where
    SPI: SpiDevice<u8>,
    BUSY: InputPin,
    RESET: OutputPin,
{
    pub fn new(spi: SPI, busy: BUSY, reset: RESET) -> Self {
        Self { spi, busy, reset }
    }

    pub fn into_parts(self) -> (SPI, BUSY, RESET) {
        (self.spi, self.busy, self.reset)
    }

    pub async fn probe(&mut self) -> Result<Version, Error> {
        self.hardware_reset().await?;

        let mut product = [0; 2];
        let mut firmware = [0; 2];
        let mut eeprom = [0; 2];
        self.read_eeprom(PRODUCT_VERSION, &mut product).await?;
        self.read_eeprom(FIRMWARE_VERSION, &mut firmware).await?;
        self.read_eeprom(EEPROM_VERSION, &mut eeprom).await?;

        let invalid = |value: [u8; 2]| value == [0, 0] || value == [0xff, 0xff];
        if invalid(product) || invalid(firmware) || invalid(eeprom) {
            return Err(Error::InvalidResponse);
        }

        Ok(Version {
            product,
            firmware,
            eeprom,
        })
    }

    pub async fn initialize_iso_a(&mut self) -> Result<(), Error> {
        self.write_register(IRQ_CLEAR, u32::MAX).await?;
        self.load_rf_config(0x00, 0x80).await?;
        self.rf_on().await?;
        Timer::after_millis(20).await;
        Ok(())
    }

    pub async fn reset_and_initialize_iso_a(&mut self) -> Result<(), Error> {
        self.hardware_reset().await?;
        self.initialize_iso_a().await
    }

    pub async fn reacquire_type_a(&mut self) -> Result<Option<TypeATarget>, Error> {
        let _ = self.rf_off().await;
        Timer::after_millis(10).await;
        self.write_register(IRQ_CLEAR, u32::MAX).await?;
        self.load_rf_config(0x00, 0x80).await?;
        self.rf_on().await?;
        Timer::after_millis(20).await;
        self.activate_type_a(true).await
    }

    pub async fn activate_type_a(&mut self, wake_up: bool) -> Result<Option<TypeATarget>, Error> {
        self.write_register_and_mask(SYSTEM_CONFIG, 0xffff_ffbf).await?;
        self.write_register_and_mask(CRC_RX_CONFIG, 0xffff_fffe).await?;
        self.write_register_and_mask(CRC_TX_CONFIG, 0xffff_fffe).await?;
        self.write_register(IRQ_CLEAR, u32::MAX).await?;

        let request = if wake_up { 0x52 } else { 0x26 };
        self.send_data(&[request], 7).await?;
        if self.wait_for_rx(2, 2, Duration::from_millis(25)).await?.is_none() {
            return Ok(None);
        }

        let mut raw_atqa = [0; 2];
        self.read_data(&mut raw_atqa).await?;
        if raw_atqa == [0, 0] || raw_atqa == [0xff, 0xff] {
            return Ok(None);
        }

        self.write_register(IRQ_CLEAR, u32::MAX).await?;
        self.send_data(&[0x93, 0x20], 0).await?;
        if self.wait_for_rx(5, 5, Duration::from_millis(25)).await?.is_none() {
            return Err(Error::InvalidResponse);
        }

        let mut anticollision = [0; 5];
        self.read_data(&mut anticollision).await?;
        let bcc = anticollision[0] ^ anticollision[1] ^ anticollision[2] ^ anticollision[3];
        if bcc != anticollision[4] || anticollision[0] == 0x88 {
            return Err(Error::InvalidResponse);
        }

        self.write_register_or_mask(CRC_RX_CONFIG, 1).await?;
        self.write_register_or_mask(CRC_TX_CONFIG, 1).await?;
        self.write_register(IRQ_CLEAR, u32::MAX).await?;
        self.send_data(
            &[
                0x93,
                0x70,
                anticollision[0],
                anticollision[1],
                anticollision[2],
                anticollision[3],
                anticollision[4],
            ],
            0,
        )
        .await?;
        let Some(sak_length) = self.wait_for_rx(1, 3, Duration::from_millis(25)).await? else {
            return Err(Error::InvalidResponse);
        };

        let mut sak = [0; 3];
        self.read_data(&mut sak[..sak_length]).await?;
        if sak[0] & 0x80 != 0 {
            return Err(Error::InvalidResponse);
        }
        Ok(Some(TypeATarget {
            uid: [
                anticollision[0],
                anticollision[1],
                anticollision[2],
                anticollision[3],
            ],
            // ISO-A sends ATQA least-significant byte first. ReaderEvent uses
            // the conventional human-readable order (for example 0004).
            atqa: [raw_atqa[1], raw_atqa[0]],
            sak: sak[0],
        }))
    }

    pub async fn authenticate(&mut self, block: u8, key: &[u8; 6], uid: &[u8; 4]) -> Result<(), Error> {
        let command = [
            0x0c, key[0], key[1], key[2], key[3], key[4], key[5], 0x60, block, uid[0], uid[1], uid[2], uid[3],
        ];
        self.command(&command, Duration::from_secs(1)).await?;
        let mut response = [0xff];
        self.read_response(&mut response).await?;
        match response[0] {
            0 => Ok(()),
            status => Err(Error::Authentication(status)),
        }
    }

    pub async fn read_mifare_block(&mut self, block: u8, data: &mut [u8; 16]) -> Result<(), Error> {
        self.write_register(IRQ_CLEAR, u32::MAX).await?;
        self.write_register_or_mask(CRC_RX_CONFIG, 1).await?;
        self.write_register_or_mask(CRC_TX_CONFIG, 1).await?;
        self.send_data(&[0x30, block], 0).await?;
        if self.wait_for_rx(16, 16, Duration::from_millis(40)).await?.is_none() {
            return Err(Error::ResponseTimeout);
        }
        self.read_data(data).await
    }

    async fn hardware_reset(&mut self) -> Result<(), Error> {
        self.reset.set_low().map_err(|_| Error::Pin)?;
        Timer::after_millis(50).await;
        self.reset.set_high().map_err(|_| Error::Pin)?;
        Timer::after_millis(100).await;
        self.wait_busy_low(Duration::from_secs(2)).await?;
        Timer::after_millis(20).await;
        Ok(())
    }

    async fn load_rf_config(&mut self, tx: u8, rx: u8) -> Result<(), Error> {
        self.command(&[0x11, tx, rx], Duration::from_millis(100)).await?;
        Timer::after_millis(5).await;
        Ok(())
    }

    async fn rf_on(&mut self) -> Result<(), Error> {
        self.command(&[0x16, 0x00], Duration::from_millis(100)).await
    }

    async fn rf_off(&mut self) -> Result<(), Error> {
        self.command(&[0x17, 0x00], Duration::from_millis(100)).await
    }

    async fn set_transceive(&mut self) -> Result<(), Error> {
        self.write_register_and_mask(SYSTEM_CONFIG, 0xffff_fff8).await?;
        Timer::after_millis(1).await;
        self.write_register_or_mask(SYSTEM_CONFIG, 0x0000_0003).await?;

        let deadline = Instant::now() + Duration::from_millis(5);
        loop {
            let state = ((self.read_register(RF_STATUS).await? >> 24) & 0x07) as u8;
            if state == 1 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::InvalidResponse);
            }
            Timer::after_micros(100).await;
        }
    }

    async fn send_data(&mut self, data: &[u8], valid_bits: u8) -> Result<(), Error> {
        self.set_transceive().await?;
        let mut command = [0_u8; 16];
        let length = data.len() + 2;
        if length > command.len() {
            return Err(Error::InvalidResponse);
        }
        command[0] = 0x09;
        command[1] = valid_bits;
        command[2..length].copy_from_slice(data);
        self.command(&command[..length], Duration::from_millis(100)).await
    }

    async fn read_data(&mut self, data: &mut [u8]) -> Result<(), Error> {
        self.command(&[0x0a, 0x00], Duration::from_millis(100)).await?;
        self.read_response(data).await
    }

    async fn wait_for_rx(
        &mut self,
        minimum: usize,
        maximum: usize,
        timeout: Duration,
    ) -> Result<Option<usize>, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.read_register(IRQ_STATUS).await? & RX_IRQ != 0 {
                let status = self.read_register(RX_STATUS).await?;
                if status & RX_ERROR_MASK != 0 {
                    return Err(Error::ReceiveStatus(status));
                }
                let length = (status & 0x1ff) as usize;
                if (minimum..=maximum).contains(&length) {
                    return Ok(Some(length));
                }
                return Err(Error::InvalidResponse);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            Timer::after_millis(1).await;
        }
    }

    async fn read_eeprom(&mut self, address: u8, data: &mut [u8]) -> Result<(), Error> {
        if data.len() > u8::MAX as usize {
            return Err(Error::InvalidResponse);
        }
        self.command(&[0x07, address, data.len() as u8], Duration::from_millis(100))
            .await?;
        self.read_response(data).await
    }

    async fn write_register(&mut self, register: u8, value: u32) -> Result<(), Error> {
        self.register_command(0x00, register, value).await
    }

    async fn write_register_or_mask(&mut self, register: u8, value: u32) -> Result<(), Error> {
        self.register_command(0x01, register, value).await
    }

    async fn write_register_and_mask(&mut self, register: u8, value: u32) -> Result<(), Error> {
        self.register_command(0x02, register, value).await
    }

    async fn register_command(&mut self, command: u8, register: u8, value: u32) -> Result<(), Error> {
        let value = value.to_le_bytes();
        self.command(
            &[command, register, value[0], value[1], value[2], value[3]],
            Duration::from_millis(100),
        )
        .await
    }

    async fn read_register(&mut self, register: u8) -> Result<u32, Error> {
        self.command(&[0x04, register], Duration::from_millis(100)).await?;
        let mut response = [0xff; 4];
        self.read_response(&mut response).await?;
        Ok(u32::from_le_bytes(response))
    }

    async fn command(&mut self, command: &[u8], timeout: Duration) -> Result<(), Error> {
        self.wait_busy_low(Duration::from_millis(100)).await?;
        self.spi.write(command).await.map_err(|_| Error::Spi)?;
        Timer::after_micros(5).await;
        self.wait_busy_cycle(timeout).await
    }

    async fn read_response(&mut self, response: &mut [u8]) -> Result<(), Error> {
        response.fill(0xff);
        self.spi.transfer_in_place(response).await.map_err(|_| Error::Spi)?;
        Timer::after_micros(100).await;
        Ok(())
    }

    async fn wait_busy_cycle(&mut self, timeout: Duration) -> Result<(), Error> {
        let high_deadline = Instant::now() + Duration::from_millis(10);
        while !self.busy.is_high().map_err(|_| Error::Pin)? {
            if Instant::now() >= high_deadline {
                break;
            }
            Timer::after_micros(10).await;
        }
        self.wait_busy_low(timeout).await
    }

    async fn wait_busy_low(&mut self, timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        while self.busy.is_high().map_err(|_| Error::Pin)? {
            if Instant::now() >= deadline {
                return Err(Error::BusyTimeout);
            }
            Timer::after_micros(100).await;
        }
        Ok(())
    }
}
