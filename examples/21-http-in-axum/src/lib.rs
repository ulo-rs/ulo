use axum::Router;
use axum::routing::get;
use tokio::sync::Semaphore;
use ulo::{App, AppBuilder, Dep, injectable, module, routes};
use ulo_http_axum::{Embedded, Handle};

/// Holds `/api/work` until released; `entered` counts the handlers that have started.
pub struct Gate {
    pub entered: Semaphore,
    pub release: Semaphore,
}

#[injectable]
impl Gate {
    fn new() -> Self {
        Gate { entered: Semaphore::new(0), release: Semaphore::new(0) }
    }
}

#[injectable]
pub struct ApiController {
    gate: Dep<Gate>,
}

/// Routes are written as in any ulo app; the host strips `/api` before ulo sees the path.
#[routes]
impl ApiController {
    #[ulo_http::get("/hello")]
    fn hello(&self) -> &'static str {
        "hello from ulo"
    }

    #[ulo_http::get("/work")]
    async fn work(&self) -> &'static str {
        self.gate.entered.add_permits(1);
        self.gate.release.acquire().await.expect("the gate is never closed").forget();
        "finished"
    }
}

#[module(providers = [Gate], controllers = [ApiController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}

/// The embedding server the app binds to: it binds no socket, the host owns them.
pub fn embedded() -> Embedded {
    Embedded::new().nested_at("/api")
}

/// The host's router: its own route beside the app, mounted at `/api`.
pub fn router(app: &Handle) -> Router {
    Router::new().route("/health", get(|| async { "axum is up" })).nest_service("/api", app.service())
}
