use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
};
use core::cell::RefCell;

use framework::{framework::Framework, utils::SpawnerHeapExt};
use serde::{Deserialize, Serialize};

use crate::diagnostics::LogBuffer;

const SETTINGS_PATH: &str = "/filascan/lang.jsn";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    English,
    German,
}

impl Default for Language {
    fn default() -> Self {
        Self::English
    }
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::German => "de",
        }
    }

    pub fn from_code(value: &str) -> Result<Self, String> {
        match value {
            "en" => Ok(Self::English),
            "de" => Ok(Self::German),
            _ => Err("Unsupported display language".to_string()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalizationSettings {
    language: Language,
}

pub struct LocalizationService {
    framework: Rc<RefCell<Framework>>,
    diagnostics: Rc<RefCell<LogBuffer>>,
    language: RefCell<Language>,
    sdcard_available: bool,
}

impl LocalizationService {
    pub fn new(framework: Rc<RefCell<Framework>>, diagnostics: Rc<RefCell<LogBuffer>>, sdcard_available: bool) -> Rc<Self> {
        Rc::new(Self {
            framework,
            diagnostics,
            language: RefCell::new(Language::default()),
            sdcard_available,
        })
    }

    pub fn language(&self) -> Language {
        *self.language.borrow()
    }

    pub async fn load_from_sd(&self) {
        if !self.sdcard_available {
            return;
        }
        let file_store = self.framework.borrow().file_store();
        let Ok(bytes) = file_store.lock().await.read_file_bytes(SETTINGS_PATH).await else {
            return;
        };
        match serde_json::from_slice::<LocalizationSettings>(&bytes) {
            Ok(settings) => {
                *self.language.borrow_mut() = settings.language;
                self.log_info(&format!("Display language loaded: {}", settings.language.code()));
            }
            Err(error) => self.log_warn(&format!("Ignoring invalid display language settings: {error}")),
        }
    }

    pub fn set_language(self: &Rc<Self>, language: Language) -> Result<(), String> {
        *self.language.borrow_mut() = language;
        self.log_info(&format!("Display language changed to {}", language.code()));
        if !self.sdcard_available {
            return Err("Display language cannot be saved without an SD card".to_string());
        }
        let service = self.clone();
        let spawner = self.framework.borrow().spawner;
        spawner
            .spawn_heap(async move {
                if let Err(error) = service.persist().await {
                    service.log_warn(&format!("Could not persist display language: {error}"));
                }
            })
            .map_err(|_| "Could not start display language save task".to_string())
    }

    async fn persist(&self) -> Result<(), String> {
        let bytes =
            serde_json::to_vec(&LocalizationSettings { language: self.language() }).map_err(|error| format!("serialization failed: {error}"))?;
        let file_store = self.framework.borrow().file_store();
        file_store
            .lock()
            .await
            .create_write_file_bytes(SETTINGS_PATH, &bytes)
            .await
            .map_err(|error| format!("SD write failed: {error:?}"))
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

pub fn text(language: Language, english: &'static str, german: &'static str) -> &'static str {
    match language {
        Language::English => english,
        Language::German => german,
    }
}
