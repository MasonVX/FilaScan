use alloc::{
    format,
    rc::{Rc, Weak},
    string::String,
    vec::Vec,
};
use core::cell::RefCell;

use embedded_hal_bus::spi::ExclusiveDevice;
use framework::framework::{Framework, FrameworkObserver, WebConfigMode};
use framework::utils::SpawnerHeapExt;
use hashbrown::HashMap;
use log::{error, info, warn};
use shared::bambu_reader::{BambuReader, BambuReaderObserver, ReaderEvent};
use slint::{Color, ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::{
    bambu_spool::BambuSpool,
    catalog::Catalog,
    diagnostics::LogBuffer,
    filaman::{ArchiveOutcome, FilaManLocation, FilaManService, ImportOutcome, MoveOutcome, SpoolRegistration},
    image_loader,
    localization::{self, Language, LocalizationService},
};

slint::include_modules!();

pub fn create_slint_app() -> AppWindow {
    AppWindow::new().expect("Failed to load FilaScan UI")
}

pub struct ReaderController {
    ui: slint::Weak<AppWindow>,
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    catalog: Rc<RefCell<Catalog>>,
    filaman: Rc<FilaManService>,
    localization: Rc<LocalizationService>,
    self_ref: Weak<RefCell<ReaderController>>,
    active_tray_uid: String,
    pending_spool: Option<BambuSpool>,
    pending_locations: Vec<FilaManLocation>,
    registered_spool_id: Option<u64>,
    current_location_id: Option<u64>,
    _reader: Rc<RefCell<BambuReader>>,
}

pub fn init_app(
    ui: slint::Weak<AppWindow>,
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    catalog: Rc<RefCell<Catalog>>,
    filaman: Rc<FilaManService>,
    localization: Rc<LocalizationService>,
    spi_device: ExclusiveDevice<esp_hal::spi::master::SpiDmaBus<'static, esp_hal::Async>, esp_hal::gpio::Output<'static>, embassy_time::Delay>,
    irq: esp_hal::gpio::Input<'static>,
) -> Rc<RefCell<ReaderController>> {
    let reader = shared::bambu_reader::init(spi_device, irq, framework.borrow().spawner);
    let controller = Rc::new(RefCell::new(ReaderController {
        ui: ui.clone(),
        framework: framework.clone(),
        diagnostics,
        catalog,
        filaman,
        localization,
        self_ref: Weak::new(),
        active_tray_uid: String::new(),
        pending_spool: None,
        pending_locations: Vec::new(),
        registered_spool_id: None,
        current_location_id: None,
        _reader: reader.clone(),
    }));
    controller.borrow_mut().self_ref = Rc::downgrade(&controller);
    let initial_language = controller.borrow().language();
    let initial_window = ui.unwrap();
    let initial_state = initial_window.global::<ReaderState>();
    initial_state.set_german(initial_language == Language::German);
    initial_state.set_status_text(localization::text(initial_language, "Starting RFID reader…", "RFID-Leser wird gestartet…").into());

    {
        let weak = Rc::downgrade(&controller);
        ui.unwrap().global::<ReaderState>().on_location_selected(move |location_id| {
            if let Some(controller) = weak.upgrade() {
                controller.borrow_mut().select_location(location_id);
            }
        });
    }
    {
        let weak = Rc::downgrade(&controller);
        ui.unwrap().global::<ReaderState>().on_cancel_import(move || {
            if let Some(controller) = weak.upgrade() {
                controller.borrow_mut().cancel_import();
            }
        });
    }
    {
        let weak = Rc::downgrade(&controller);
        ui.unwrap().global::<ReaderState>().on_manage_location(move || {
            if let Some(controller) = weak.upgrade() {
                controller.borrow_mut().open_location_management();
            }
        });
    }
    {
        let weak = Rc::downgrade(&controller);
        ui.unwrap().global::<ReaderState>().on_archive_spool(move || {
            if let Some(controller) = weak.upgrade() {
                controller.borrow_mut().archive_spool();
            }
        });
    }

    let reader_observer: Rc<RefCell<dyn BambuReaderObserver>> = controller.clone();
    reader.borrow_mut().subscribe(Rc::downgrade(&reader_observer));

    let framework_observer: Rc<RefCell<dyn FrameworkObserver>> = controller.clone();
    framework.borrow_mut().subscribe(Rc::downgrade(&framework_observer));

    controller
}

impl ReaderController {
    fn language(&self) -> Language {
        self.localization.language()
    }

    fn t(&self, english: &'static str, german: &'static str) -> &'static str {
        localization::text(self.language(), english, german)
    }
    fn log_info(&self, message: &str) {
        info!("{}", message);
        self.diagnostics.borrow_mut().info(message);
    }

    fn log_warn(&self, message: &str) {
        warn!("{}", message);
        self.diagnostics.borrow_mut().warn(message);
    }

    fn log_error(&self, message: &str) {
        error!("{}", message);
        self.diagnostics.borrow_mut().error(message);
    }

    fn close_location_selection(&self) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_choosing_location(false);
        state.set_moving_existing_spool(false);
        state.set_confirming_archive(false);
        state.set_filaman_busy(false);
        state.set_location_prompt("".into());
    }

    fn reset_filaman_context(&mut self) {
        self.pending_spool = None;
        self.pending_locations.clear();
        self.registered_spool_id = None;
        self.current_location_id = None;
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_has_location_action(false);
        state.set_current_location_label("".into());
        state.set_locations(ModelRc::new(VecModel::from(Vec::<LocationChoice>::new())));
        self.close_location_selection();
    }

    fn update_location_choices(&self) {
        let choices: Vec<LocationChoice> = self
            .pending_locations
            .iter()
            .filter(|location| location.id <= i32::MAX as u64)
            .map(|location| LocationChoice {
                id: location.id as i32,
                name: SharedString::from(if Some(location.id) == self.current_location_id {
                    format!("{} (current)", location.name)
                } else {
                    location.name.clone()
                }),
            })
            .collect();
        self.ui
            .unwrap()
            .global::<ReaderState>()
            .set_locations(ModelRc::new(VecModel::from(choices)));
    }

    fn start_filaman_preparation(&mut self, spool: BambuSpool) {
        self.reset_filaman_context();
        if !self.filaman.import_enabled() {
            return;
        }

        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_filaman_busy(true);
        state.set_status_text(self.t("Checking FilaMan inventory…", "FilaMan-Bestand wird geprüft…").into());

        let service = self.filaman.clone();
        let weak = self.self_ref.clone();
        let spawner = self.framework.borrow().spawner;
        if spawner
            .spawn_heap(async move {
                let result = service.prepare_spool(&spool).await;
                if let Some(controller) = weak.upgrade() {
                    controller.borrow_mut().finish_filaman_preparation(spool, result);
                }
            })
            .is_err()
        {
            state.set_filaman_busy(false);
            state.set_status_text(
                self.t("Could not start FilaMan lookup", "FilaMan-Abfrage konnte nicht gestartet werden")
                    .into(),
            );
            self.log_warn("Could not start FilaMan spool lookup task");
        }
    }

    fn finish_filaman_preparation(&mut self, spool: BambuSpool, result: Result<SpoolRegistration, String>) {
        if spool.tray_uid != self.active_tray_uid {
            self.log_info(&format!("Ignoring stale FilaMan lookup for Tray UID {}", spool.tray_uid));
            return;
        }

        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_filaman_busy(false);
        match result {
            Ok(SpoolRegistration::Existing {
                spool_id,
                location_id,
                location_name,
                locations,
            }) => {
                self.pending_spool = None;
                self.pending_locations = locations;
                self.registered_spool_id = Some(spool_id);
                self.current_location_id = location_id;
                self.update_location_choices();
                state.set_current_location_label(
                    format!(
                        "{}: {}",
                        self.t("Location", "Standort"),
                        location_name.as_deref().unwrap_or(self.t("Not assigned", "Nicht zugewiesen"))
                    )
                    .into(),
                );
                state.set_has_location_action(true);
                self.close_location_selection();
                state.set_status_text(
                    if self.language() == Language::German {
                        format!("Bereits als Spule #{spool_id} in FilaMan registriert")
                    } else {
                        format!("Already registered in FilaMan as spool #{spool_id}")
                    }
                    .into(),
                );
            }
            Ok(SpoolRegistration::New { locations }) => {
                self.pending_spool = Some(spool);
                self.pending_locations = locations;
                self.update_location_choices();
                state.set_location_prompt(
                    if self.pending_locations.is_empty() {
                        self.t(
                            "No eligible FilaMan locations are available",
                            "Keine geeigneten FilaMan-Standorte verfügbar",
                        )
                    } else {
                        self.t("Choose where this spool will be stored", "Wähle den Lagerort für diese Spule")
                    }
                    .into(),
                );
                state.set_choosing_location(true);
                state.set_status_text(self.t("New spool: choose a storage location", "Neue Spule: Lagerort auswählen").into());
                self.framework.borrow().undim_display();
            }
            Err(error) => {
                self.reset_filaman_context();
                state.set_status_text(
                    self.t(
                        "FilaMan lookup failed; spool was not added",
                        "FilaMan-Abfrage fehlgeschlagen; Spule wurde nicht hinzugefügt",
                    )
                    .into(),
                );
                self.log_warn(&format!("FilaMan location selection unavailable: {error}"));
            }
        }
    }

    fn open_location_management(&mut self) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        if state.get_filaman_busy() || self.registered_spool_id.is_none() {
            return;
        }
        self.update_location_choices();
        state.set_moving_existing_spool(true);
        state.set_confirming_archive(false);
        state.set_choosing_location(true);
        state.set_location_prompt(
            if self.pending_locations.is_empty() {
                self.t(
                    "No eligible FilaMan locations are available",
                    "Keine geeigneten FilaMan-Standorte verfügbar",
                )
            } else {
                self.t("Choose the new storage location", "Wähle den neuen Lagerort")
            }
            .into(),
        );
        self.framework.borrow().undim_display();
    }

    fn select_location(&mut self, location_id: i32) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        if state.get_filaman_busy() || location_id <= 0 {
            return;
        }
        let Some(location) = self.pending_locations.iter().find(|location| location.id == location_id as u64).cloned() else {
            self.log_warn(&format!("Ignoring unknown FilaMan location selection {location_id}"));
            return;
        };
        state.set_filaman_busy(true);
        state.set_location_prompt(
            if self.language() == Language::German {
                format!("Standort {} wird gespeichert…", location.name)
            } else {
                format!("Saving location {}…", location.name)
            }
            .into(),
        );
        let service = self.filaman.clone();
        let weak = self.self_ref.clone();
        let spawner = self.framework.borrow().spawner;
        let spawn_result = if let Some(spool) = self.pending_spool.clone() {
            spawner.spawn_heap(async move {
                let result = service.import_spool_at(&spool, location.id).await;
                if let Some(controller) = weak.upgrade() {
                    controller.borrow_mut().finish_spool_import(spool, location, result);
                }
            })
        } else if let Some(spool_id) = self.registered_spool_id {
            spawner.spawn_heap(async move {
                let result = service.move_spool(spool_id, location.id).await;
                if let Some(controller) = weak.upgrade() {
                    controller.borrow_mut().finish_spool_move(location, result);
                }
            })
        } else {
            state.set_filaman_busy(false);
            self.log_warn("Ignoring FilaMan location selection without a pending or registered spool");
            return;
        };
        if spawn_result.is_err() {
            state.set_filaman_busy(false);
            state.set_location_prompt(
                self.t(
                    "Could not start FilaMan location update",
                    "FilaMan-Standortänderung konnte nicht gestartet werden",
                )
                .into(),
            );
            self.log_warn("Could not start FilaMan location task");
        }
    }

    fn finish_spool_import(&mut self, spool: BambuSpool, location: FilaManLocation, result: Result<ImportOutcome, String>) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_filaman_busy(false);
        match result {
            Ok(outcome) => {
                if outcome.status != "created" {
                    self.close_location_selection();
                    self.start_filaman_preparation(spool);
                    return;
                }
                self.pending_spool = None;
                self.registered_spool_id = Some(outcome.spool_id);
                self.current_location_id = Some(location.id);
                self.update_location_choices();
                state.set_current_location_label(format!("{}: {}", self.t("Location", "Standort"), location.name).into());
                state.set_has_location_action(true);
                self.close_location_selection();
                let message = if self.language() == Language::German {
                    format!("Spule #{} zu {} hinzugefügt", outcome.spool_id, location.name)
                } else {
                    format!("Added spool #{} to {}", outcome.spool_id, location.name)
                };
                state.set_status_text(message.into());
            }
            Err(error) => {
                self.pending_spool = Some(spool);
                state.set_choosing_location(true);
                state.set_location_prompt(
                    self.t(
                        "Import failed. Choose a location to retry or cancel",
                        "Import fehlgeschlagen. Standort zum Wiederholen wählen oder abbrechen",
                    )
                    .into(),
                );
                state.set_status_text(
                    self.t(
                        "FilaMan import failed; spool was not added",
                        "FilaMan-Import fehlgeschlagen; Spule wurde nicht hinzugefügt",
                    )
                    .into(),
                );
                self.log_warn(&format!("FilaMan import can be retried after failure: {error}"));
            }
        }
    }

    fn finish_spool_move(&mut self, location: FilaManLocation, result: Result<MoveOutcome, String>) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_filaman_busy(false);
        match result {
            Ok(outcome) => {
                self.registered_spool_id = Some(outcome.spool_id);
                self.current_location_id = Some(outcome.location_id);
                self.update_location_choices();
                state.set_current_location_label(format!("{}: {}", self.t("Location", "Standort"), outcome.location_name).into());
                state.set_has_location_action(true);
                self.close_location_selection();
                state.set_status_text(
                    if self.language() == Language::German {
                        format!("Spule #{} nach {} verschoben", outcome.spool_id, outcome.location_name)
                    } else {
                        format!("Moved spool #{} to {}", outcome.spool_id, outcome.location_name)
                    }
                    .into(),
                );
            }
            Err(error) => {
                state.set_moving_existing_spool(true);
                state.set_choosing_location(true);
                state.set_location_prompt(
                    self.t(
                        "Move failed. Choose a location to retry or cancel",
                        "Verschieben fehlgeschlagen. Standort zum Wiederholen wählen oder abbrechen",
                    )
                    .into(),
                );
                state.set_status_text(self.t("FilaMan location update failed", "FilaMan-Standortänderung fehlgeschlagen").into());
                self.log_warn(&format!("FilaMan spool move to {} can be retried: {error}", location.name));
            }
        }
    }

    fn archive_spool(&mut self) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        if state.get_filaman_busy() {
            return;
        }
        let Some(spool_id) = self.registered_spool_id else {
            self.log_warn("Ignoring archive request without a registered FilaMan spool");
            return;
        };
        if !state.get_confirming_archive() {
            state.set_confirming_archive(true);
            state.set_location_prompt(
                self.t(
                    "Confirm removal from the active FilaMan inventory",
                    "Entfernen aus dem aktiven FilaMan-Bestand bestätigen",
                )
                .into(),
            );
            self.framework.borrow().undim_display();
            return;
        }

        state.set_filaman_busy(true);
        state.set_location_prompt(self.t("Archiving spool in FilaMan…", "Spule wird in FilaMan archiviert…").into());
        let service = self.filaman.clone();
        let weak = self.self_ref.clone();
        let spawner = self.framework.borrow().spawner;
        if spawner
            .spawn_heap(async move {
                let result = service.archive_spool(spool_id).await;
                if let Some(controller) = weak.upgrade() {
                    controller.borrow_mut().finish_spool_archive(spool_id, result);
                }
            })
            .is_err()
        {
            state.set_filaman_busy(false);
            state.set_location_prompt(
                self.t(
                    "Could not start FilaMan archive request",
                    "FilaMan-Archivierung konnte nicht gestartet werden",
                )
                .into(),
            );
            self.log_warn("Could not start FilaMan spool archive task");
        }
    }

    fn finish_spool_archive(&mut self, spool_id: u64, result: Result<ArchiveOutcome, String>) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        state.set_filaman_busy(false);
        if self.registered_spool_id != Some(spool_id) {
            self.log_info(&format!("Ignoring stale FilaMan archive result for spool {spool_id}"));
            return;
        }
        match result {
            Ok(outcome) => {
                self.reset_filaman_context();
                state.set_status_text(
                    if self.language() == Language::German {
                        format!("Spule #{} in FilaMan archiviert", outcome.spool_id)
                    } else {
                        format!("Archived spool #{} in FilaMan", outcome.spool_id)
                    }
                    .into(),
                );
            }
            Err(error) => {
                state.set_moving_existing_spool(true);
                state.set_confirming_archive(true);
                state.set_choosing_location(true);
                state.set_location_prompt(
                    self.t("Archive failed. Retry or go back", "Archivierung fehlgeschlagen. Wiederholen oder zurück")
                        .into(),
                );
                state.set_status_text(self.t("FilaMan archive failed", "FilaMan-Archivierung fehlgeschlagen").into());
                self.log_warn(&format!("FilaMan spool archive can be retried: {error}"));
            }
        }
    }

    fn cancel_import(&mut self) {
        let window = self.ui.unwrap();
        let state = window.global::<ReaderState>();
        if state.get_filaman_busy() {
            return;
        }
        if state.get_confirming_archive() {
            state.set_confirming_archive(false);
            state.set_location_prompt(
                if self.pending_locations.is_empty() {
                    self.t(
                        "No eligible FilaMan locations are available",
                        "Keine geeigneten FilaMan-Standorte verfügbar",
                    )
                } else {
                    self.t("Choose the new storage location", "Wähle den neuen Lagerort")
                }
                .into(),
            );
            return;
        }
        if self.pending_spool.is_some() {
            self.log_info("FilaMan spool import cancelled by user");
            self.reset_filaman_context();
            state.set_status_text(
                self.t(
                    "FilaMan import cancelled; spool was not added",
                    "FilaMan-Import abgebrochen; Spule wurde nicht hinzugefügt",
                )
                .into(),
            );
        } else if self.registered_spool_id.is_some() {
            self.log_info("FilaMan location change cancelled by user");
            self.close_location_selection();
            state.set_status_text(self.t("Location unchanged", "Standort unverändert").into());
        } else {
            self.close_location_selection();
        }
    }

    fn show_spool(&mut self, spool: &BambuSpool) {
        self.active_tray_uid = spool.tray_uid.clone();
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_reading(false);
        state.set_has_spool(true);
        state.set_status_text(self.t("Spool read successfully", "Spule erfolgreich gelesen").into());

        state.set_material_name(spool.official_material_name.clone().into());
        state.set_material_detail(format!("{} · {} · {}", spool.filament_type, spool.material_id, spool.variant_id).into());
        state.set_color_name(spool.display_color_name.clone().into());
        state.set_color_code(format!("#{}", spool.color_hex).into());
        state.set_bambu_color_code(spool.bambu_color_code.clone().into());
        state.set_primary_color(to_slint_color(spool.primary_rgba));
        state.set_has_secondary_color(spool.secondary_rgba.is_some());
        if let Some(color) = spool.secondary_rgba {
            state.set_secondary_color(to_slint_color(color));
        }
        state.set_spool_image(Image::default());
        state.set_has_spool_image(false);
        state.set_spool_image_loading(false);

        state.set_physical_parameters(format!("{} g · Ø {:.2} mm · {} m", spool.weight_g, spool.diameter_mm, spool.filament_length_m).into());
        state.set_temperature_parameters(
            if self.language() == Language::German {
                format!(
                    "Düse {}–{} °C · Bett {} °C",
                    spool.nozzle_temperature_min_c, spool.nozzle_temperature_max_c, spool.bed_temperature_c
                )
            } else {
                format!(
                    "Nozzle {}–{} °C · Bed {} °C",
                    spool.nozzle_temperature_min_c, spool.nozzle_temperature_max_c, spool.bed_temperature_c
                )
            }
            .into(),
        );
        state.set_drying_parameters(
            if self.language() == Language::German {
                format!(
                    "Trocknen {} °C / {} h · Breite {:.2} mm",
                    spool.drying_temperature_c, spool.drying_time_h, spool.spool_width_mm
                )
            } else {
                format!(
                    "Dry {} °C / {} h · Width {:.2} mm",
                    spool.drying_temperature_c, spool.drying_time_h, spool.spool_width_mm
                )
            }
            .into(),
        );
        state.set_production_parameters(
            if self.language() == Language::German {
                format!("Produziert {} · Tag-Typ {}", spool.production_date, spool.detailed_filament_type)
            } else {
                format!("Produced {} · Tag type {}", spool.production_date, spool.detailed_filament_type)
            }
            .into(),
        );
        state.set_identifier_parameters(format!("Tray UID {} · Tag UID {}", spool.tray_uid, spool.tag_uid).into());

        self.framework.borrow().undim_display();
        self.start_product_image_load(spool);
    }

    fn start_product_image_load(&self, spool: &BambuSpool) {
        if spool.bambu_color_code.is_empty() {
            self.log_info(&format!("No Bambu product image mapping for color code {}", spool.bambu_color_code));
            return;
        }

        let product_code = spool.bambu_color_code.clone();
        let ui = self.ui.clone();
        let diagnostics = self.diagnostics.clone();
        let (framework, spawner) = {
            let framework = self.framework.borrow();
            (self.framework.clone(), framework.spawner)
        };
        ui.unwrap().global::<ReaderState>().set_spool_image_loading(true);

        let future = async move {
            let result = image_loader::load_product_image(framework, &product_code).await;
            let window = ui.unwrap();
            let state = window.global::<ReaderState>();
            if !state.get_has_spool() || state.get_bambu_color_code().as_str() != product_code.as_str() {
                return;
            }

            state.set_spool_image_loading(false);
            match result {
                Ok(loaded) => {
                    state.set_spool_image(loaded.image);
                    state.set_has_spool_image(true);
                    let source = match loaded.source {
                        image_loader::ProductImageSource::SdCard => "SD cache",
                        image_loader::ProductImageSource::BambuCdn => "Bambu CDN",
                    };
                    let message = format!("Bambu product image loaded for color code {product_code} from {source}");
                    info!("{}", message);
                    diagnostics.borrow_mut().info(&message);
                }
                Err(detail) => {
                    let message = format!("Bambu product image {product_code} unavailable: {detail}");
                    warn!("{}", message);
                    diagnostics.borrow_mut().warn(&message);
                }
            }
        };

        if spawner.spawn_heap(future).is_err() {
            self.ui.unwrap().global::<ReaderState>().set_spool_image_loading(false);
            self.log_warn("Could not start Bambu product image download task");
        }
    }

    fn show_status(&self, text: &str) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_reading(false);
        state.set_has_spool(false);
        state.set_status_text(text.into());
        self.framework.borrow().undim_display();
    }

    fn log_spool_dump(&self, spool: &BambuSpool, blocks: &HashMap<i32, alloc::vec::Vec<u8>>) {
        self.log_info("RFID decoded data:");
        self.log_info(&format!("  Tag UID: {}", spool.tag_uid));
        self.log_info(&format!("  Tray UID: {}", spool.tray_uid));
        self.log_info(&format!("  Material ID: {}", spool.material_id));
        self.log_info(&format!("  Variant ID: {}", spool.variant_id));
        self.log_info(&format!(
            "  Filament: {} / {} / mapped as {}",
            spool.filament_type, spool.detailed_filament_type, spool.official_material_name
        ));
        self.log_info(&format!(
            "  Color: {} / #{} / Bambu code {}",
            spool.color_name, spool.color_hex, spool.bambu_color_code
        ));
        self.log_info(&format!(
            "  Physical: {} g / diameter {:.3} mm / length {} m / spool width {:.2} mm",
            spool.weight_g, spool.diameter_mm, spool.filament_length_m, spool.spool_width_mm
        ));
        self.log_info(&format!(
            "  Temperatures: nozzle {}-{} C / bed {} C / drying {} C for {} h",
            spool.nozzle_temperature_min_c, spool.nozzle_temperature_max_c, spool.bed_temperature_c, spool.drying_temperature_c, spool.drying_time_h
        ));
        self.log_info(&format!("  Production date: {}", spool.production_date));

        let mut block_numbers: alloc::vec::Vec<i32> = blocks.keys().copied().collect();
        block_numbers.sort_unstable();
        self.log_info(&format!("RFID raw block dump ({} blocks):", block_numbers.len()));
        for block_number in block_numbers {
            if let Some(bytes) = blocks.get(&block_number) {
                self.log_info(&format!("  Block {block_number:02}: {}", hex::encode_upper(bytes)));
            }
        }
    }
}

