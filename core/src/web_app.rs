use alloc::rc::Rc;
use core::{cell::RefCell, future::ready};

use framework::framework_web_app::{CustomNotFound, NestedAppWithWebAppStateBuilder, WebAppState};
use framework_macros::include_bytes_gz;
use picoserve::{
    AppWithStateBuilder,
    extract::State,
    response::Redirect,
    routing::{get, get_service},
};

use crate::diagnostics::LogBuffer;

#[derive(Clone)]
pub struct FilaScanWebState {
    pub diagnostics: Rc<RefCell<LogBuffer>>,
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
    }
}
