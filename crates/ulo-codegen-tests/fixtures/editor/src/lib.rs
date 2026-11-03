//! Opened by `scripts/editor_probe.py`: every pathed attribute below must answer hover and
//! go-to-definition at its first and last path segment.
//!
//! Each macro that writes its item back through `syn` restores that item's attributes, so each
//! carries one attribute only it hands on: the `#[ulo::injectable]` structs inside the handler
//! bodies and the constructor go through `#[routes]` and then the transport attribute or
//! `#[injectable]`, and the two attributes on `M` and `N` through `#[module]` and `#[injectable]`.

use ulo::{injectable, module, routes};
use ulo_rpc::Payload;

#[injectable]
pub struct Users;

#[routes]
impl Users {
    #[ulo_http::get("/x")]
    fn show(&self) -> String {
        #[ulo::injectable]
        struct InHttp;
        let _ = InHttp;
        String::new()
    }

    #[ulo_http::post("/y")]
    fn create(&self) -> String {
        String::new()
    }
}

#[injectable]
pub struct Billing;

#[routes]
impl Billing {
    #[ulo_rpc::message("invoice.create")]
    fn create(&self, input: Payload<String>) -> String {
        #[ulo::injectable]
        struct InRpc;
        let _ = InRpc;
        input.0
    }

    #[ulo_rpc::event("invoice.paid")]
    fn paid(&self, input: Payload<String>) {
        let _ = input;
    }
}

#[injectable]
pub struct Chat;

#[routes]
#[ulo_ws::gateway(path = "/chat")]
impl Chat {
    #[ulo_ws::message("say")]
    fn say(&self, text: ulo_ws::Payload<String>) -> String {
        #[ulo::injectable]
        struct InWs;
        let _ = InWs;
        text.0
    }
}

pub struct Ledger;

#[injectable]
impl Ledger {
    pub fn new() -> Self {
        #[ulo::injectable]
        struct InConstructor;
        let _ = InConstructor;
        Ledger
    }
}

#[module(providers = [])]
#[ulo::injectable]
pub struct M;

#[injectable]
#[ulo::module(providers = [])]
pub struct N;
