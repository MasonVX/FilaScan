use alloc::{
    boxed::Box,
    ffi::CString,
    format,
    rc::Rc,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{
    cell::{Cell, RefCell},
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use edge_http::{Method, io::client::Connection};
use edge_nal_embassy::{Tcp, TcpBuffers};
use embassy_net::IpAddress;
use embedded_io_async::{Read, Write};
use esp_mbedtls::{Certificates, TlsVersion, X509};
use framework::{framework::Framework, utils::SpawnerHeapExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{bambu_spool::BambuSpool, diagnostics::LogBuffer};

// The SD card is mounted without long-file-name support. Keep every path
// component within the FAT 8.3 limits, including the three-character suffix.
const SETTINGS_PATH: &str = "/filascan/filaman/settings.jsn";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FilaManSettings {
    pub enabled: bool,
    pub base_url: String,
    pub device_token: String,
    pub ca_certificate_pem: String,
}

impl Default for FilaManSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            device_token: String::new(),
            ca_certificate_pem: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FilaManStatus {
    pub state: String,
    pub busy: bool,
}

#[derive(Clone, Debug)]
pub struct FilaManLocation {
    pub id: u64,
    pub name: String,
}

#[derive(Debug)]
pub enum SpoolRegistration {
    Existing {
        spool_id: u64,
        location_id: Option<u64>,
        location_name: Option<String>,
        locations: Vec<FilaManLocation>,
    },
    New {
        locations: Vec<FilaManLocation>,
    },
}

#[derive(Debug)]
pub struct ImportOutcome {
    pub status: String,
    pub spool_id: u64,
}

#[derive(Debug)]
pub struct MoveOutcome {
    pub spool_id: u64,
    pub location_id: u64,
    pub location_name: String,
}

#[derive(Debug)]
pub struct ArchiveOutcome {
    pub spool_id: u64,
}

struct ExistingSpool {
    id: u64,
    location_id: Option<u64>,
}

pub struct FilaManService {
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    settings: RefCell<FilaManSettings>,
    state: RefCell<String>,
    busy: Cell<bool>,
    sdcard_available: bool,
}

impl FilaManService {
    pub fn new(framework: Rc<RefCell<Framework>>, diagnostics: Rc<RefCell<LogBuffer>>, sdcard_available: bool) -> Rc<Self> {
        Rc::new(Self {
            framework,
            diagnostics,
            settings: RefCell::new(FilaManSettings::default()),
            state: RefCell::new("Not configured".to_string()),
            busy: Cell::new(false),
            sdcard_available,
        })
    }

    pub fn settings(&self) -> FilaManSettings {
        self.settings.borrow().clone()
    }

    pub fn status(&self) -> FilaManStatus {
        FilaManStatus {
            state: self.state.borrow().clone(),
            busy: self.busy.get(),
        }
    }

    pub fn import_enabled(&self) -> bool {
        let settings = self.settings.borrow();
        settings.enabled && !settings.device_token.is_empty()
    }

    pub async fn load_from_sd(&self) {
        if !self.sdcard_available {
            self.log_warn("FilaMan settings cannot be persisted because no SD card is installed");
            return;
        }
        let file_store = self.framework.borrow().file_store();
        let result = file_store.lock().await.read_file_bytes(SETTINGS_PATH).await;
        if let Ok(bytes) = result {
            match serde_json::from_slice::<FilaManSettings>(&bytes) {
                Ok(settings) if validate_settings(&settings).is_ok() => {
                    let configured = settings.enabled && !settings.device_token.is_empty();
                    *self.settings.borrow_mut() = settings;
                    *self.state.borrow_mut() = if configured { "Ready" } else { "Disabled" }.to_string();
                    self.log_info("FilaMan settings loaded from SD card");
                }
                _ => self.log_warn("Ignoring invalid cached FilaMan settings"),
            }
        }
    }

    pub fn set_settings(self: &Rc<Self>, settings: FilaManSettings) -> Result<(), String> {
        validate_settings(&settings)?;
        if settings.base_url.is_empty() {
            self.log_info("FilaMan settings cleared; location-assisted import disabled");
        } else {
            let endpoint = parse_base_url(&settings.base_url)?;
            self.log_info(&format!(
                "FilaMan settings accepted: {}://{}:{}{}; location-assisted import {}",
                if endpoint.secure { "https" } else { "http" },
                endpoint.host,
                endpoint.port,
                endpoint.base_path,
                if settings.enabled { "enabled" } else { "disabled" }
            ));
        }
        *self.settings.borrow_mut() = settings;
        *self.state.borrow_mut() = if self.settings.borrow().enabled { "Ready" } else { "Disabled" }.to_string();

        if !self.sdcard_available {
            return Err("FilaMan settings cannot be saved without an SD card".to_string());
        }
        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        spawner
            .spawn_heap(async move {
                if let Err(error) = service.persist_settings().await {
                    service.log_warn(&format!("Could not persist FilaMan settings: {error}"));
                }
            })
            .map_err(|_| "Could not start FilaMan settings save task".to_string())
    }

    pub fn request_test(self: &Rc<Self>) -> Result<(), String> {
        let endpoint = parse_base_url(&self.settings.borrow().base_url)?;
        if self.busy.replace(true) {
            return Err("A FilaMan request is already running".to_string());
        }
        self.log_info(&format!(
            "FilaMan: testing {}://{}:{}{}",
            if endpoint.secure { "https" } else { "http" },
            endpoint.host,
            endpoint.port,
            endpoint.base_path
        ));
        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        if spawner
            .spawn_heap(async move {
                let result = service.api_get("/devices/filascan/status").await;
                service.busy.set(false);
                match result {
                    Ok(response)
                        if response.get("status").and_then(Value::as_str) == Some("ready")
                            && response.get("location_selection").and_then(Value::as_bool) == Some(true)
                            && response.get("location_management").and_then(Value::as_bool) == Some(true)
                            && response.get("spool_archiving").and_then(Value::as_bool) == Some(true) =>
                    {
                        *service.state.borrow_mut() = "Connected".to_string();
                        service.log_info("FilaMan plugin connection test succeeded");
                    }
                    Ok(response) => {
                        let status = response.get("status").and_then(Value::as_str).unwrap_or("unknown");
                        let error = if status == "ready" {
                            "FilaScan import plugin does not support location selection, management, and spool archiving".to_string()
                        } else {
                            format!("FilaScan import plugin is not ready (status: {status})")
                        };
                        *service.state.borrow_mut() = format!("Connection failed: {error}");
                        service.log_warn(&format!("FilaMan connection test failed: {error}"));
                    }
                    Err(error) => {
                        *service.state.borrow_mut() = format!("Connection failed: {error}");
                        service.log_warn(&format!("FilaMan connection test failed: {error}"));
                    }
                }
            })
            .is_err()
        {
            self.busy.set(false);
            return Err("Could not start FilaMan connection test".to_string());
        }
        Ok(())
    }

    pub async fn prepare_spool(&self, spool: &BambuSpool) -> Result<SpoolRegistration, String> {
        if !self.import_enabled() {
            return Err("FilaMan location-assisted import is disabled".to_string());
        }
        if let Err(error) = validate_bambu_spool(spool) {
            return Err(error.to_string());
        }
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }

        self.log_info(&format!("FilaMan: checking registration and locations for Tray UID {}", spool.tray_uid));
        let result = self.prepare_spool_inner(spool).await;
        self.busy.set(false);
        match &result {
            Ok(SpoolRegistration::Existing { spool_id, location_name, .. }) => {
                *self.state.borrow_mut() = format!("Spool {spool_id} already registered");
                self.log_info(&format!(
                    "FilaMan: Tray UID {} is already registered as spool {} at {}",
                    spool.tray_uid,
                    spool_id,
                    location_name.as_deref().unwrap_or("no location")
                ));
            }
            Ok(SpoolRegistration::New { locations }) => {
                *self.state.borrow_mut() = format!("Choose one of {} locations", locations.len());
                self.log_info(&format!(
                    "FilaMan: Tray UID {} is new; {} eligible locations available",
                    spool.tray_uid,
                    locations.len()
                ));
            }
            Err(error) => {
                *self.state.borrow_mut() = format!("Lookup failed: {error}");
                self.log_warn(&format!("FilaMan lookup failed for Tray UID {}: {error}", spool.tray_uid));
            }
        }
        result
    }

    pub async fn import_spool_at(&self, spool: &BambuSpool, location_id: u64) -> Result<ImportOutcome, String> {
        if !self.import_enabled() {
            return Err("FilaMan location-assisted import is disabled".to_string());
        }
        if let Err(error) = validate_bambu_spool(spool) {
            return Err(error.to_string());
        }
        if location_id == 0 {
            return Err("FilaMan location is invalid".to_string());
        }
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }

        self.log_info(&format!("FilaMan: importing Tray UID {} into location {}", spool.tray_uid, location_id));
        let result = self.import_spool_inner(spool, location_id).await;
        self.busy.set(false);
        match &result {
            Ok(response) => {
                *self.state.borrow_mut() = format!("{} spool {}", import_status_label(&response.status), response.spool_id);
                self.log_info(&format!(
                    "FilaMan: plugin import {} spool {} for external_id {} at location {} (filament {}, manufacturer {}, colors {:?})",
                    response.status,
                    response.spool_id,
                    response.external_id,
                    location_id,
                    response.filament_id,
                    response.manufacturer_id,
                    response.color_ids
                ));
            }
            Err(error) => {
                *self.state.borrow_mut() = format!("Import failed: {error}");
                self.log_warn(&format!(
                    "FilaMan import failed for Tray UID {} at location {}: {error}",
                    spool.tray_uid, location_id
                ));
            }
        }
        result.map(|response| ImportOutcome {
            status: response.status,
            spool_id: response.spool_id,
        })
    }

    pub async fn move_spool(&self, spool_id: u64, location_id: u64) -> Result<MoveOutcome, String> {
        if !self.import_enabled() {
            return Err("FilaMan location-assisted import is disabled".to_string());
        }
        if spool_id == 0 || location_id == 0 {
            return Err("FilaMan spool or location is invalid".to_string());
        }
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }

        self.log_info(&format!("FilaMan: moving spool {} to location {}", spool_id, location_id));
        let result = self.move_spool_inner(spool_id, location_id).await;
        self.busy.set(false);
        match &result {
            Ok(response) => {
                *self.state.borrow_mut() = format!("Moved spool {}", response.spool_id);
                self.log_info(&format!(
                    "FilaMan: moved spool {} to location {} ({})",
                    response.spool_id, response.location_id, response.location_name
                ));
            }
            Err(error) => {
                *self.state.borrow_mut() = format!("Move failed: {error}");
                self.log_warn(&format!(
                    "FilaMan move failed for spool {} to location {}: {error}",
                    spool_id, location_id
                ));
            }
        }
        result.map(|response| MoveOutcome {
            spool_id: response.spool_id,
            location_id: response.location_id,
            location_name: response.location_name,
        })
    }

    pub async fn archive_spool(&self, spool_id: u64) -> Result<ArchiveOutcome, String> {
        if !self.import_enabled() {
            return Err("FilaMan location-assisted import is disabled".to_string());
        }
        if spool_id == 0 {
            return Err("FilaMan spool is invalid".to_string());
        }
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }

        self.log_info(&format!("FilaMan: archiving spool {spool_id}"));
        let result = self.archive_spool_inner(spool_id).await;
        self.busy.set(false);
        match &result {
            Ok(response) => {
                *self.state.borrow_mut() = format!("Archived spool {}", response.spool_id);
                self.log_info(&format!("FilaMan: archived spool {}", response.spool_id));
            }
            Err(error) => {
                *self.state.borrow_mut() = format!("Archive failed: {error}");
                self.log_warn(&format!("FilaMan archive failed for spool {spool_id}: {error}"));
            }
        }
        result.map(|response| ArchiveOutcome { spool_id: response.spool_id })
    }

    async fn persist_settings(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&*self.settings.borrow()).map_err(|error| format!("serialization failed: {error}"))?;
        let file_store = self.framework.borrow().file_store();
        file_store
            .lock()
            .await
            .create_write_file_bytes(SETTINGS_PATH, &bytes)
            .await
            .map_err(|error| format!("SD write failed: {error:?}"))
    }

    async fn prepare_spool_inner(&self, spool: &BambuSpool) -> Result<SpoolRegistration, String> {
        if let Some(existing) = self.find_spool(&spool.tray_uid).await? {
            let location_name = match existing.location_id {
                Some(location_id) => Some(self.load_location_name(location_id).await?),
                None => None,
            };
            return Ok(SpoolRegistration::Existing {
                spool_id: existing.id,
                location_id: existing.location_id,
                location_name,
                locations: self.load_locations().await?,
            });
        }
        Ok(SpoolRegistration::New {
            locations: self.load_locations().await?,
        })
    }

    async fn find_spool(&self, tray_uid: &str) -> Result<Option<ExistingSpool>, String> {
        let canonical_id = format!("bambulab:{}", tray_uid.to_ascii_uppercase());
        for page in 1..=100 {
            let response = self.api_get(&format!("/spools?page={page}&page_size=50&include_archived=false")).await?;
            let items = response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| "FilaMan spool list response is invalid".to_string())?;
            for item in items {
                let external_id = item.get("external_id").and_then(Value::as_str);
                if external_id
                    .map(|value| value.eq_ignore_ascii_case(&canonical_id) || value.eq_ignore_ascii_case(tray_uid))
                    .unwrap_or(false)
                {
                    let id = item
                        .get("id")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| "FilaMan spool list contains an item without a numeric id".to_string())?;
                    return Ok(Some(ExistingSpool {
                        id,
                        location_id: item.get("location_id").and_then(Value::as_u64),
                    }));
                }
            }
            let total = response.get("total").and_then(Value::as_u64).unwrap_or(items.len() as u64);
            if page as u64 * 50 >= total {
                return Ok(None);
            }
        }
        Err("FilaMan spool list exceeds 5000 entries".to_string())
    }

    async fn load_location_name(&self, location_id: u64) -> Result<String, String> {
        let response = self.api_get(&format!("/locations/{location_id}")).await?;
        response
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| "FilaMan location response has no name".to_string())
    }

    async fn load_locations(&self) -> Result<Vec<FilaManLocation>, String> {
        let mut locations = Vec::new();
        for page in 1..=20 {
            let response = self.api_get(&format!("/locations?page={page}&page_size=200")).await?;
            let items = response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| "FilaMan location list response is invalid".to_string())?;
            for item in items {
                if is_ineligible_location(item) {
                    continue;
                }
                let Some(id) = item.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if id > 0 && !name.trim().is_empty() {
                    locations.push(FilaManLocation {
                        id,
                        name: name.trim().to_string(),
                    });
                }
            }
            let total = response.get("total").and_then(Value::as_u64).unwrap_or(items.len() as u64);
            if page as u64 * 200 >= total {
                return Ok(locations);
            }
        }
        Err("FilaMan location list exceeds 4000 entries".to_string())
    }

    async fn import_spool_inner(&self, spool: &BambuSpool, location_id: u64) -> Result<ImportResponse, String> {
        let payload = json!({
            "external_id": spool.tray_uid,
            "manufacturer": "Bambu Lab",
            "material_id": spool.material_id,
            "variant_id": spool.variant_id,
            "filament_type": spool.filament_type,
            "detailed_filament_type": spool.detailed_filament_type,
            "official_material_name": spool.official_material_name,
            "color_name": spool.color_name,
            "bambu_color_code": optional_string(&spool.bambu_color_code),
            "primary_rgba": rgba_hex(spool.primary_rgba),
            "secondary_rgba": spool.secondary_rgba.map(rgba_hex),
            "location_id": location_id,
            "weight_g": spool.weight_g,
            "diameter_mm": spool.diameter_mm,
            "drying_temperature_c": optional_u16(spool.drying_temperature_c),
            "drying_time_h": optional_u16(spool.drying_time_h),
            "bed_temperature_c": optional_u16(spool.bed_temperature_c),
            "nozzle_temperature_min_c": optional_u16(spool.nozzle_temperature_min_c),
            "nozzle_temperature_max_c": optional_u16(spool.nozzle_temperature_max_c),
            "spool_width_mm": optional_f32(spool.spool_width_mm),
            "filament_length_m": optional_u16(spool.filament_length_m),
            "production_date": optional_string(&spool.production_date)
        });
        let response = self.api_post("/devices/filascan/import-spool?type=bambu", &payload).await?;
        let result: ImportResponse = serde_json::from_value(response).map_err(|error| format!("invalid FilaMan plugin import response: {error}"))?;
        if !matches!(result.status.as_str(), "created" | "existing" | "updated") {
            return Err(format!("FilaMan plugin returned unsupported import status '{}'", result.status));
        }
        let canonical_id = format!("bambulab:{}", spool.tray_uid.to_ascii_uppercase());
        if !result.external_id.eq_ignore_ascii_case(&canonical_id) && !result.external_id.eq_ignore_ascii_case(&spool.tray_uid) {
            return Err("FilaMan plugin response external_id does not match the requested Tray UID".to_string());
        }
        Ok(result)
    }

    async fn move_spool_inner(&self, spool_id: u64, location_id: u64) -> Result<MoveResponse, String> {
        let response = self
            .api_post(
                &format!("/devices/filascan/spools/{spool_id}/location"),
                &json!({ "location_id": location_id }),
            )
            .await?;
        let result: MoveResponse = serde_json::from_value(response).map_err(|error| format!("invalid FilaMan plugin move response: {error}"))?;
        if result.status != "moved" || result.spool_id != spool_id || result.location_id != location_id {
            return Err("FilaMan plugin move response does not match the request".to_string());
        }
        Ok(result)
    }

    async fn archive_spool_inner(&self, spool_id: u64) -> Result<ArchiveResponse, String> {
        let response = self.api_post(&format!("/devices/filascan/spools/{spool_id}/archive"), &json!({})).await?;
        let result: ArchiveResponse =
            serde_json::from_value(response).map_err(|error| format!("invalid FilaMan plugin archive response: {error}"))?;
        if result.status != "archived" || result.spool_id != spool_id {
            return Err("FilaMan plugin archive response does not match the request".to_string());
        }
        Ok(result)
    }

    async fn api_get(&self, path: &str) -> Result<Value, String> {
        self.api_request(Method::Get, path, None).await
    }

    async fn api_post(&self, path: &str, payload: &Value) -> Result<Value, String> {
        let body = serde_json::to_vec(payload).map_err(|error| format!("request serialization failed: {error}"))?;
        self.api_request(Method::Post, path, Some(&body)).await
    }

    async fn api_request(&self, method: Method, path: &str, body: Option<&[u8]>) -> Result<Value, String> {
        let settings = self.settings.borrow().clone();
        let endpoint = parse_base_url(&settings.base_url)?;
        let (stack, tls) = {
            let framework = self.framework.borrow();
            (framework.stack, framework.tls)
        };
        let address = match endpoint.host.parse::<Ipv4Addr>() {
            Ok(address) => address,
            Err(_) => {
                let ips = stack
                    .dns_query(&endpoint.host, embassy_net::dns::DnsQueryType::A)
                    .await
                    .map_err(|error| format!("DNS lookup failed: {error:?}"))?;
                match ips.first().copied() {
                    Some(IpAddress::Ipv4(address)) => address,
                    _ => return Err("DNS lookup returned no IPv4 address".to_string()),
                }
            }
        };
        let authorization = format!("Device {}", settings.device_token);
        let content_length = body.map(|value| value.len()).unwrap_or(0).to_string();
        let mut headers = vec![
            ("Host", endpoint.host.as_str()),
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("User-Agent", "FilaScan"),
            ("Connection", "close"),
        ];
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
            headers.push(("Content-Length", content_length.as_str()));
        }

        let api_path = format!("{}/api/v1{}", endpoint.base_path, path);
        let socket_address = SocketAddr::new(IpAddr::V4(address), endpoint.port);

        if endpoint.secure {
            let mut tcp_buffers = Box::new(TcpBuffers::<1, 2048, 8192>::new());
            let tcp = Tcp::new(stack, &mut *tcp_buffers);
            let ca_pem = CString::new(settings.ca_certificate_pem).map_err(|_| "FilaMan CA certificate contains a null byte".to_string())?;
            let ca_chain = X509::pem(ca_pem.as_bytes_with_nul()).map_err(|error| format!("Invalid FilaMan CA certificate: {error:?}"))?;
            let certificates = Certificates {
                ca_chain: Some(ca_chain),
                ..Default::default()
            };
            let server_name = CString::new(endpoint.host.as_str()).map_err(|_| "FilaMan host contains a null byte".to_string())?;
            let tls_connector = Box::new(esp_mbedtls::asynch::TlsConnector::new(
                tcp,
                &server_name,
                TlsVersion::Tls1_2,
                certificates,
                tls,
            ));
            let mut connection_buffer = Box::new([0_u8; 4096]);
            let mut connection: Box<Connection<_, 32>> = Box::new(Connection::new(&mut *connection_buffer, &*tls_connector, socket_address));
            connection
                .initiate_request(true, method, &api_path, &headers)
                .await
                .map_err(|error| format!("HTTPS request failed: {error:?}"))?;
            if let Some(body) = body {
                connection
                    .write_all(body)
                    .await
                    .map_err(|error| format!("HTTPS body write failed: {error:?}"))?;
            }
            connection
                .initiate_response()
                .await
                .map_err(|error| format!("HTTPS response failed: {error:?}"))?;
            let status = connection.headers().map_err(|error| format!("invalid HTTPS response: {error:?}"))?.code;
            let response = read_response(&mut connection).await?;
            decode_response(status, &response)
        } else {
            let mut tcp_buffers = Box::new(TcpBuffers::<1, 2048, 8192>::new());
            let tcp = Tcp::new(stack, &mut *tcp_buffers);
            let mut connection_buffer = Box::new([0_u8; 4096]);
            let mut connection: Box<Connection<_, 32>> = Box::new(Connection::new(&mut *connection_buffer, &tcp, socket_address));
            connection
                .initiate_request(true, method, &api_path, &headers)
                .await
                .map_err(|error| format!("HTTP request failed: {error:?}"))?;
            if let Some(body) = body {
                connection
                    .write_all(body)
                    .await
                    .map_err(|error| format!("HTTP body write failed: {error:?}"))?;
            }
            connection
                .initiate_response()
                .await
                .map_err(|error| format!("HTTP response failed: {error:?}"))?;
            let status = connection.headers().map_err(|error| format!("invalid HTTP response: {error:?}"))?.code;
            let response = read_response(&mut connection).await?;
            decode_response(status, &response)
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

#[derive(Debug, Deserialize)]
struct ImportResponse {
    status: String,
    spool_id: u64,
    filament_id: u64,
    manufacturer_id: u64,
    color_ids: Vec<u64>,
    external_id: String,
}

#[derive(Debug, Deserialize)]
struct MoveResponse {
    status: String,
    spool_id: u64,
    location_id: u64,
    location_name: String,
}

#[derive(Debug, Deserialize)]
struct ArchiveResponse {
    status: String,
    spool_id: u64,
}

struct Endpoint {
    host: String,
    port: u16,
    base_path: String,
    secure: bool,
}

fn validate_settings(settings: &FilaManSettings) -> Result<(), String> {
    if !settings.enabled && settings.base_url.is_empty() && settings.device_token.is_empty() && settings.ca_certificate_pem.is_empty() {
        return Ok(());
    }
    let endpoint = parse_base_url(&settings.base_url)?;
    if settings.device_token.len() > 512
        || settings
            .device_token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err("FilaMan device token contains invalid characters".to_string());
    }
    if settings.enabled && settings.device_token.is_empty() {
        return Err("FilaMan device token is required when location-assisted import is enabled".to_string());
    }
    if settings.enabled && endpoint.secure && !settings.ca_certificate_pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err("FilaMan CA certificate in PEM format is required when location-assisted import is enabled".to_string());
    }
    Ok(())
}

