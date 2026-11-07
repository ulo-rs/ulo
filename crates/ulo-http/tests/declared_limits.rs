//! A capability a backend or host is declared without refuses, at `listen()`, what needs it, and
//! the refusal names the reason the declaration carries. A backend's declaration is read from the
//! backend value, so it can follow the backend's own configuration.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata};
use ulo::{App, AppHandle, BoxError, BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, StartupError, controller, handlers};
use ulo_http::embed::{Embed, EmbedLimits, Embedded, Miss, Service};
use ulo_http::{AppService, Backend, BackendLimits, ClientAddr, ConnInfo, Host, HttpBody, HttpConfig, Request, Response, UpgradeHandler, Upgrades};
use ulo_net::{BoundListener, Endpoint, Tls};
use ulo_net::rustls::ServerConfig;
use ulo_runtime_tokio::Tokio;

const NO_PEER: &str = "the test host hands over no peer address";
const NO_FORWARD: &str = "the test host routes a request once";
const NO_EXTENSIONS: &str = "the test host keeps its request store to itself";
const NO_H2C: &str = "the test backend was built without cleartext HTTP/2";
const NO_UPGRADES: &str = "the test hands over no upgraded I/O";
const NO_PORT_ZERO: &str = "the test backend binds only the port it is given";
const NO_INHERITED: &str = "the test backend opens its own sockets";
const NO_TLS: &str = "the test backend serves cleartext alone";
const NO_STRIP: &str = "the test host hands over every path whole";

/// A host declared without the four capabilities `prepare` checks against an app's reads.
struct Lacking;

impl Embed for Lacking {
    const NAME: &'static str = "lacking";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
            .without_peer_addr(NO_PEER)
            .without_forward_miss(NO_FORWARD)
            .without_host_extensions(NO_EXTENSIONS)
            .without_upgrades(NO_UPGRADES)
    }
}

#[derive(Clone)]
struct Tenant;

#[controller]
struct Peer {
    addr: Dep<ClientAddr>,
}

#[handlers]
impl Peer {
    #[ulo_http::get("/peer")]
    fn peer(&self) -> String {
        self.addr.0.to_string()
    }

    #[ulo_http::get("/tenant")]
    fn tenant(&self, _tenant: Host<Tenant>) -> &'static str {
        "tenant"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Peer>();
        m.meta::<Upgrades>().register(Taker);
    }
}

/// Takes upgrades on `/socket`, so the app needs a host or backend that hands one over.
struct Taker;

impl UpgradeHandler for Taker {
    fn paths(&self, _app: &AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/socket".into()]
    }

    fn upgrade(&self, _req: Request) -> BoxFuture<'static, Response> {
        Box::pin(async { Response::new(HttpBody::from("taken")) })
    }
}

#[tokio::test]
async fn a_host_s_refusals_name_the_reasons_it_declared() {
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let server = Embedded::<Lacking>::new().on_miss(Miss::Forward);
    let Err(error) = app.bind(server).listen().await else { panic!("listen() accepted reads the host is declared without") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
    let text = error.to_string();
    for (capability, reason) in
        [("peer address", NO_PEER), ("`forward_miss`", NO_FORWARD), ("`host_extensions`", NO_EXTENSIONS), ("`upgrades`", NO_UPGRADES)]
    {
        assert!(text.contains(capability) && text.contains(reason), "the refusal of the {capability} read names `{reason}`: {text}");
    }
}

/// A backend whose `h2c` follows how it was built.
struct Built {
    h2c: bool,
}

impl Backend for Built {
    const NAME: &'static str = "built";

    fn limits(&self) -> BackendLimits {
        if self.h2c { BackendLimits::NONE } else { BackendLimits::NONE.without_h2c(NO_H2C) }
    }

    async fn bind(&mut self, _: Vec<BoundListener>, _: Option<Arc<ServerConfig>>, _: AppService, _: &HttpConfig) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Ok(())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

async fn listens_with_h2c(backend: Built) -> Result<(), StartupError> {
    let app = App::builder(Empty).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    app.bind(ulo_http::Server::with_backend("127.0.0.1:0", backend).h2c(true)).listen().await.map(drop)
}

#[tokio::test]
async fn a_backend_s_declaration_follows_the_backend_value() {
    let Err(error) = listens_with_h2c(Built { h2c: false }).await else { panic!("listen() accepted h2c on a backend built without it") };
    let text = error.to_string();
    assert!(text.contains("declared without `h2c`") && text.contains(NO_H2C), "the refusal names the declared reason: {text}");
    listens_with_h2c(Built { h2c: true }).await.expect("a backend built with h2c takes it");
}

/// A backend declared without every capability `prepare` checks against the server's settings.
struct Bare;

impl Backend for Bare {
    const NAME: &'static str = "bare";

    fn limits(&self) -> BackendLimits {
        BackendLimits::NONE
            .without_upgrades(NO_UPGRADES)
            .without_port_zero(NO_PORT_ZERO)
            .without_inherited_sockets(NO_INHERITED)
            .without_tls(NO_TLS)
    }

    async fn bind(&mut self, _: Vec<BoundListener>, _: Option<Arc<ServerConfig>>, _: AppService, _: &HttpConfig) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Ok(())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_backend_s_refusals_name_the_reasons_it_declared() {
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let server = ulo_http::Server::with_backend("127.0.0.1:0", Bare)
        .endpoint(Endpoint::inherited("http"))
        .tls(Tls::from_pem(Vec::new(), Vec::new()));
    let Err(error) = app.bind(server).listen().await else { panic!("listen() accepted what the backend is declared without") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
    let text = error.to_string();
    for (capability, reason) in [
        ("`upgrades`", NO_UPGRADES),
        ("`port_zero`", NO_PORT_ZERO),
        ("`inherited_sockets`", NO_INHERITED),
        ("`tls`", NO_TLS),
    ] {
        assert!(
            text.contains(&format!("declared without {capability}")) && text.contains(reason),
            "the refusal of {capability} names `{reason}`: {text}"
        );
    }
}

/// A host that leaves the mount prefix on the path.
struct Whole;

impl Embed for Whole {
    const NAME: &'static str = "whole";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE.without_strips_prefix(NO_STRIP)
    }
}

/// Every event's fields, as `name=value` text, one string per event.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = String::new();
        event.record(&mut Fields(&mut line));
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(line);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

#[tokio::test]
async fn the_warn_for_a_path_outside_the_prefix_names_the_declared_reason() {
    let server = Embedded::<Whole>::new().nested_at("/app");
    let handle = server.handle();
    let _bound = App::builder(Empty)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the app listens");
    let service: Service<Whole> = handle.service();
    let capture = Capture::default();
    let response = {
        let _guard = tracing::subscriber::set_default(capture.clone());
        let (head, ()) = http::Request::get("/elsewhere").body(()).expect("a request").into_parts();
        let req = Request { head: head.clone(), body: HttpBody::from(""), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
        service.respond(&head, req).await
    };
    assert_eq!(response.status(), 404, "a path outside the prefix is the app's 404");
    let events = capture.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let warned = format!("reason={NO_STRIP:?}");
    assert!(events.iter().any(|event| event.contains(&warned)), "no event carries `{warned}`: {events:?}");
}
