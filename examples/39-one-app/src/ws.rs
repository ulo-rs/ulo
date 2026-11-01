use ulo::{Dep, injectable, routes};
use ulo_ws::Payload;

use crate::store::{Gate, Store};

#[injectable]
pub struct WsApi {
    store: Dep<Store>,
    gate: Dep<Gate>,
}

#[routes]
#[ulo_ws::gateway(path = "/notes", port = own)]
impl WsApi {
    #[ulo_ws::message("add")]
    fn add(&self, text: Payload<String>) -> u32 {
        self.store.add("ws", &text.0)
    }

    #[ulo_ws::message("hold")]
    async fn hold(&self) -> &'static str {
        self.gate.hold().await;
        "held"
    }
}
