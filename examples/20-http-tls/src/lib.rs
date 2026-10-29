use ulo::{App, AppBuilder, injectable, module, routes};
use ulo_http_hyper::Server;
use ulo_net::Tls;

#[injectable]
pub struct HelloController;

#[routes]
impl HelloController {
    #[ulo_http::get("/")]
    fn hello(&self) -> &'static str {
        "Hello over TLS"
    }
}

#[module(controllers = [HelloController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}

/// The server: every endpoint speaks TLS with this certificate chain and key, offering `h2` and
/// `http/1.1`. The files are read, and a bad pair refused, when the app listens.
pub fn server(endpoint: &str, cert: &str, key: &str) -> Server {
    Server::new(endpoint).tls(Tls::from_pem_files(cert, key))
}
