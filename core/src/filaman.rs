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
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::{Read, Write};
use esp_mbedtls::{Certificates, TlsVersion, X509};
use framework::{framework::Framework, utils::SpawnerHeapExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    diagnostics::LogBuffer,
    spool::{FilamentSpool, ProductReference, SpoolSource},
};

mod offline;

use offline::{CachedSpool, OfflineState, PreparedQueue};

// The SD card is mounted without long-file-name support. Keep every path
// component within the FAT 8.3 limits, including the three-character suffix.
const SETTINGS_PATH: &str = "/filascan/filaman/settings.jsn";
const INVENTORY_PATH: &str = "/filascan/filaman/invent.jsn";
const QUEUE_PATH: &str = "/filascan/filaman/queue.jsn";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const HEARTBEAT_INITIAL_DELAY: Duration = Duration::from_secs(15);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);
const INVENTORY_REFRESH_HEARTBEATS: u16 = 15;

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
    pub registered: bool,
    pub device_id: Option<u64>,
    pub device_name: Option<String>,
    pub offline: bool,
    pub cached_spools: usize,
    pub cached_locations: usize,
    pub pending_operations: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
        offline: bool,
    },
    New {
        locations: Vec<FilaManLocation>,
        offline: bool,
        inventory_known: bool,
    },
    Pending {
        location_id: u64,
        location_name: String,
        locations: Vec<FilaManLocation>,
    },
}

#[derive(Debug)]
pub struct ArchiveOutcome {
    pub spool_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExistingSpool {
    id: u64,
    location_id: Option<u64>,
}

#[derive(Debug)]
pub enum StorageOutcome {
    Applied {
        spool_id: u64,
        location_id: u64,
        location_name: String,
    },
    Queued {
        location_id: u64,
        location_name: String,
    },
}

pub struct FilaManService {
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    settings: RefCell<FilaManSettings>,
    state: RefCell<String>,
    device_name: RefCell<Option<String>>,
    busy: Cell<bool>,
    heartbeat_succeeded: Cell<bool>,
    heartbeat_failure_active: Cell<bool>,
    offline: OfflineState,
    inventory_refresh_ticks: Cell<u16>,
    sdcard_available: bool,
}

impl FilaManService {
    pub fn new(framework: Rc<RefCell<Framework>>, diagnostics: Rc<RefCell<LogBuffer>>, sdcard_available: bool) -> Rc<Self> {
        Rc::new(Self {
            framework,
            diagnostics,
            settings: RefCell::new(FilaManSettings::default()),
            state: RefCell::new("Not configured".to_string()),
            device_name: RefCell::new(None),
            busy: Cell::new(false),
            heartbeat_succeeded: Cell::new(false),
            heartbeat_failure_active: Cell::new(false),
            offline: OfflineState::new(),
            inventory_refresh_ticks: Cell::new(0),
            sdcard_available,
        })
    }

    pub fn settings(&self) -> FilaManSettings {
        self.settings.borrow().clone()
    }

    pub fn status(&self) -> FilaManStatus {
        let settings = self.settings.borrow();
        let device_id = device_id_from_token(&settings.device_token);
        let offline = self.offline.status();
        FilaManStatus {
            state: self.state.borrow().clone(),
            busy: self.busy.get(),
            registered: device_id.is_some(),
            device_id,
            device_name: self.device_name.borrow().clone(),
            offline: self.import_enabled() && self.use_offline_inventory(),
            cached_spools: offline.cached_spools,
            cached_locations: offline.cached_locations,
            pending_operations: offline.pending_operations,
        }
    }

    pub fn import_enabled(&self) -> bool {
        let settings = self.settings.borrow();
        settings.enabled && !settings.device_token.is_empty()
    }

    fn use_offline_inventory(&self) -> bool {
        self.local_ipv4_address().is_none() || !self.heartbeat_succeeded.get() || self.heartbeat_failure_active.get()
    }

