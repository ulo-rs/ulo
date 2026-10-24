//! A gateway takes part in the container like any controller (capability 24): one whose
//! dependencies are all singletons is built once with the graph, one reading an extension a
//! message guard wrote is built per message, and a hand-written `Gateway` is resolved once per
//! connection, in the connection phase's execution, and kept while the connection lasts (transports
//! DESIGN §4.1). An attributed gateway implementing no `OnConnect` is not built in the connection
//! phase at all: one reading an extension only a message guard writes connects, and one built per
//! execution is built for its messages alone (F468).
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

use tokio_tungstenite::tungstenite::Message;

use support::{Running, hang_up, next_json, next_message, send_json};

/// How many instances of each gateway the container has built.
#[derive(Clone, Default)]
struct Built(Arc<Counts>);

#[derive(Default)]
struct Counts {
    once: AtomicU64,
    /// The `Tenant` each instance of `Each` read when it was built, in build order.
    each: Mutex<Vec<&'static str>>,
    manual: AtomicU64,
    /// The `Tenant` each instance of `MessageOnly` read when it was built, in build order.
    message_only: Mutex<Vec<&'static str>>,
    counted: AtomicU64,
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

    fn message_only(&self) -> Vec<&'static str> {
        self.0.message_only.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn counted(&self) -> u64 {
        self.0.counted.load(Ordering::SeqCst)
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

/// Reads an extension a guard wrote, so it needs an execution. Its connect guard writes one too,
/// which no build reads: the gateway implements no `OnConnect`, so the connection phase does not
/// build it.
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

/// Reads an extension only its message guard writes: the connection phase's execution has none.
struct MessageOnly {
    id: u64,
    tenant: &'static str,
}

#[injectable]
impl MessageOnly {
    fn new(built: Dep<Built>, tenant: Ext<Tenant>) -> Self {
        let mut built = built.0.message_only.lock().unwrap_or_else(PoisonError::into_inner);
        built.push(tenant.0);
        MessageOnly { id: built.len() as u64, tenant: tenant.0 }
    }
}

#[routes]
#[ulo_ws::gateway(path = "/message-only", port = own)]
#[guards(value = MessageTag)]
impl MessageOnly {
    #[ulo_ws::message("who")]
    fn who(&self) -> Who {
        Who { id: self.id, tenant: Some(self.tenant) }
    }
}

/// Built per execution from singletons alone, so every build is one the counter records. The
/// connection phase runs before the 101 under `refuse = handshake`, so it has ended by the time
/// the upgrade returns.
struct Counted {
    id: u64,
}

#[injectable(execution)]
impl Counted {
    fn new(built: Dep<Built>) -> Self {
        Counted { id: built.0.counted.fetch_add(1, Ordering::SeqCst) + 1 }
    }
}

#[routes]
#[ulo_ws::gateway(path = "/counted", port = own, refuse = handshake)]
impl Counted {
    #[ulo_ws::message("who")]
    fn who(&self) -> Who {
        Who { id: self.id, tenant: None }
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
        m.controller::<MessageOnly>();
        m.controller::<Counted>();
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
    assert_eq!(
        (built.once(), built.each().len(), built.message_only().len(), built.counted(), built.manual()),
        (1, 0, 0, 0, 0),
        "only the gateway with no execution-scoped read exists before a connection"
    );

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
    // own guard wrote.
    let ids: Vec<u64> = replies.iter().filter_map(|reply| reply["id"].as_u64()).collect();
    assert_eq!(ids.len(), 3, "{replies:?}");
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2], "two messages shared an instance: {replies:?}");
    assert!(ids[0] < ids[1], "a connection's second message ran on a later instance than its first: {replies:?}");
    assert!(replies.iter().all(|reply| reply["tenant"] == json!("message")), "{replies:?}");
    assert_eq!(built.each(), ["message", "message", "message"], "one build per message, and none from the connect guard's `Tenant`");
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

/// [`who`], reading the frame that answers it whatever it is, so a connection the server closed
/// fails on the Close frame rather than on a parse.
async fn answered(socket: &mut support::Socket, id: u64) -> Value {
    send_json(socket, &json!({ "event": "who", "id": id })).await;
    let frame = next_message(socket).await;
    assert!(!matches!(frame, Message::Close(_)), "the connection was closed instead of answered: {frame:?}");
    let Message::Text(text) = &frame else { panic!("expected the reply as text, got {frame:?}") };
    let reply: Value = serde_json::from_str(text).unwrap_or_else(|error| panic!("the reply {text:?} is not JSON: {error}"));
    assert_eq!(reply["id"], json!(id), "{reply}");
    reply["data"].clone()
}

#[tokio::test]
async fn a_gateway_reading_an_extension_only_a_message_guard_writes_connects_and_is_built_per_message() {
    let (app, built) = start().await;
    let mut first = app.connect("/message-only", &[]).await;
    let mut second = app.connect("/message-only", &[]).await;

    let replies = [answered(&mut first, 1).await, answered(&mut first, 2).await, answered(&mut second, 1).await];
    let ids: Vec<u64> = replies.iter().filter_map(|reply| reply["id"].as_u64()).collect();
    assert_eq!(ids, [1, 2, 3], "each message ran on the instance built for it, in order: {replies:?}");
    assert!(replies.iter().all(|reply| reply["tenant"] == json!("message")), "{replies:?}");
    assert_eq!(built.message_only(), ["message", "message", "message"], "one build per message and none at connect");
    hang_up(first).await;
    hang_up(second).await;
    app.stop().await;
}

#[tokio::test]
async fn a_connection_to_a_gateway_with_no_connection_hook_builds_no_instance_of_it() {
    let (app, built) = start().await;
    let mut first = app.connect("/counted", &[]).await;
    let second = app.connect("/counted", &[]).await;
    assert_eq!(built.counted(), 0, "two connection phases ended and built nothing");

    let reply = who(&mut first, 1).await;
    assert_eq!(reply["id"], json!(1), "the message's instance is the first one built: {reply}");
    assert_eq!(built.counted(), 1, "one message, one build");
    hang_up(first).await;
    hang_up(second).await;
    assert_eq!(built.counted(), 1, "the connections ended and built nothing");
    app.stop().await;
}
