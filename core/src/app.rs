use alloc::{format, rc::Rc};
use core::cell::RefCell;

use embedded_hal_bus::spi::ExclusiveDevice;
use framework::framework::{Framework, FrameworkObserver, WebConfigMode};
use log::{error, info, warn};
use shared::bambu_reader::{BambuReader, BambuReaderObserver, ReaderEvent};
use slint::{Color, ComponentHandle};

use crate::{bambu_spool::BambuSpool, diagnostics::LogBuffer};

slint::include_modules!();

pub fn create_slint_app() -> AppWindow {
    AppWindow::new().expect("Failed to load FilaScan UI")
}

pub struct ReaderController {
    ui: slint::Weak<AppWindow>,
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    _reader: Rc<RefCell<BambuReader>>,
}

pub fn init_app(
    ui: slint::Weak<AppWindow>,
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    spi_device: ExclusiveDevice<esp_hal::spi::master::SpiDmaBus<'static, esp_hal::Async>, esp_hal::gpio::Output<'static>, embassy_time::Delay>,
    irq: esp_hal::gpio::Input<'static>,
) -> Rc<RefCell<ReaderController>> {
    let reader = shared::bambu_reader::init(spi_device, irq, framework.borrow().spawner);
    let controller = Rc::new(RefCell::new(ReaderController {
        ui,
        framework: framework.clone(),
        diagnostics,
        _reader: reader.clone(),
    }));

    let reader_observer: Rc<RefCell<dyn BambuReaderObserver>> = controller.clone();
    reader.borrow_mut().subscribe(Rc::downgrade(&reader_observer));

    let framework_observer: Rc<RefCell<dyn FrameworkObserver>> = controller.clone();
    framework.borrow_mut().subscribe(Rc::downgrade(&framework_observer));

    controller
}

impl ReaderController {
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

    fn show_spool(&self, spool: &BambuSpool) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_reading(false);
        state.set_has_spool(true);
        state.set_status_text("Spool read successfully".into());

        state.set_material_name(spool.official_material_name.clone().into());
        state.set_material_detail(format!("{} · {} · {}", spool.filament_type, spool.material_id, spool.variant_id).into());
        state.set_color_name(spool.color_name.clone().into());
        state.set_color_code(format!("#{}", spool.color_hex).into());
        state.set_bambu_color_code(spool.bambu_color_code.clone().into());
        state.set_primary_color(to_slint_color(spool.primary_rgba));
        state.set_has_secondary_color(spool.secondary_rgba.is_some());
        if let Some(color) = spool.secondary_rgba {
            state.set_secondary_color(to_slint_color(color));
        }

        state.set_physical_parameters(format!("{} g · Ø {:.2} mm · {} m", spool.weight_g, spool.diameter_mm, spool.filament_length_m).into());
        state.set_temperature_parameters(
            format!(
                "Nozzle {}–{} °C · Bed {} °C",
                spool.nozzle_temperature_min_c, spool.nozzle_temperature_max_c, spool.bed_temperature_c
            )
            .into(),
        );
        state.set_drying_parameters(
            format!(
                "Dry {} °C / {} h · Width {:.2} mm",
                spool.drying_temperature_c, spool.drying_time_h, spool.spool_width_mm
            )
            .into(),
        );
        state.set_production_parameters(format!("Produced {} · Tag type {}", spool.production_date, spool.detailed_filament_type).into());
        state.set_identifier_parameters(format!("Spool UID {} · Tag UID {}", spool.spool_uid, spool.tag_uid).into());

        self.framework.borrow().undim_display();
    }

    fn show_status(&self, text: &str) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_reading(false);
        state.set_has_spool(false);
        state.set_status_text(text.into());
        self.framework.borrow().undim_display();
    }
}

impl BambuReaderObserver for ReaderController {
    fn on_reader_available(&mut self, available: bool) {
        let ui = self.ui.unwrap();
        let state = ui.global::<ReaderState>();
        state.set_reader_available(available);
        if available {
            self.log_info("PN532 reader initialized and ready");
            state.set_status_text("Hold a Bambu filament spool near the reader".into());
        } else {
            self.log_error("PN532 reader initialization failed");
            self.show_status("RFID reader unavailable");
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
                state.set_status_text("Reading Bambu RFID tag…".into());
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
                state.set_status_text("Read unstable; retrying…".into());
            }
            ReaderEvent::Spool { tag_uid, blocks } => {
                let spool = BambuSpool::from_tag(tag_uid, blocks);
                self.log_info(&format!(
                    "Spool read: {} / {} / #{} (material {}, variant {})",
                    spool.official_material_name, spool.color_name, spool.color_hex, spool.material_id, spool.variant_id
                ));
                self.show_spool(&spool);
            }
            ReaderEvent::UnsupportedTag { tag_uid, atqa, sak } => {
                self.log_warn(&format!(
                    "Unsupported ISO-A tag: UID {}, ATQA {:02X}{:02X}, SAK {:02X}; expected MIFARE Classic 1K",
                    hex::encode_upper(tag_uid),
                    atqa[0],
                    atqa[1],
                    sak
                ));
                self.show_status("Tag is not a supported Bambu Lab spool");
            }
            ReaderEvent::ReadFailed { tag_uid, detail } => {
                let uid = tag_uid.as_ref().map(hex::encode_upper).unwrap_or_else(|| "unknown".into());
                self.log_error(&format!("Tag {uid}: {detail}"));
                self.show_status("Could not read tag. Reposition the spool and try again");
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
