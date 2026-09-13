pub const AP_ADDR: (u8, u8, u8, u8) = (192, 168, 2, 1);

pub const WEB_SERVER_HTTPS: bool = false;
pub const WEB_SERVER_PORT: u16 = 80;
pub const WEB_SERVER_CAPTIVE: bool = true;
pub const WEB_SERVER_NUM_LISTENERS: usize = 2;
pub const WEB_SERVER_TLS_CERTIFICATE: &str = "\0";
pub const WEB_SERVER_TLS_PRIVATE_KEY: &str = "\0";

pub const WEB_APP_DOMAIN: &str = "filascan.local";
pub const WEB_APP_SECURITY_KEY_LENGTH: usize = 7;
pub const WEB_APP_SALT: &str = "example_salt";
pub const WEB_APP_KEY_DERIVATION_ITERATIONS: u32 = 10_000;

pub fn rfid_reader_mode() -> shared::reader::ReaderMode {
    match option_env!("FILASCAN_RFID_READER") {
        Some("pn532") => shared::reader::ReaderMode::Pn532,
        Some("pn5180") => shared::reader::ReaderMode::Pn5180,
        _ => shared::reader::ReaderMode::Auto,
    }
}

// Release firmware is published as direct HTTPS assets by GitHub Pages. The
// trailing NUL is required by the embedded X.509 parser.
pub const OTA_DOMAIN: &str = "masonvx.github.io";
pub const OTA_PATH: &str = "/FilaScan/";
pub const OTA_TOML_FILENAME: &str = "ota.toml";
pub const OTA_TLS_CERTIFICATE: &str = concat!(include_str!("certs/isrg-root-x1.pem"), "\0");
