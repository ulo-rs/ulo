use ulo::{App, AppBuilder, controller, handlers, module};

#[controller]
pub struct HelloController;

#[handlers]
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
