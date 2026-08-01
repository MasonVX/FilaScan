use shared::settings::OTA_DOMAIN_STABLE;

pub const AP_ADDR: (u8, u8, u8, u8) = (192, 168, 2, 1);


pub const WEB_SERVER_HTTPS: bool = false; // Don't forget to set also port below
pub const WEB_SERVER_PORT: u16 = 80; // For HTTPS use 443 normally, for HTTP 80, but either can be any other port number
pub const WEB_SERVER_CAPTIVE: bool = true;
pub const WEB_SERVER_NUM_LISTENERS: usize = 5;
// HTTPS is disabled above. Never ship the shared upstream development key.
// Generate a device-specific certificate before enabling HTTPS.
pub const WEB_SERVER_TLS_CERTIFICATE: &str = "\0";
pub const WEB_SERVER_TLS_PRIVATE_KEY: &str = "\0";

pub const WEB_APP_DOMAIN: &str = "device.spoolease.io";
pub const WEB_APP_SECURITY_KEY_LENGTH: usize = 7;
pub const WEB_APP_SALT: &str = "example_salt"; // to be aligned with WASM & Captive HTML
pub const WEB_APP_KEY_DERIVATION_ITERATIONS: u32 = 10_000; // to be aligned with WASM & Captive HTML

pub const MAX_NUM_PRINTERS: usize = 5;

/// This fork is deliberately a filament reader, not a printer/AMS manager.
/// Existing printer credentials can remain in flash, but no MQTT connection is
/// opened while this is enabled.
pub const READER_ONLY_MODE: bool = true;

// Framework basic OTA (from web-config)
pub const OTA_DOMAIN: &str = OTA_DOMAIN_STABLE;
pub const OTA_PATH: &str = CONSOLE_STABLE_OTA_PATH;

pub const OTA_TOML_FILENAME: &str = "ota.toml";
// pub const OTA_TLS_CERTIFICATE: &str = concat!(include_str!("./certs/raw.githubusercontent.com.pem"), "\0");
pub const CONSOLE_STABLE_OTA_PATH: &str = "/yanshay/SpoolEase/main/build/bins/0.6/console/ota/";
pub const CONSOLE_UNSTABLE_OTA_PATH: &str = "/yanshay/SpoolEase/main/build/bins/0.6/console/ota-unstable/";
pub const CONSOLE_DEBUG_OTA_PATH: &str = "/yanshay/SpoolEase/main/build/bins/0.6/console/debug/";
