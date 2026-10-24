//! A gateway takes part in the container like any controller (capability 24): one whose
//! dependencies are all singletons is built once with the graph, one reading an extension a
//! message guard wrote is built per message, and a hand-written `Gateway` is resolved once per
//! connection, in the connection phase's execution, and kept while the connection lasts (transports
//! DESIGN §4.1).
//!
//! Every constructor numbers its instance from a counter the test holds, so "built once" is the
//! counter's value and the id each reply carries, not an inference from behaviour.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};
use ulo::{BoxError, Controller, Dep, Ext, Guard, Module, ModuleDef, ModuleIdentity, Mount, injectable, routes};
use ulo_ws::{ConnectCx, Connection, ConnectionInfo, Frame, Gateway, GatewayConfig, GatewaySettings, Port, WsConnect, WsCx};

use support::{Running, hang_up, next_json, send_json};

/// How many instances of each gateway the container has built.
#[derive(Clone, Default)]
struct Built(Arc<Counts>);

#[derive(Default)]
struct Counts {
    once: AtomicU64,
    /// The `Tenant` each instance of `Each` read when it was built, in build order.
    each: Mutex<Vec<&'static str>>,
    manual: AtomicU64,
}

impl Built {
    fn once(&self) -> u64 {
        self.0.once.load(Ordering::SeqCst)
    }

    fn each(&self) -> Vec<&'static str> {
        self.0.each.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn manual(&self) -> u64 {
        self.0.manual.load(Ordering::SeqCst)
    }
}

/// What a guard wrote into an execution's extensions: the connection phase's, and each message's.
struct Tenant(&'static str);

struct ConnectTag;

impl Guard<WsConnect> for ConnectTag {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        cx.extensions().insert(Tenant("connect"));
        Ok(true)
    }
}

struct MessageTag;

impl Guard<ulo_ws::Ws> for MessageTag {
    async fn can_activate(&self, cx: &WsCx) -> Result<bool, BoxError> {
        cx.extensions().insert(Tenant("message"));
        Ok(true)
    }
}

#[derive(Serialize)]
struct Who {
    id: u64,
    tenant: Option<&'static str>,
}

/// Reads singletons only: built with the graph.
struct Once {
    id: u64,
}

#[injectable]
impl Once {
    fn new(built: Dep<Built>) -> Self {
        Once { id: built.0.once.fetch_add(1, Ordering::SeqCst) + 1 }
    }
}

#[routes]
#[ulo_ws::gateway(path = "/once", port = own)]
impl Once {
    #[ulo_ws::message("who")]
    fn who(&self) -> Who {
        Who { id: self.id, tenant: None }
    }
}

/// Reads an extension a guard wrote, so it needs an execution. The connection phase's guard writes
/// one too, because the connect handler resolves the gateway in that execution (F468).
struct Each {
    id: u64,
    tenant: &'static str,
}

#[injectable]
impl Each {
    fn new(built: Dep<Built>, tenant: Ext<Tenant>) -> Self {
        let mut built = built.0.each.lock().unwrap_or_else(PoisonError::into_inner);
        built.push(tenant.0);
        Each { id: built.len() as u64, tenant: tenant.0 }
    }
}

#[routes]
#[ulo_ws::gateway(path = "/each", port = own, connect_guards(value = ConnectTag))]
#[guards(value = MessageTag)]
impl Each {
    #[ulo_ws::message("who")]
    fn who(&self) -> Who {
        Who { id: self.id, tenant: Some(self.tenant) }
    }
}

/// Written by hand: it reads the connection, which only the connection phase's execution has.
struct Manual {
    id: u64,
    connection: String,
}

#[injectable]
impl Manual {
    fn new(built: Dep<Built>, info: Dep<ConnectionInfo>) -> Self {
        Manual { id: built.0.manual.fetch_add(1, Ordering::SeqCst) + 1, connection: info.id().to_string() }
    }
}