    pub async fn load_from_sd(&self) {
        if !self.sdcard_available {
            self.log_warn("FilaMan offline data cannot be persisted because no SD card is installed");
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

        if let Ok(bytes) = file_store.lock().await.read_file_bytes(INVENTORY_PATH).await {
            match self.offline.load_inventory(bytes) {
                Ok(status) => self.log_info(&format!(
                    "FilaMan offline inventory loaded from SD card: {} spools, {} locations",
                    status.cached_spools, status.cached_locations
                )),
                _ => self.log_warn("Ignoring invalid cached FilaMan inventory"),
            }
        }

        if let Ok(bytes) = file_store.lock().await.read_file_bytes(QUEUE_PATH).await {
            match self.offline.load_queue(bytes) {
                Ok(count) => {
                    if count > 0 {
                        self.log_info(&format!("FilaMan offline queue loaded from SD card: {count} pending operations"));
                    }
                }
                _ => self.log_warn("Ignoring invalid FilaMan offline queue"),
            }
        }
    }

    pub fn start_heartbeat(self: &Rc<Self>) -> Result<(), String> {
        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        spawner
            .spawn_heap(async move {
                Timer::after(HEARTBEAT_INITIAL_DELAY).await;
                loop {
                    service.send_heartbeat_if_ready().await;
                    Timer::after(HEARTBEAT_INTERVAL).await;
                }
            })
            .map_err(|_| "Could not start FilaMan heartbeat task".to_string())
    }

    async fn send_heartbeat_if_ready(&self) {
        let settings = self.settings.borrow().clone();
        if settings.base_url.is_empty() || device_id_from_token(&settings.device_token).is_none() {
            return;
        }
        let Some(ip_address) = self.local_ipv4_address() else {
            return;
        };
        if self.busy.replace(true) {
            return;
        }

        let result = with_timeout(
            HEARTBEAT_TIMEOUT,
            self.api_post("/devices/heartbeat", &json!({ "ip_address": ip_address })),
        )
        .await;
        self.busy.set(false);

        let error = match result {
            Ok(Ok(response)) if response.get("status").and_then(Value::as_str) == Some("ok") => {
                let recovered = self.heartbeat_failure_active.replace(false);
                let first_success = !self.heartbeat_succeeded.replace(true);
                if first_success {
                    self.log_info(&format!("FilaMan heartbeat active; reporting local IP {ip_address}"));
                } else if recovered {
                    self.log_info(&format!("FilaMan heartbeat recovered; reporting local IP {ip_address}"));
                }
                self.run_online_maintenance(first_success || recovered).await;
                return;
            }
            Ok(Ok(_)) => "FilaMan returned an invalid heartbeat response".to_string(),
            Ok(Err(error)) => error,
            Err(_) => "request timed out after 20 seconds".to_string(),
        };

        if !self.heartbeat_failure_active.replace(true) {
            self.log_warn(&format!("FilaMan heartbeat failed: {error}"));
        }
    }

    async fn run_online_maintenance(&self, force_inventory_refresh: bool) {
        let queue_changed = self.synchronize_offline_queue().await;
        let ticks = self.inventory_refresh_ticks.get().saturating_add(1);
        let refresh_due = force_inventory_refresh || queue_changed || ticks >= INVENTORY_REFRESH_HEARTBEATS;
        if refresh_due {
            self.inventory_refresh_ticks.set(0);
            if let Err(error) = self.refresh_inventory().await {
                self.log_warn(&format!("FilaMan inventory refresh failed: {error}"));
            }
        } else {
            self.inventory_refresh_ticks.set(ticks);
        }
    }

    async fn refresh_inventory(&self) -> Result<(), String> {
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }
        let result = async {
            let locations = self.load_locations().await?;
            let spools = self.load_inventory_spools().await?;
            let prepared = self.offline.prepare_inventory(locations, spools)?;
            let changed = prepared.changed;
            if changed && self.sdcard_available {
                let file_store = self.framework.borrow().file_store();
                file_store
                    .lock()
                    .await
                    .create_write_file_bytes(INVENTORY_PATH, &prepared.serialized)
                    .await
                    .map_err(|error| format!("SD write failed: {error:?}"))?;
            }
            let spool_count = prepared.spool_count();
            let location_count = prepared.location_count();
            self.offline.commit_inventory(prepared);
            if changed {
                self.log_info(&format!(
                    "FilaMan inventory refreshed: {spool_count} spools, {location_count} locations; snapshot {}",
                    if self.sdcard_available { "written to SD card" } else { "kept in RAM" }
                ));
            } else {
                self.log_info(&format!(
                    "FilaMan inventory unchanged: {spool_count} spools, {location_count} locations; SD write skipped"
                ));
            }
            Ok(())
        }
        .await;
        self.busy.set(false);
        result
    }