impl BambuReaderObserver for ReaderController {
    fn on_reader_available(&mut self, available: bool) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_reader_available(available);
        if available {
            self.log_info("PN532 reader initialized and ready");
            state.set_status_text(
                self.t(
                    "Hold a Bambu filament spool near the reader",
                    "Halte eine Bambu-Filamentspule an den Leser",
                )
                .into(),
            );
        } else {
            self.log_error("PN532 reader initialization failed");
            self.show_status(self.t("RFID reader unavailable", "RFID-Leser nicht verfügbar"));
        }
    }

    fn on_reader_event(&mut self, event: &ReaderEvent) {
        match event {
            ReaderEvent::Reading { tag_uid, atqa, sak } => {
                self.log_info(&format!(
                    "Tag detected: UID {}, ATQA {:02X}{:02X}, SAK {:02X}; reading Bambu payload",
                    hex::encode_upper(tag_uid),
                    atqa[0],
                    atqa[1],
                    sak
                ));
                let ui = self.ui.unwrap();
                let state = ui.global::<ReaderState>();
                state.set_reading(true);
                state.set_status_text(self.t("Reading Bambu RFID tag…", "Bambu-RFID-Tag wird gelesen…").into());
                self.framework.borrow().undim_display();
            }
            ReaderEvent::Retrying {
                tag_uid,
                next_attempt,
                detail,
            } => {
                self.log_warn(&format!(
                    "Tag {}: {}; waiting for PN532 reacquisition, next attempt {}",
                    hex::encode_upper(tag_uid),
                    detail,
                    next_attempt
                ));
                let ui = self.ui.unwrap();
                let state = ui.global::<ReaderState>();
                state.set_reading(true);
                state.set_status_text(self.t("Read unstable; retrying…", "Lesen instabil; neuer Versuch…").into());
            }
            ReaderEvent::Spool { tag_uid, blocks } => {
                let spool = BambuSpool::from_tag(tag_uid, blocks, &self.catalog.borrow(), self.language());
                self.log_info(&format!(
                    "Spool read: {} / {} / #{} (material {}, variant {})",
                    spool.official_material_name, spool.color_name, spool.color_hex, spool.material_id, spool.variant_id
                ));
                self.log_spool_dump(&spool, blocks);
                self.show_spool(&spool);
                self.start_filaman_preparation(spool);
            }
            ReaderEvent::UnsupportedTag { tag_uid, atqa, sak } => {
                self.log_warn(&format!(
                    "Unsupported ISO-A tag: UID {}, ATQA {:02X}{:02X}, SAK {:02X}; expected MIFARE Classic 1K",
                    hex::encode_upper(tag_uid),
                    atqa[0],
                    atqa[1],
                    sak
                ));
                self.show_status(self.t(
                    "Tag is not a supported Bambu Lab spool",
                    "Tag gehört nicht zu einer unterstützten Bambu-Lab-Spule",
                ));
            }
            ReaderEvent::ReadFailed { tag_uid, detail } => {
                let uid = tag_uid.as_ref().map(hex::encode_upper).unwrap_or_else(|| "unknown".into());
                self.log_error(&format!("Tag {uid}: {detail}"));
                self.show_status(self.t(
                    "Could not read tag. Reposition the spool and try again",
                    "Tag konnte nicht gelesen werden. Spule neu positionieren und erneut versuchen",
                ));
            }
            ReaderEvent::TagRemoved => self.log_info("Tag removed from reader field"),
        }
    }
}

impl FrameworkObserver for ReaderController {
    fn on_webapp_url_update(&self, ip_url: &str, name_url: Option<&str>, ssid: &str) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_wifi_url(name_url.unwrap_or(ip_url).into());
        state.set_wifi_ssid(ssid.into());
    }

    fn on_initialization_completed(&self, _status: bool) {}
    fn on_ota_version_available(&mut self, _version: &str, _newer: bool) {}
    fn on_ota_start(&mut self) {}
    fn on_ota_status(&mut self, _text: &str) {}
    fn on_ota_failed(&mut self, _text: &str) {}
    fn on_ota_completed(&mut self, _text: &str) {}

    fn on_web_config_started(&self, key: &str, _mode: WebConfigMode) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_wifi_setup_active(true);
        state.set_wifi_key(key.into());
    }

    fn on_web_config_stopped(&self) {
        let ui = self.ui.unwrap();
        ui.global::<ReaderState>().set_wifi_setup_active(false);
    }

    fn on_wifi_sta_connected(&self) {}
    fn on_wifi_sta_disconnected(&self) {}
}

fn to_slint_color([red, green, blue, alpha]: [u8; 4]) -> Color {
    Color::from_argb_u8(alpha, red, green, blue)
}
