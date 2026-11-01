//! A message is one execution and a connection is not: an execution-scoped service is built once
//! per message and shared by everything that message reads, a connection's id is the same across
//! its messages, and what a connect guard writes into the connect phase's extensions reaches
//! `OnConnect`. Capability: WebSocket execution scope (G25).

mod support;

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::{Value, json};
use ulo::{BoxError, Dep, Guard, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_ws::{ConnectCx, ConnectRefused, ConnectionInfo, OnConnect, WsConnect};

use support::{Record, Running, hang_up, next_json, send_json};

/// Numbers each `Stamp` the container builds.
struct Counter(AtomicU64);

/// Built once per message: the number it was given.
struct Stamp(u64);

/// Built once per message from the message's `Stamp`.
struct Wrapper(Dep<Stamp>);

#[derive(Serialize)]
struct Ids {
    direct: u64,
    through_wrapper: u64,
    connection: ulo_ws::ConnId,
}

/// What a connect guard learned from the upgrade.
struct Tenant(&'static str);

struct Tag;

impl Guard<WsConnect> for Tag {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        cx.extensions().insert(Tenant("acme"));
        Ok(true)
    }
}

/// What `on_connect` read from the guard's extensions.
#[derive(Clone)]
struct Seen(Record<&'static str>);

#[injectable]
struct Scoped {
    seen: Dep<Seen>,
}

#[routes]
#[ulo_ws::gateway(path = "/scoped", port = own, connect_guards(value = Tag))]
impl Scoped {
    #[ulo_ws::message("who")]
    fn who(&self, direct: Dep<Stamp>, wrapped: Dep<Wrapper>, conn: Dep<ConnectionInfo>) -> Ids {
        Ids { direct: direct.0, through_wrapper: wrapped.0.0, connection: conn.id() }
    }
}

impl OnConnect for Scoped {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        let tenant = cx.ext::<Tenant>().map(|tenant| tenant.0).unwrap_or("missing");
        self.seen.0.push(tenant);
        Ok(())
    }
}

struct Root(Seen);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.value(Counter(AtomicU64::new(1)));
        m.execution(|counter: Dep<Counter>| async move { Stamp(counter.0.fetch_add(1, Ordering::SeqCst)) });
        m.execution(|stamp: Dep<Stamp>| async move { Wrapper(stamp) });
        m.controller::<Scoped>();
    }
}

async fn start() -> (Running, Seen) {
    let seen = Seen(Record::new());
    let app = Running::start(Root(seen.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    (app, seen)
}

async fn who(socket: &mut support::Socket, id: u64) -> Value {
    send_json(socket, &json!({ "event": "who", "id": id })).await;
    let reply = next_json(socket).await;
    assert_eq!(reply["id"], json!(id), "{reply}");
    reply["data"].clone()
}

#[tokio::test]
async fn a_message_s_handler_and_the_services_it_reads_share_one_execution_scoped_instance() {
    let (app, _) = start().await;
    let mut socket = app.connect("/scoped", &[]).await;
    let ids = who(&mut socket, 1).await;
    assert_eq!(ids["direct"], ids["through_wrapper"], "one message built two `Stamp`s: {ids}");
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn each_message_is_its_own_execution_while_the_connection_stays_the_same() {
    let (app, _) = start().await;
    let mut first = app.connect("/scoped", &[]).await;
    let one = who(&mut first, 1).await;
    let two = who(&mut first, 2).await;
    assert_ne!(one["direct"], two["direct"], "two messages on one connection shared a `Stamp`: {one} {two}");
    assert_eq!(one["connection"], two["connection"], "a connection's id changed between its messages");

    let mut second = app.connect("/scoped", &[]).await;
    let other = who(&mut second, 1).await;
    assert_ne!(other["connection"], one["connection"], "two connections reported one id");
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}

#[tokio::test]
async fn what_a_connect_guard_writes_into_the_extensions_reaches_on_connect() {
    let (app, seen) = start().await;
    let mut socket = app.connect("/scoped", &[]).await;
    let _ = who(&mut socket, 1).await;
    assert_eq!(seen.0.snapshot(), vec!["acme"], "`on_connect` did not read the guard's `Tenant`");
    hang_up(socket).await;
    app.stop().await;
}
