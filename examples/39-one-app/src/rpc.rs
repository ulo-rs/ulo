use ulo::{Dep, injectable, routes};
use ulo_rpc::Payload;

use crate::store::{Gate, Store};

#[injectable]
pub struct RpcApi {
    store: Dep<Store>,
    gate: Dep<Gate>,
}

#[routes]
impl RpcApi {
    #[ulo_rpc::message("notes.add")]
    fn add(&self, text: Payload<String>) -> u32 {
        self.store.add("rpc", &text.0)
    }

    #[ulo_rpc::message("notes.hold")]
    async fn hold(&self) -> String {
        self.gate.hold().await;
        "held".to_owned()
    }
}