    async fn synchronize_offline_queue(&self) -> bool {
        let operations = self.offline.operations();
        if operations.is_empty() || self.busy.replace(true) {
            return false;
        }

        self.log_info(&format!("Synchronizing {} queued FilaMan operations", operations.len()));
        let mut remaining = Vec::new();
        let mut completed = 0usize;
        let mut stop_after_failure = false;
        for operation in operations {
            if stop_after_failure {
                remaining.push(operation);
                continue;
            }
            match self
                .apply_storage_online(&operation.spool, operation.location_id, &operation.location_name)
                .await
            {
                Ok(spool_id) => {
                    completed += 1;
                    self.log_info(&format!(
                        "FilaMan offline operation {} synchronized as spool {spool_id} at {}",
                        operation.sequence, operation.location_name
                    ));
                }
                Err(error) => {
                    stop_after_failure = is_transport_error(&error);
                    self.log_warn(&format!("FilaMan offline operation {} remains queued: {error}", operation.sequence));
                    remaining.push(operation);
                }
            }
        }
        if completed == 0 {
            self.busy.set(false);
            return false;
        }
        let prepared = match self.offline.prepare_remaining(remaining) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.busy.set(false);
                self.log_warn(&format!("Could not prepare synchronized FilaMan queue: {error}"));
                return false;
            }
        };
        let result = match self.persist_queue(prepared).await {
            Ok(true) => true,
            Ok(false) => false,
            Err(error) => {
                self.log_warn(&format!("Could not persist synchronized FilaMan queue: {error}"));
                false
            }
        };
        self.busy.set(false);
        result
    }

    fn local_ipv4_address(&self) -> Option<String> {
        let stack = self.framework.borrow().stack;
        if !stack.is_link_up() {
            return None;
        }
        stack.config_v4().map(|config| config.address.address().to_string())
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

    pub fn update_connection_settings(self: &Rc<Self>, enabled: bool, base_url: String, ca_certificate_pem: String) -> Result<(), String> {
        let device_token = self.settings.borrow().device_token.clone();
        self.set_settings(FilaManSettings {
            enabled,
            base_url,
            device_token,
            ca_certificate_pem,
        })
    }

    pub fn forget_registration(self: &Rc<Self>) -> Result<(), String> {
        let current = self.settings.borrow().clone();
        self.set_settings(FilaManSettings {
            enabled: false,
            base_url: current.base_url,
            device_token: String::new(),
            ca_certificate_pem: current.ca_certificate_pem,
        })?;
        *self.device_name.borrow_mut() = None;
        *self.state.borrow_mut() = "Device registration removed from FilaScan".to_string();
        self.log_info("FilaMan device token removed from FilaScan");
        Ok(())
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
                        *service.device_name.borrow_mut() = response
                            .get("device_name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.trim().is_empty())
                            .map(|name| name.trim().to_string());
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

    pub fn request_registration(
        self: &Rc<Self>,
        base_url: String,
        device_code: String,
        ca_certificate_pem: String,
        enabled: bool,
    ) -> Result<(), String> {
        if !self.sdcard_available {
            return Err("FilaMan device registration requires an SD card to store the issued token".to_string());
        }

        let device_code = device_code.trim().to_ascii_uppercase();
        validate_device_code(&device_code)?;
        let registration_settings = FilaManSettings {
            enabled: false,
            base_url,
            device_token: String::new(),
            ca_certificate_pem,
        };
        validate_registration_settings(&registration_settings)?;
        let endpoint = parse_base_url(&registration_settings.base_url)?;

        if self.busy.replace(true) {
            return Err("A FilaMan request is already running".to_string());
        }
        self.log_info(&format!(
            "FilaMan: registering device at {}://{}:{}{}",
            if endpoint.secure { "https" } else { "http" },
            endpoint.host,
            endpoint.port,
            endpoint.base_path
        ));
        *self.state.borrow_mut() = "Registering device".to_string();

        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        if spawner
            .spawn_heap(async move {
                let result = service.register_device(&registration_settings, &device_code).await;
                match result {
                    Ok(device_token) => {
                        let settings = FilaManSettings {
                            enabled,
                            base_url: registration_settings.base_url,
                            device_token,
                            ca_certificate_pem: registration_settings.ca_certificate_pem,
                        };
                        *service.settings.borrow_mut() = settings;
                        *service.device_name.borrow_mut() = None;
                        match service.persist_settings().await {
                            Ok(()) => {
                                *service.state.borrow_mut() = "Device registered; authorize it in the FilaScan import plugin".to_string();
                                service.log_info("FilaMan device registration succeeded; device token stored on SD card");
                            }
                            Err(error) => {
                                *service.state.borrow_mut() = "Device registered, but the token could not be stored".to_string();
                                service.log_warn(&format!("FilaMan issued a device token, but it could not be persisted: {error}"));
                            }
                        }
                    }
                    Err(error) => {
                        *service.state.borrow_mut() = format!("Registration failed: {error}");
                        service.log_warn(&format!("FilaMan device registration failed: {error}"));
                    }
                }
                service.busy.set(false);
            })
            .is_err()
        {
            self.busy.set(false);
            return Err("Could not start FilaMan device registration".to_string());
        }
        Ok(())
    }

    pub async fn prepare_spool(&self, spool: &FilamentSpool) -> Result<SpoolRegistration, String> {
        if !self.import_enabled() {
            return Err("FilaMan location-assisted import is disabled".to_string());
        }
        if let Err(error) = validate_spool(spool) {
            return Err(error.to_string());
        }
        if self.use_offline_inventory() {
            self.log_info(&format!(
                "FilaMan is offline; resolving external ID {} from the cached inventory",
                spool.external_id
            ));
            return Ok(self.offline.resolve(spool, true));
        }
        if self.offline.has_inventory() {
            self.log_info(&format!(
                "FilaMan: resolving external ID {} from the in-memory inventory",
                spool.external_id
            ));
            return Ok(self.offline.resolve(spool, false));
        }
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }

        self.log_info(&format!(
            "FilaMan: checking registration and locations for external ID {}",
            spool.external_id
        ));
        let result = self.prepare_spool_inner(spool).await;
        self.busy.set(false);
        match &result {
            Ok(SpoolRegistration::Existing { spool_id, location_name, .. }) => {
                *self.state.borrow_mut() = format!("Spool {spool_id} already registered");
                self.log_info(&format!(
                    "FilaMan: external ID {} is already registered as spool {} at {}",
                    spool.external_id,
                    spool_id,
                    location_name.as_deref().unwrap_or("no location")
                ));
            }
            Ok(SpoolRegistration::New { locations, .. }) => {
                *self.state.borrow_mut() = format!("Choose one of {} locations", locations.len());
                self.log_info(&format!(
                    "FilaMan: external ID {} is new; {} eligible locations available",
                    spool.external_id,
                    locations.len()
                ));
            }
            Ok(SpoolRegistration::Pending { location_name, .. }) => {
                *self.state.borrow_mut() = format!("Pending synchronization at {location_name}");
            }
            Err(error) => {
                *self.state.borrow_mut() = format!("Lookup failed: {error}");
                self.log_warn(&format!("FilaMan lookup failed for external ID {}: {error}", spool.external_id));
            }
        }
        result
    }

    pub async fn store_spool_at(
        &self,
        spool: &FilamentSpool,
        known_spool_id: Option<u64>,
        location_id: u64,
        location_name: &str,
    ) -> Result<StorageOutcome, String> {
        if !self.import_enabled() {
            return Err("FilaMan location-assisted import is disabled".to_string());
        }
        validate_spool(spool).map_err(String::from)?;
        if location_id == 0 || location_name.trim().is_empty() {
            return Err("FilaMan location is invalid".to_string());
        }

        if self.use_offline_inventory() {
            self.enqueue_storage(spool, location_id, location_name).await?;
            return Ok(StorageOutcome::Queued {
                location_id,
                location_name: location_name.to_string(),
            });
        }
        if self.busy.replace(true) {
            return Err("Another FilaMan request is already running".to_string());
        }
        let result = match known_spool_id {
            Some(spool_id) => self.move_spool_inner(spool_id, location_id).await.map(|response| response.spool_id),
            None => self.apply_storage_online(spool, location_id, location_name).await,
        };
        let outcome = match result {
            Ok(spool_id) => Ok(StorageOutcome::Applied {
                spool_id,
                location_id,
                location_name: location_name.to_string(),
            }),
            Err(error) if is_transport_error(&error) => {
                self.heartbeat_failure_active.set(true);
                self.log_warn(&format!("FilaMan became unavailable; preserving the requested location offline: {error}"));
                self.enqueue_storage(spool, location_id, location_name).await?;
                Ok(StorageOutcome::Queued {
                    location_id,
                    location_name: location_name.to_string(),
                })
            }
            Err(error) => Err(error),
        };
        self.busy.set(false);
        outcome
    }

    async fn apply_storage_online(&self, spool: &FilamentSpool, location_id: u64, _location_name: &str) -> Result<u64, String> {
        if let Some(existing) = self.find_spool(spool).await? {
            if existing.location_id != Some(location_id) {
                self.move_spool_inner(existing.id, location_id).await?;
            }
            return Ok(existing.id);
        }

        let response = self.import_spool_inner(spool, location_id).await?;
        if response.status != "created" {
            self.move_spool_inner(response.spool_id, location_id).await?;
        }
        Ok(response.spool_id)
    }

    async fn enqueue_storage(&self, spool: &FilamentSpool, location_id: u64, location_name: &str) -> Result<(), String> {
        if !self.sdcard_available {
            return Err("Offline storage requires an SD card".to_string());
        }
        let prepared = self.offline.prepare_storage(spool, location_id, location_name)?;
        let changed = self.persist_queue(prepared).await?;
        if changed {
            self.log_info(&format!(
                "FilaMan operation queued offline for external ID {} at {}",
                spool.external_id, location_name
            ));
        } else {
            self.log_info(&format!(
                "FilaMan offline operation for external ID {} is unchanged; SD write skipped",
                spool.external_id
            ));
        }
        Ok(())
    }

    async fn persist_queue(&self, prepared: PreparedQueue) -> Result<bool, String> {
        if !prepared.changed {
            self.offline.commit_queue(prepared);
            return Ok(false);
        }
        let file_store = self.framework.borrow().file_store();
        file_store
            .lock()
            .await
            .create_write_file_bytes(QUEUE_PATH, &prepared.serialized)
            .await
            .map_err(|error| format!("SD write failed: {error:?}"))?;
        self.offline.commit_queue(prepared);
        Ok(true)
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

    async fn prepare_spool_inner(&self, spool: &FilamentSpool) -> Result<SpoolRegistration, String> {
        if let Some(existing) = self.find_spool(spool).await? {
            let location_name = match existing.location_id {
                Some(location_id) => Some(self.load_location_name(location_id).await?),
                None => None,
            };
            return Ok(SpoolRegistration::Existing {
                spool_id: existing.id,
                location_id: existing.location_id,
                location_name,
                locations: self.load_locations().await?,
                offline: false,
            });
        }
        Ok(SpoolRegistration::New {
            locations: self.load_locations().await?,
            offline: false,
            inventory_known: true,
        })
    }

    async fn find_spool(&self, spool: &FilamentSpool) -> Result<Option<ExistingSpool>, String> {
        let canonical_id = canonical_external_id(spool);
        for page in 1..=100 {
            let response = self.api_get(&format!("/spools?page={page}&page_size=50&include_archived=false")).await?;
            let items = response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| "FilaMan spool list response is invalid".to_string())?;
            for item in items {
                let external_id = item.get("external_id").and_then(Value::as_str);
                if external_id
                    .map(|value| value.eq_ignore_ascii_case(&canonical_id) || value.eq_ignore_ascii_case(&spool.external_id))
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

    async fn load_inventory_spools(&self) -> Result<Vec<CachedSpool>, String> {
        let mut spools = Vec::new();
        for page in 1..=100 {
            let response = self.api_get(&format!("/spools?page={page}&page_size=50&include_archived=false")).await?;
            let items = response
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| "FilaMan spool list response is invalid".to_string())?;
            for item in items {
                let Some(external_id) = item.get("external_id").and_then(Value::as_str).and_then(canonicalize_remote_external_id) else {
                    continue;
                };
                let Some(spool_id) = item.get("id").and_then(Value::as_u64).filter(|id| *id > 0) else {
                    continue;
                };
                spools.push(CachedSpool {
                    external_id,
                    spool_id,
                    location_id: item.get("location_id").and_then(Value::as_u64),
                });
            }
            let total = response.get("total").and_then(Value::as_u64).unwrap_or(items.len() as u64);
            if page as u64 * 50 >= total {
                return Ok(spools);
            }
        }
        Err("FilaMan spool list exceeds 5000 entries".to_string())
    }

    async fn import_spool_inner(&self, spool: &FilamentSpool, location_id: u64) -> Result<ImportResponse, String> {
        let (spool_type, payload) = import_payload(spool, location_id)?;
        let response = self
            .api_post(&format!("/devices/filascan/import-spool?type={spool_type}"), &payload)
            .await?;
        let result: ImportResponse = serde_json::from_value(response).map_err(|error| format!("invalid FilaMan plugin import response: {error}"))?;
        if !matches!(result.status.as_str(), "created" | "existing" | "updated") {
            return Err(format!("FilaMan plugin returned unsupported import status '{}'", result.status));
        }
        let canonical_id = canonical_external_id(spool);
        if !result.external_id.eq_ignore_ascii_case(&canonical_id) && !result.external_id.eq_ignore_ascii_case(&spool.external_id) {
            return Err("FilaMan plugin response external_id does not match the requested spool identity".to_string());
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
        self.api_request_with_settings(&settings, method, path, body, ApiAuthentication::DeviceToken)
            .await
    }

    async fn register_device(&self, settings: &FilaManSettings, device_code: &str) -> Result<String, String> {
        let response = self
            .api_request_with_settings(
                settings,
                Method::Post,
                "/devices/register",
                None,
                ApiAuthentication::RegistrationCode(device_code),
            )
            .await?;
        let token = response
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| "FilaMan registration response does not contain a device token".to_string())?
            .to_string();
        validate_device_token(&token)?;
        Ok(token)
    }

    async fn api_request_with_settings(
        &self,
        settings: &FilaManSettings,
        method: Method,
        path: &str,
        body: Option<&[u8]>,
        authentication: ApiAuthentication<'_>,
    ) -> Result<Value, String> {
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
            ("User-Agent", "FilaScan"),
            ("Connection", "close"),
        ];
        match authentication {
            ApiAuthentication::DeviceToken => headers.push(("Authorization", authorization.as_str())),
            ApiAuthentication::RegistrationCode(device_code) => headers.push(("X-Device-Code", device_code)),
        }
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
            headers.push(("Content-Length", content_length.as_str()));
        }

        let api_path = format!("{}/api/v1{}", endpoint.base_path, path);
        let socket_address = SocketAddr::new(IpAddr::V4(address), endpoint.port);

        if endpoint.secure {
            let mut tcp_buffers = Box::new(TcpBuffers::<1, 2048, 8192>::new());
            let tcp = Tcp::new(stack, &mut *tcp_buffers);
            let ca_pem = CString::new(settings.ca_certificate_pem.as_str()).map_err(|_| "FilaMan CA certificate contains a null byte".to_string())?;
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
    external_id: String,
}

#[derive(Debug, Deserialize)]
struct MoveResponse {
    status: String,
    spool_id: u64,
    location_id: u64,
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

enum ApiAuthentication<'a> {
    DeviceToken,
    RegistrationCode(&'a str),
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

fn validate_registration_settings(settings: &FilaManSettings) -> Result<(), String> {
    let endpoint = parse_base_url(&settings.base_url)?;
    if endpoint.secure && !settings.ca_certificate_pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err("FilaMan CA certificate in PEM format is required for HTTPS device registration".to_string());
    }
    Ok(())
}

fn validate_device_code(code: &str) -> Result<(), String> {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()) {
        return Err("FilaMan device registration code must contain exactly 6 letters or digits".to_string());
    }
    Ok(())
}

fn validate_device_token(token: &str) -> Result<(), String> {
    if token.len() > 512 || token.bytes().any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace()) {
        return Err("FilaMan returned an invalid device token".to_string());
    }
    let mut parts = token.splitn(3, '.');
    let prefix = parts.next();
    let device_id = parts.next();
    let secret = parts.next();
    if prefix != Some("dev")
        || device_id.and_then(|value| value.parse::<u64>().ok()).filter(|value| *value > 0).is_none()
        || secret.is_none_or(|value| value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    {
        return Err("FilaMan returned an invalid device token".to_string());
    }
    Ok(())
}

fn device_id_from_token(token: &str) -> Option<u64> {
    if validate_device_token(token).is_err() {
        return None;
    }
    token.split('.').nth(1)?.parse().ok()
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

fn validate_spool(spool: &FilamentSpool) -> Result<(), &'static str> {
    if spool.external_id.is_empty() || spool.material_type.is_empty() || spool.material_name.is_empty() {
        return Err("required decoded filament metadata is missing");
    }
    match &spool.product_reference {
        ProductReference::Bambu {
            material_id,
            variant_id,
            detailed_filament_type,
            color_name,
            ..
        } => {
            if spool.external_id.len() != 32 || !spool.external_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("Bambu Tray UID is missing or invalid");
            }
            if material_id.is_empty() || variant_id.is_empty() || detailed_filament_type.is_empty() || color_name.is_empty() {
                return Err("required decoded Bambu filament metadata is missing");
            }
        }
        ProductReference::OpenPrintTag { .. } => {
            if spool.external_id.len() != 36 {
                return Err("OpenPrintTag instance UUID is missing or invalid");
            }
        }
    }
    Ok(())
}

fn canonical_external_id(spool: &FilamentSpool) -> String {
    match spool.source {
        SpoolSource::BambuLab => format!("bambulab:{}", spool.external_id.to_ascii_uppercase()),
        SpoolSource::OpenPrintTag => format!("openprinttag:{}", spool.external_id.to_ascii_lowercase()),
    }
}

fn canonicalize_remote_external_id(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(id) = value.strip_prefix("bambulab:") {
        return (!id.is_empty()).then(|| format!("bambulab:{}", id.to_ascii_uppercase()));
    }
    if let Some(id) = value.strip_prefix("openprinttag:") {
        return (!id.is_empty()).then(|| format!("openprinttag:{}", id.to_ascii_lowercase()));
    }
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(format!("bambulab:{}", value.to_ascii_uppercase()));
    }
    if value.len() == 36 {
        return Some(format!("openprinttag:{}", value.to_ascii_lowercase()));
    }
    None
}

fn is_transport_error(error: &str) -> bool {
    error.starts_with("DNS lookup failed")
        || error.starts_with("DNS lookup returned")
        || error.starts_with("HTTP request failed")
        || error.starts_with("HTTP body write failed")
        || error.starts_with("HTTP response failed")
        || error.starts_with("HTTPS request failed")
        || error.starts_with("HTTPS body write failed")
        || error.starts_with("HTTPS response failed")
        || error.starts_with("FilaMan response read failed")
        || error.contains("request timed out")
}

fn import_payload(spool: &FilamentSpool, location_id: u64) -> Result<(&'static str, Value), String> {
    match &spool.product_reference {
        ProductReference::Bambu {
            color_code,
            color_name,
            material_id,
            variant_id,
            detailed_filament_type,
            spool_width_mm,
            production_date,
        } => Ok((
            "bambu",
            json!({
                "external_id": spool.external_id,
                "manufacturer": "Bambu Lab",
                "material_id": material_id,
                "variant_id": variant_id,
                "filament_type": spool.material_type,
                "detailed_filament_type": detailed_filament_type,
                "official_material_name": spool.material_name,
                "color_name": color_name,
                "bambu_color_code": optional_string(color_code),
                "primary_rgba": rgba_hex(spool.primary_color()),
                "secondary_rgba": spool.colors.get(1).copied().map(rgba_hex),
                "location_id": location_id,
                "weight_g": spool.nominal_weight_g,
                "diameter_mm": spool.diameter_mm,
                "drying_temperature_c": spool.drying_temperature_c,
                "drying_time_h": spool.drying_time_h,
                "bed_temperature_c": spool.bed_min_c,
                "nozzle_temperature_min_c": spool.nozzle_min_c,
                "nozzle_temperature_max_c": spool.nozzle_max_c,
                "spool_width_mm": optional_f32(*spool_width_mm),
                "filament_length_m": spool.length_m,
                "production_date": optional_string(production_date)
            }),
        )),
        ProductReference::OpenPrintTag { ndef_uri, .. } => {
            let manufacturer = spool.brand.as_deref().unwrap_or("OpenPrintTag");
            let color_name = if spool.color_name.is_empty() {
                rgba_hex(spool.primary_color())
            } else {
                spool.color_name.clone()
            };
            Ok((
                "openprinttag",
                json!({
                    "external_id": spool.external_id,
                    "manufacturer": manufacturer,
                    "material_type": spool.material_type,
                    "material_name": spool.material_name,
                    "color_name": color_name,
                    "primary_rgba": rgba_hex(spool.primary_color()),
                    "secondary_rgba": spool.colors.get(1).copied().map(rgba_hex),
                    "product_url": ndef_uri,
                    "location_id": location_id,
                    "weight_g": spool.nominal_weight_g,
                    "remaining_weight_g": spool.remaining_weight_g,
                    "diameter_mm": spool.diameter_mm,
                }),
            ))
        }
    }
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
