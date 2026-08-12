use alloc::{
    boxed::Box,
    ffi::CString,
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    cell::{Cell, RefCell},
    net::{IpAddr, SocketAddr},
};

use edge_http::io::client::Connection;
use edge_nal_embassy::{Tcp, TcpBuffers};
use embassy_net::IpAddress;
use embassy_time::Timer;
use embedded_io_async::Read;
use esp_mbedtls::{Certificates, TlsVersion, X509};
use framework::{framework::Framework, utils::SpawnerHeapExt};
use serde::{Deserialize, Serialize};

use crate::diagnostics::LogBuffer;
use crate::localization::Language;

pub const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/bambulab/BambuStudio/master/resources/profiles/BBL/filament/filaments_color_codes.json";

// The SD card is mounted without long-file-name support. Keep writable names
// within FAT 8.3 limits; `.json` itself is already one character too long.
const CATALOG_PATH: &str = "/filascan/catalog/catalog.jsn";
const CATALOG_STAGING_PATH: &str = "/filascan/catalog/catalog.tmp";
const LEGACY_CATALOG_PATH: &str = "/filascan/catalog/filaments_color_codes.json";
const SETTINGS_PATH: &str = "/filascan/catalog/settings.jsn";
const GITHUB_ROOT_CA: &str = include_str!("certs/isrg-root-x1.pem");
const MAX_CATALOG_BYTES: usize = 384 * 1024;
const AUTO_UPDATE_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CatalogSettings {
    pub url: String,
    pub auto_update: bool,
}

