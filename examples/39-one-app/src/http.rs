use ulo::{Dep, injectable, routes};
use ulo_http::{Json, Path};

use crate::config::AppConfig;
use crate::store::{Gate, Store};

#[injectable]
pub struct HttpApi {
    config: Dep<AppConfig>,
    store: Dep<Store>,
    gate: Dep<Gate>,
}

#[routes]
impl HttpApi {
    #[ulo_http::get("/greeting")]
    fn greeting(&self) -> String {
        self.config.greeting.clone()
    }

    #[ulo_http::post("/notes/{text}")]
    fn add(&self, Path(text): Path<String>) -> String {
        self.store.add("http", &text).to_string()
    }

    #[ulo_http::get("/notes")]
    fn notes(&self) -> Json<Vec<String>> {
        Json(self.store.notes())
    }

    #[ulo_http::get("/hold")]
    async fn hold(&self) -> &'static str {
        self.gate.hold().await;
        "held"
    }
}
