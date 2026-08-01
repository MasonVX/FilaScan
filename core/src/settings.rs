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

// OTA is intentionally not exposed by FilaScan. These values only satisfy the
// generic hardware framework configuration until a FilaScan update path exists.
pub const OTA_DOMAIN: &str = "";
pub const OTA_PATH: &str = "";
pub const OTA_TOML_FILENAME: &str = "";
pub const OTA_TLS_CERTIFICATE: &str = "\0";