fn parse_base_url(url: &str) -> Result<Endpoint, String> {
    let (rest, secure) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else {
        return Err("FilaMan URL must start with http:// or https://".to_string());
    };
    let (authority, raw_path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.len() > 253 || authority.contains('@') {
        return Err("FilaMan URL has an invalid host".to_string());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => (host, port.parse::<u16>().map_err(|_| "FilaMan URL has an invalid port".to_string())?),
        _ => (authority, if secure { 443 } else { 80 }),
    };
    let base_path = if raw_path.is_empty() {
        String::new()
    } else {
        format!("/{}", raw_path.trim_end_matches('/'))
    };
    Ok(Endpoint {
        host: host.to_string(),
        port,
        base_path,
        secure,
    })
}

async fn read_response<T>(connection: &mut Connection<'_, T, 32>) -> Result<Vec<u8>, String>
where
    T: edge_nal::TcpConnect,
{
    let mut response = Vec::new();
    let mut chunk = [0_u8; 2048];
    loop {
        let length = connection
            .read(&mut chunk)
            .await
            .map_err(|error| format!("FilaMan response read failed: {error:?}"))?;
        if length == 0 {
            break;
        }
        if response.len() + length > MAX_RESPONSE_BYTES {
            return Err("FilaMan response exceeds 256 KiB".to_string());
        }
        response.extend_from_slice(&chunk[..length]);
    }
    Ok(response)
}

fn decode_response(status: u16, response: &[u8]) -> Result<Value, String> {
    if !(200..300).contains(&status) {
        let detail = core::str::from_utf8(response).unwrap_or("non-UTF-8 response");
        return Err(format!("FilaMan returned HTTP {status}: {}", truncate(detail, 240)));
    }
    serde_json::from_slice(response).map_err(|error| format!("invalid FilaMan JSON response: {error}"))
}

fn rgba_hex(rgba: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2], rgba[3])
}