impl Default for CatalogSettings {
    fn default() -> Self {
        Self {
            url: DEFAULT_CATALOG_URL.to_string(),
            auto_update: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CatalogStatus {
    pub entries: usize,
    pub state: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct CatalogMatch {
    pub filament_type: String,
    color_name_en: String,
    color_name_de: String,
    pub product_code: String,
}

impl CatalogMatch {
    pub fn color_name(&self, language: Language) -> String {
        if language == Language::German && !self.color_name_de.is_empty() {
            self.color_name_de.clone()
        } else {
            self.color_name_en.clone()
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn lookup(&self, material_id: &str, variant_id: &str, colors: &str) -> Option<CatalogMatch> {
        let variant_color_code = variant_id.rsplit_once('-').map(|(_, code)| code);
        let entry = variant_color_code
            .and_then(|code| {
                self.entries
                    .iter()
                    .find(|entry| entry.material_id == material_id && entry.variant_color_code == code)
            })
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| entry.material_id == material_id && entry.colors == colors)
            })?;

        Some(CatalogMatch {
            filament_type: entry.filament_type.clone(),
            color_name_en: entry.color_name_en.clone(),
            color_name_de: entry.color_name_de.clone(),
            product_code: entry.product_code.clone(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone, Debug)]
struct CatalogEntry {
    material_id: String,
    filament_type: String,
    color_name_en: String,
    color_name_de: String,
    product_code: String,
    variant_color_code: String,
    colors: String,
}

#[derive(Deserialize)]
struct UpstreamCatalog {
    data: Vec<UpstreamEntry>,
}

#[derive(Deserialize)]
struct UpstreamEntry {
    fila_color_code: String,
    fila_id: String,
    fila_type: String,
    fila_color_name: UpstreamNames,
    color_code: String,
    fila_color: Vec<String>,
}

#[derive(Deserialize)]
struct UpstreamNames {
    en: String,
    #[serde(default)]
    de: String,
}

pub struct CatalogService {
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    catalog: Rc<RefCell<Catalog>>,
    settings: RefCell<CatalogSettings>,
    status: RefCell<CatalogStatus>,
    updating: Cell<bool>,
    sdcard_available: bool,
}

impl CatalogService {
    pub fn new(framework: Rc<RefCell<Framework>>, diagnostics: Rc<RefCell<LogBuffer>>, sdcard_available: bool) -> Rc<Self> {
        Rc::new(Self {
            framework,
            diagnostics,
            catalog: Rc::new(RefCell::new(Catalog::default())),
            settings: RefCell::new(CatalogSettings::default()),
            status: RefCell::new(CatalogStatus {
                entries: 0,
                state: "No cached catalog".to_string(),
                source: String::new(),
            }),
            updating: Cell::new(false),
            sdcard_available,
        })
    }

    pub fn catalog(&self) -> Rc<RefCell<Catalog>> {
        self.catalog.clone()
    }

    pub fn settings(&self) -> CatalogSettings {
        self.settings.borrow().clone()
    }

    pub fn status(&self) -> CatalogStatus {
        self.status.borrow().clone()
    }

    pub fn is_updating(&self) -> bool {
        self.updating.get()
    }

    pub async fn load_from_sd(&self) {
        if !self.sdcard_available {
            self.log_warn("Bambu catalog cache unavailable because no SD card is installed");
            return;
        }

        let file_store = self.framework.borrow().file_store();
        if let Ok(bytes) = file_store.lock().await.read_file_bytes(SETTINGS_PATH).await {
            match serde_json::from_slice::<CatalogSettings>(&bytes) {
                Ok(settings) if validate_catalog_url(&settings.url).is_ok() => {
                    *self.settings.borrow_mut() = settings;
                }
                _ => self.log_warn("Ignoring invalid cached Bambu catalog settings"),
            }
        }

        for path in [CATALOG_STAGING_PATH, CATALOG_PATH, LEGACY_CATALOG_PATH] {
            let Ok(bytes) = file_store.lock().await.read_file_bytes(path).await else {
                continue;
            };
            match parse_catalog(&bytes) {
                Ok(catalog) => {
                    let entries = catalog.len();
                    *self.catalog.borrow_mut() = catalog;
                    *self.status.borrow_mut() = CatalogStatus {
                        entries,
                        state: "Ready".to_string(),
                        source: "SD cache".to_string(),
                    };
                    self.log_info(&format!("Bambu catalog loaded from SD cache: {entries} entries"));
                    return;
                }
                Err(error) => self.log_warn(&format!("Ignoring invalid Bambu catalog cache {path}: {error}")),
            }
        }

        self.log_info("No valid Bambu catalog found on the SD card");
    }

    pub fn set_settings(self: &Rc<Self>, settings: CatalogSettings) -> Result<(), String> {
        validate_catalog_url(&settings.url)?;
        *self.settings.borrow_mut() = settings;
        if !self.sdcard_available {
            self.log_warn("Bambu catalog settings changed for this session but cannot be persisted without an SD card");
            return Ok(());
        }

        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        spawner
            .spawn_heap(async move {
                if let Err(error) = service.persist_settings().await {
                    service.log_warn(&format!("Could not persist Bambu catalog settings: {error}"));
                }
            })
            .map_err(|_| "Could not start catalog settings save task".to_string())?;
        Ok(())
    }

    pub fn request_update(self: &Rc<Self>) -> Result<(), String> {
        if self.updating.replace(true) {
            return Err("A catalog update is already running".to_string());
        }
        if self.framework.borrow().wifi_ok != Some(true) {
            self.updating.set(false);
            return Err("Wi-Fi is not connected".to_string());
        }

        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        if spawner
            .spawn_heap(async move {
                service.update().await;
                service.updating.set(false);
            })
            .is_err()
        {
            self.updating.set(false);
            return Err("Could not start catalog update task".to_string());
        }
        Ok(())
    }

    pub fn start_periodic_updates(self: &Rc<Self>) -> Result<(), String> {
        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        spawner
            .spawn_heap(async move {
                Timer::after_secs(10).await;
                loop {
                    if service.settings.borrow().auto_update {
                        let _ = service.request_update();
                    }
                    Timer::after_secs(AUTO_UPDATE_INTERVAL_SECONDS).await;
                }
            })
            .map_err(|_| "Could not start automatic catalog update task".to_string())
    }

    async fn persist_settings(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&*self.settings.borrow()).map_err(|error| format!("settings serialization failed: {error}"))?;
        let file_store = self.framework.borrow().file_store();
        file_store
            .lock()
            .await
            .create_write_file_bytes(SETTINGS_PATH, &bytes)
            .await
            .map_err(|error| format!("SD write failed: {error:?}"))
    }

    async fn update(&self) {
        let url = self.settings.borrow().url.clone();
        self.status.borrow_mut().state = "Downloading".to_string();
        self.log_info(&format!("Downloading Bambu catalog from {url}"));

        let result = async {
            let bytes = download_catalog(self.framework.clone(), &url).await?;
            let catalog = parse_catalog(&bytes)?;
            let entries = catalog.len();

            if self.sdcard_available {
                let file_store = self.framework.borrow().file_store();
                file_store
                    .lock()
                    .await
                    .create_write_file_bytes(CATALOG_STAGING_PATH, &bytes)
                    .await
                    .map_err(|error| format!("SD staging write failed: {error:?}"))?;
                file_store
                    .lock()
                    .await
                    .create_write_file_bytes(CATALOG_PATH, &bytes)
                    .await
                    .map_err(|error| format!("SD catalog write failed: {error:?}"))?;
            }

            *self.catalog.borrow_mut() = catalog;
            Ok::<usize, String>(entries)
        }
        .await;

        match result {
            Ok(entries) => {
                *self.status.borrow_mut() = CatalogStatus {
                    entries,
                    state: "Ready".to_string(),
                    source: if self.sdcard_available {
                        "GitHub + SD cache"
                    } else {
                        "GitHub (memory only)"
                    }
                    .to_string(),
                };
                self.log_info(&format!("Bambu catalog updated successfully: {entries} entries"));
            }
            Err(error) => {
                self.status.borrow_mut().state = format!("Update failed: {error}");
                self.log_warn(&format!("Bambu catalog update failed: {error}"));
            }
        }
    }

    fn log_info(&self, message: &str) {
        log::info!("{}", message);
        self.diagnostics.borrow_mut().info(message);
    }

    fn log_warn(&self, message: &str) {
        log::warn!("{}", message);
        self.diagnostics.borrow_mut().warn(message);
    }
}

fn parse_catalog(bytes: &[u8]) -> Result<Catalog, String> {
    if bytes.is_empty() || bytes.len() > MAX_CATALOG_BYTES {
        return Err(format!("catalog size {} is outside the allowed range", bytes.len()));
    }
    let upstream: UpstreamCatalog = serde_json::from_slice(bytes).map_err(|error| format!("invalid JSON: {error}"))?;
    if upstream.data.is_empty() || upstream.data.len() > 1_000 {
        return Err(format!("unexpected entry count {}", upstream.data.len()));
    }

    let mut entries = Vec::with_capacity(upstream.data.len());
    for item in upstream.data {
        if item.fila_id.is_empty()
            || item.fila_type.is_empty()
            || item.fila_color_name.en.is_empty()
            || item.color_code.is_empty()
            || item.fila_color_code.len() != 5
            || !item.fila_color_code.bytes().all(|byte| byte.is_ascii_digit())
            || item.fila_color.is_empty()
            || item.fila_color.len() > 4
        {
            return Err("catalog contains an incomplete entry".to_string());
        }

        let mut normalized_colors = Vec::with_capacity(item.fila_color.len());
        for color in item.fila_color {
            normalized_colors.push(normalize_rgba(&color)?);
        }
        let colors = normalized_colors.join("/");
        if entries
            .iter()
            .any(|entry: &CatalogEntry| entry.material_id == item.fila_id && entry.variant_color_code == item.color_code)
        {
            return Err(format!("duplicate material/color key {} + {}", item.fila_id, item.color_code));
        }

        entries.push(CatalogEntry {
            material_id: item.fila_id,
            filament_type: item.fila_type,
            color_name_en: item.fila_color_name.en,
            color_name_de: item.fila_color_name.de,
            product_code: item.fila_color_code,
            variant_color_code: item.color_code,
            colors,
        });
    }
    Ok(Catalog { entries })
}

fn normalize_rgba(value: &str) -> Result<String, String> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid RGBA value {value}"));
    }
    Ok(value.to_ascii_uppercase())
}

fn validate_catalog_url(url: &str) -> Result<(&str, &str), String> {
    if url.len() > 384 {
        return Err("Catalog URL is too long".to_string());
    }
    let rest = url.strip_prefix("https://").ok_or_else(|| "Catalog URL must use HTTPS".to_string())?;
    let (host, path) = rest.split_once('/').ok_or_else(|| "Catalog URL has no path".to_string())?;
    if host != "raw.githubusercontent.com" {
        return Err("Catalog URL must use raw.githubusercontent.com".to_string());
    }
    if !path.ends_with(".json") || path.contains('?') || path.contains('#') {
        return Err("Catalog URL must point directly to a JSON file".to_string());
    }
    Ok((host, path))
}

async fn download_catalog(framework: Rc<RefCell<Framework>>, url: &str) -> Result<Vec<u8>, String> {
    let (host, path_without_slash) = validate_catalog_url(url)?;
    let (stack, tls) = {
        let framework = framework.borrow();
        (framework.stack, framework.tls)
    };
    let ips = stack
        .dns_query(host, embassy_net::dns::DnsQueryType::A)
        .await
        .map_err(|error| format!("DNS lookup failed: {error:?}"))?;
    let Some(IpAddress::Ipv4(address)) = ips.first().copied() else {
        return Err("DNS lookup returned no IPv4 address".to_string());
    };

    let ca_pem = CString::new(GITHUB_ROOT_CA).map_err(|_| "Embedded GitHub root certificate contains a null byte".to_string())?;
    let ca_chain = X509::pem(ca_pem.as_bytes_with_nul()).map_err(|error| format!("Invalid embedded GitHub root certificate: {error:?}"))?;
    let certificates = Certificates {
        ca_chain: Some(ca_chain),
        ..Default::default()
    };
    let mut tcp_buffers = Box::new(TcpBuffers::<1, 1024, 8192>::new());
    let tcp = Tcp::new(stack, &mut *tcp_buffers);
    let server_name = CString::new(host).map_err(|_| "Invalid GitHub host".to_string())?;
    let tls_connector = Box::new(esp_mbedtls::asynch::TlsConnector::new(
        tcp,
        &server_name,
        TlsVersion::Tls1_2,
        certificates,
        tls,
    ));

    let mut connection_buffer = Box::new([0_u8; 4096]);
    let mut connection: Box<Connection<_, 32>> = Box::new(Connection::new(
        &mut *connection_buffer,
        &*tls_connector,
        SocketAddr::new(IpAddr::V4(address), 443),
    ));
    let request_path = format!("/{path_without_slash}");
    connection
        .initiate_request(
            true,
            edge_http::Method::Get,
            &request_path,
            &[("Host", host), ("Accept", "application/json"), ("User-Agent", "FilaScan")],
        )
        .await
        .map_err(|error| format!("HTTPS request failed: {error:?}"))?;
    connection
        .initiate_response()
        .await
        .map_err(|error| format!("HTTPS response failed: {error:?}"))?;

    let status = connection.headers().map_err(|error| format!("Invalid HTTP response: {error:?}"))?.code;
    if status != 200 {
        return Err(format!("GitHub returned HTTP {status}"));
    }

    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let length = connection
            .read(&mut chunk)
            .await
            .map_err(|error| format!("Catalog download failed: {error:?}"))?;
        if length == 0 {
            break;
        }
        if bytes.len() + length > MAX_CATALOG_BYTES {
            return Err("Bambu catalog exceeds the 384 KiB safety limit".to_string());
        }
        bytes.extend_from_slice(&chunk[..length]);
    }
    Ok(bytes)
}
