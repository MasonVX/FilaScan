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
    filaman::{FilaManService, FilaManSettings},
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
                    FilaManConfigDto {
                        enabled: settings.enabled,
                        base_url: settings.base_url,
                        device_token: settings.device_token,
                        ca_certificate_pem: settings.ca_certificate_pem,
                        state: status.state,
                        busy: status.busy,
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
                                serde_json::from_str::<FilaManConfigDto>(&json).map_err(|error| format!("Invalid FilaMan settings: {error}"))
                            })
                            .and_then(|config| {
                                state.filaman.set_settings(FilaManSettings {
                                    enabled: config.enabled,
                                    base_url: config.base_url,
                                    device_token: config.device_token,
                                    ca_certificate_pem: config.ca_certificate_pem,
                                })
                            });
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

#[derive(Deserialize, Serialize)]
struct FilaManConfigDto {
    enabled: bool,
    base_url: String,
    device_token: String,
    ca_certificate_pem: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    busy: bool,
}
