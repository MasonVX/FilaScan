use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
};
use core::{cell::RefCell, future::ready};

use framework::framework_web_app::{CustomNotFound, Encryptable, Encryption, NestedAppWithWebAppStateBuilder, WebAppState, decrypt};
use framework_macros::include_bytes_gz;
use picoserve::{
    AppWithStateBuilder,
    extract::State,
    response::Redirect,
    routing::{get, get_service, post},
};
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;

use crate::{
    app::{AppWindow, ReaderState},
    catalog::{CatalogService, CatalogSettings},
    diagnostics::LogBuffer,
    filaman::FilaManService,
    localization::{Language, LocalizationService},
};

#[derive(Clone)]
pub struct FilaScanWebState {
    pub diagnostics: Rc<RefCell<LogBuffer>>,
    pub catalog: Rc<CatalogService>,
    pub filaman: Rc<FilaManService>,
    pub localization: Rc<LocalizationService>,
    pub ui: slint::Weak<AppWindow>,
}

impl picoserve::extract::FromRef<WebAppState<FilaScanWebState>> for FilaScanWebState {
    fn from_ref(state: &WebAppState<FilaScanWebState>) -> Self {
        state.more_state.clone()
    }
}

pub struct WifiAppBuilder {
    pub captive: bool,
}

impl NestedAppWithWebAppStateBuilder<FilaScanWebState> for WifiAppBuilder {
    fn path_description(&self) -> &'static str {
        ""
    }
}

impl AppWithStateBuilder for WifiAppBuilder {
    type State = WebAppState<FilaScanWebState>;
    type PathRouter = impl picoserve::routing::PathRouter<WebAppState<FilaScanWebState>>;

    fn build_app(self) -> picoserve::Router<Self::PathRouter, Self::State> {
        picoserve::Router::from_service(CustomNotFound {
            web_server_captive: self.captive,
        })
        .route("/", get(|| ready(Redirect::to("/config"))))
        .route(
            "/styles.css",
            get_service(picoserve::response::File::with_content_type_and_headers(
                "text/css; charset=utf-8",
                include_bytes_gz!("static/styles.css"),
                &[("Content-Encoding", "gzip")],
            )),
        )
        .route(
            "/api/logs",
            get(|State(state): State<FilaScanWebState>| ready(state.diagnostics.borrow().render())),
        )
        .route(
            "/api/language-config",
            get(|State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>| {
                ready(
                    LanguageConfigDto {
                        language: state.localization.language().code().to_string(),
                    }
                    .encrypt(&key.borrow()),
                )
            })
            .post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|json| {
                                serde_json::from_str::<LanguageConfigDto>(&json).map_err(|error| format!("Invalid language settings: {error}"))
                            })
                            .and_then(|config| Language::from_code(&config.language))
                            .and_then(|language| {
                                state.localization.set_language(language)?;
                                state.ui.unwrap().global::<ReaderState>().set_german(language == Language::German);
                                Ok(())
                            });
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
        .route(
            "/api/catalog-config",
            get(|State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>| {
                let settings = state.catalog.settings();
                let status = state.catalog.status();
                ready(
                    CatalogConfigDto {
                        url: settings.url,
                        auto_update: settings.auto_update,
                        entries: status.entries,
                        updating: state.catalog.is_updating(),
                        state: status.state,
                        source: status.source,
                    }
                    .encrypt(&key.borrow()),
                )
            })
            .post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|json| {
                                serde_json::from_str::<CatalogConfigDto>(&json).map_err(|error| format!("Invalid catalog settings: {error}"))
                            })
                            .and_then(|config| {
                                state.catalog.set_settings(CatalogSettings {
                                    url: config.url,
                                    auto_update: config.auto_update,
                                })
                            });
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
        .route(
            "/api/catalog-update",
            post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|_| state.catalog.request_update());
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
        .route(
            "/api/filaman-config",
            get(|State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>| {
                let settings = state.filaman.settings();
                let status = state.filaman.status();
                ready(
                    FilaManConfigResponse {
                        enabled: settings.enabled,
                        base_url: settings.base_url,
                        ca_certificate_pem: settings.ca_certificate_pem,
                        state: status.state,
                        busy: status.busy,
                        registered: status.registered,
                        device_id: status.device_id,
                        device_name: status.device_name,
                        offline: status.offline,
                        cached_spools: status.cached_spools,
                        cached_locations: status.cached_locations,
                        pending_operations: status.pending_operations,
                    }
                    .encrypt(&key.borrow()),
                )
            })
            .post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|json| {
                                serde_json::from_str::<FilaManConfigUpdate>(&json)
                                    .map_err(|error| format!("Invalid FilaMan settings: {error}"))
                            })
                            .and_then(|config| {
                                state.filaman.update_connection_settings(
                                    config.enabled,
                                    config.base_url,
                                    config.ca_certificate_pem,
                                )
                            });
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
        .route(
            "/api/filaman-register",
            post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|json| {
                                serde_json::from_str::<FilaManRegistrationDto>(&json)
                                    .map_err(|error| format!("Invalid FilaMan registration request: {error}"))
                            })
                            .and_then(|request| {
                                state.filaman.request_registration(
                                    request.base_url,
                                    request.device_code,
                                    request.ca_certificate_pem,
                                    request.enabled,
                                )
                            });
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
        .route(
            "/api/filaman-logout",
            post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|_| state.filaman.forget_registration());
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
        .route(
            "/api/filaman-test",
            post(
                |State(Encryption(key)): State<Encryption>, State(state): State<FilaScanWebState>, body: String| {
                    ready({
                        let result = decrypt(&key.borrow(), body.as_bytes())
                            .map_err(|error| format!("Could not decrypt request: {error}"))
                            .and_then(|_| state.filaman.request_test());
                        CatalogActionResponse { error_text: result.err() }.encrypt(&key.borrow())
                    })
                },
            ),
        )
    }
}

#[derive(Deserialize, Serialize)]
struct CatalogConfigDto {
    url: String,
    auto_update: bool,
    #[serde(default)]
    entries: usize,
    #[serde(default)]
    updating: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize, Serialize)]
struct LanguageConfigDto {
    language: String,
}

#[derive(Serialize)]
struct CatalogActionResponse {
    error_text: Option<String>,
}

#[derive(Serialize)]
struct FilaManConfigResponse {
    enabled: bool,
    base_url: String,
    ca_certificate_pem: String,
    state: String,
    busy: bool,
    registered: bool,
    device_id: Option<u64>,
    device_name: Option<String>,
    offline: bool,
    cached_spools: usize,
    cached_locations: usize,
    pending_operations: usize,
}

#[derive(Deserialize)]
struct FilaManConfigUpdate {
    enabled: bool,
    base_url: String,
    ca_certificate_pem: String,
}

#[derive(Deserialize)]
struct FilaManRegistrationDto {
    enabled: bool,
    base_url: String,
    device_code: String,
    ca_certificate_pem: String,
}
