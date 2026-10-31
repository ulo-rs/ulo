use ulo::{App, AppBuilder, injectable, module, routes};
use ulo_http_hyper::Server;
use ulo_net::Endpoint;

#[injectable]
pub struct HelloController;

#[routes]
impl HelloController {
    #[ulo_http::get("/")]
    fn hello(&self) -> &'static str {
        "Hello from a socket the app did not open"
    }
}

#[module(controllers = [HelloController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}

/// Where the server listens: the first socket the supervisor handed over (descriptor 3, named by
/// `LISTEN_FDS`) when `activated`, else `127.0.0.1:port`, which the app binds itself.
pub fn server(activated: bool, port: &str) -> Server {
    if activated {
        Server::new(Endpoint::inherited_index(0))
    } else {
        Server::new(format!("127.0.0.1:{port}"))
    }
}
