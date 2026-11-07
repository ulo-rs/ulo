//! Opened by `scripts/editor_probe.py`: every pathed attribute below must answer hover and
//! go-to-definition at its first and last path segment, and every marker attribute at its last
//! and, written `ulo::<name>` or `::ulo::<name>`, the one qualifier a marker accepts, at `ulo`.
//!
//! Each macro that writes its item back through `syn` restores that item's attributes, so each
//! carries one attribute only it hands on: the `#[ulo::injectable]` structs inside the handler
//! bodies and the constructor go through `#[handlers]` and then the transport attribute or
//! `#[injectable]`, and the two attributes on `M` and `N` through `#[module]` and `#[injectable]`.
//!
//! The markers `#[handlers]`, `#[injectable]` and `#[controller]` read and remove answer through
//! the hidden import each macro writes for them: below, on an impl and on its methods before and
//! after the transport attribute, bare, pathed, inside `cfg_attr` and repeated, in a fn body and on
//! a generic impl, and as `#[construct]` and a field's `#[injectable(default)]` or
//! `#[controller(default)]`, the `#[construct]` of a controller included where `#[controller]` and
//! `#[handlers]` stack on one impl, in either order.

use std::marker::PhantomData;

use ulo::{BoxError, Guard, controller, handlers, injectable, module};
use ulo_http::{Http, HttpCx};
use ulo_rpc::Payload;

#[controller]
pub struct Users;

#[handlers]
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

#[controller]
pub struct Billing;

#[handlers]
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

#[controller]
pub struct Chat;

#[handlers]
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

pub struct G;

impl Guard<Http> for G {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

pub struct Stamp(pub &'static str);

#[controller]
pub struct Marked {
    #[controller(default)]
    count: u32,
    #[ulo::controller(default)]
    total: u32,
}

#[handlers]
#[ulo::guards(value = G)]
#[ulo::interceptors()]
#[ulo::error_handlers()]
#[ulo::meta(Stamp("impl"))]
#[guards(value = G)]
#[meta(Stamp("bare"))]
impl Marked {
    #[ulo::guards(value = G)]
    #[guards(value = G)]
    #[::ulo::guards(value = G)]
    #[ulo_http::get("/before")]
    #[ulo::interceptors()]
    #[error_handlers()]
    #[meta(Stamp("after"))]
    fn before_and_after(&self) -> String {
        format!("{}{}", self.count, self.total)
    }

    #[cfg_attr(all(), ulo::guards(value = G), meta(Stamp("gated")))]
    #[ulo_http::get("/gated")]
    #[guards(value = G)]
    fn gated(&self) -> &'static str {
        "gated"
    }
}

#[controller]
pub struct Generic<T: Send + Sync + 'static> {
    #[controller(default)]
    of: PhantomData<fn() -> T>,
}

#[handlers]
#[guards(value = G)]
impl<T: Send + Sync + 'static> Generic<T> {
    #[ulo_http::get("/generic")]
    #[ulo::meta(Stamp("generic"))]
    fn generic(&self) -> &'static str {
        let _ = &self.of;
        "generic"
    }
}

pub fn in_a_body() {
    #[controller]
    struct Local;

    #[handlers]
    #[guards(value = G)]
    impl Local {
        #[ulo_http::get("/local")]
        #[guards(value = G)]
        fn local(&self) -> &'static str {
            "local"
        }
    }
}

#[injectable]
pub struct Defaults {
    #[injectable(default)]
    count: u32,
    #[ulo::injectable(default)]
    total: u32,
}

impl Defaults {
    pub fn sum(&self) -> u32 {
        self.count + self.total
    }
}

pub struct Built;

#[injectable]
impl Built {
    #[ulo::construct]
    pub fn build() -> Self {
        Built
    }
}

pub struct BuiltBare;

#[injectable]
impl BuiltBare {
    #[construct]
    pub fn build() -> Self {
        BuiltBare
    }
}

pub struct Counter;

#[controller]
#[handlers]
impl Counter {
    #[construct]
    pub fn build() -> Self {
        Counter
    }

    #[ulo_http::get("/counter")]
    #[guards(value = G)]
    fn count(&self) -> &'static str {
        "counter"
    }
}

pub struct Tally;

#[handlers]
#[controller]
impl Tally {
    #[ulo::construct]
    pub fn build() -> Self {
        Tally
    }

    #[ulo_http::get("/tally")]
    #[guards(value = G)]
    fn tally(&self) -> &'static str {
        "tally"
    }
}