impl GatewayConfig for Manual {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/manual").port(Port::Own)
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

impl Gateway for Manual {
    async fn on_message(&self, conn: &Connection, _frame: Frame) {
        let reply = json!({ "id": self.id, "connection": self.connection, "on": conn.id().to_string() });
        let _ = conn.send(Frame::text(reply.to_string())).await;
    }
}

impl Controller for Manual {
    fn mount(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

struct Root(Built);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Once>();
        m.controller::<Each>();
        m.controller::<Manual>();
    }
}

async fn start() -> (Running, Built) {
    let built = Built::default();
    let app = Running::start(Root(built.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    (app, built)
}

async fn who(socket: &mut support::Socket, id: u64) -> Value {
    send_json(socket, &json!({ "event": "who", "id": id })).await;
    let reply = next_json(socket).await;
    assert_eq!(reply["id"], json!(id), "{reply}");
    reply["data"].clone()
}

#[tokio::test]
async fn a_gateway_reading_singletons_is_built_once_with_the_graph_and_serves_every_message() {
    let (app, built) = start().await;
    assert_eq!((built.once(), built.each().len(), built.manual()), (1, 0, 0), "only the gateway with no execution-scoped read exists before a connection");

    let mut first = app.connect("/once", &[]).await;
    let mut second = app.connect("/once", &[]).await;
    let replies = [who(&mut first, 1).await, who(&mut first, 2).await, who(&mut second, 1).await];
    for reply in &replies {
        assert_eq!(reply["id"], json!(1), "every message reached the one instance: {replies:?}");
    }
    assert_eq!(built.once(), 1, "three messages on two connections built nothing more");
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}

#[tokio::test]
async fn a_gateway_reading_an_extension_is_built_for_each_message_from_what_the_guard_wrote() {
    let (app, built) = start().await;
    assert!(built.each().is_empty(), "no instance exists before a connection does");
    let mut first = app.connect("/each", &[]).await;
    let mut second = app.connect("/each", &[]).await;
    let replies = [who(&mut first, 1).await, who(&mut first, 2).await, who(&mut second, 1).await];

    // Each reply came from an instance built for that message, from the `Tenant` the message's
    // own guard wrote; the instances the connection phase built are not among them.
    let ids: Vec<u64> = replies.iter().filter_map(|reply| reply["id"].as_u64()).collect();
    assert_eq!(ids.len(), 3, "{replies:?}");
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2], "two messages shared an instance: {replies:?}");
    assert!(ids[0] < ids[1], "a connection's second message ran on a later instance than its first: {replies:?}");
    assert!(replies.iter().all(|reply| reply["tenant"] == json!("message")), "{replies:?}");
    let reads = built.each();
    assert_eq!(reads.iter().filter(|tenant| **tenant == "message").count(), 3, "one build per message: {reads:?}");
    assert_eq!(built.once(), 1, "the other gateway is untouched");
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}

#[tokio::test]
async fn a_hand_written_gateway_is_resolved_once_per_connection_and_kept_while_it_lasts() {
    let (app, built) = start().await;
    assert_eq!(built.manual(), 0, "nothing is resolved before a connection exists");

    let mut first = app.connect("/manual", &[]).await;
    let mut second = app.connect("/manual", &[]).await;
    let mut asked = Vec::new();
    for on_first in [true, true, true, false] {
        let socket = if on_first { &mut first } else { &mut second };
        send_json(socket, &json!({ "ask": true })).await;
        asked.push(next_json(socket).await);
    }

    assert_eq!(built.manual(), 2, "one instance for each of two connections, not one per message");
    let on_first = &asked[0];
    assert_eq!(asked[1], *on_first, "the first connection's second message ran on its first message's instance");
    assert_eq!(asked[2], *on_first);
    let on_second = &asked[3];
    assert_ne!(on_second["id"], on_first["id"], "the second connection has an instance of its own");
    assert_eq!(on_first["connection"], on_first["on"], "the instance read its own connection's id");
    assert_eq!(on_second["connection"], on_second["on"]);
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}