fn validate_bambu_spool(spool: &BambuSpool) -> Result<(), &'static str> {
    if spool.tray_uid.len() != 32 || !spool.tray_uid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Bambu Tray UID is missing or invalid");
    }
    if spool.material_id.is_empty()
        || spool.variant_id.is_empty()
        || spool.filament_type.is_empty()
        || spool.detailed_filament_type.is_empty()
        || spool.official_material_name.is_empty()
        || spool.color_name.is_empty()
    {
        return Err("required decoded Bambu filament metadata is missing");
    }
    if spool.weight_g == 0 || !spool.diameter_mm.is_finite() || spool.diameter_mm <= 0.0 {
        return Err("required Bambu spool weight or diameter is invalid");
    }
    Ok(())
}

fn optional_u16(value: u16) -> Option<u16> {
    (value != 0).then_some(value)
}

fn optional_f32(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn optional_string(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn starts_with_bambulab(value: &str) -> bool {
    value
        .get(.."bambulab".len())
        .map(|prefix| prefix.eq_ignore_ascii_case("bambulab"))
        .unwrap_or(false)
}

fn is_ineligible_location(location: &Value) -> bool {
    let external_identifier = location
        .get("identifier")
        .or_else(|| location.get("rfuid"))
        .and_then(Value::as_str)
        .map(starts_with_bambulab)
        .unwrap_or(false);
    let driver_managed = location
        .get("custom_fields")
        .and_then(|fields| fields.get("managed_by"))
        .and_then(Value::as_str)
        .map(|value| value.to_ascii_lowercase().ends_with("_plugin"))
        .unwrap_or(false);
    external_identifier || driver_managed
}

fn import_status_label(status: &str) -> &'static str {
    match status {
        "created" => "Created",
        "existing" => "Found",
        "updated" => "Updated",
        _ => "Imported",
    }
}

fn truncate(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
