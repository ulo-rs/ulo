use ulo::{App, AppBuilder, injectable, module, routes};

#[injectable]
pub struct HelloController;

#[routes]
impl HelloController {
    #[ulo_http::get("/")]
    fn hello(&self) -> &'static str {
        "Hello, ulo!"
    }
}

#[module(controllers = [HelloController])]
pub struct AppModule;

/// The application, before a runtime is chosen: `main` and the tests each name theirs.
pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}

/// The HTTP server on smol's sockets.
pub fn server(endpoint: &str) -> ulo_http_hyper::ServerOn<ulo_listener_smol::SmolListener> {
    ulo_http_hyper::ServerOn::new(endpoint)
}
