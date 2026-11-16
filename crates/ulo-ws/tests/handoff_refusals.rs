//! A gateway on the HTTP server's port refusing under `refuse = handshake` answers as that server
//! answers its own errors and refusals: a 401 whose error names no challenge carries the server's
//! `challenge`, and a connection over `max_connections` is refused 503 with the server's
//! `shed_retry_after`, before the upgrade.

mod support;

use std::time::Duration;

use serde_json::json;
use ulo::{Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_transport::ErrorKind;
use ulo_ws::{ConnectCx, ConnectRefused, OnConnect, WsModule};

use support::{Running, hang_up, next_json, send_json, upgrade};

/// The HTTP server's challenge, which differs from the `Bearer` it sends unset.
const CHALLENGE: &str = "Basic realm=\"door\"";

/// The HTTP server's `shed_retry_after`, which differs from the one second it sends unset.
const SHED_RETRY_AFTER: Duration = Duration::from_secs(7);

/// The upgrade header naming the visitor; an upgrade without it is refused `Unauthorized`.
const USER: &str = "x-user";

/// Refuses before the upgrade, one connection at a time.
#[controller]
struct Door;

#[handlers]
#[ulo_ws::gateway(path = "/door", refuse = handshake, max_connections = 1)]
impl Door {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

impl OnConnect for Door {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        match cx.head().headers().get(USER) {
            Some(_) => Ok(()),
            None => Err(ConnectRefused::kind(ErrorKind::Unauthorized, "who are you?")),
        }
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Door>();
    }
}

async fn start() -> Running {
    let server = ulo_http_hyper::Server::new("127.0.0.1:0").challenge(CHALLENGE).shed_retry_after(SHED_RETRY_AFTER);
    Running::start(Root, server).await
}

#[tokio::test]
async fn a_handshake_401_on_the_http_port_carries_the_http_server_s_challenge() {
    let app = start().await;
    let refused = upgrade(app.addr, "/door", &[]).await;
    assert_eq!(refused.status, 401, "body: {}", refused.body);
    assert_eq!(refused.header("www-authenticate"), Some(CHALLENGE));
    app.stop().await;
}

#[tokio::test]
async fn a_connection_over_max_connections_on_the_http_port_is_refused_503_with_the_http_server_s_retry_after() {
    let app = start().await;
    let mut admitted = app.connect("/door", &[(USER, "ada")]).await;
    send_json(&mut admitted, &json!({ "event": "echo", "id": 1, "data": "in" })).await;
    assert_eq!(next_json(&mut admitted).await, json!({ "id": 1, "data": "in" }));

    let over = upgrade(app.addr, "/door", &[(USER, "bob")]).await;
    assert_eq!(over.status, 503, "body: {}", over.body);
    assert!(over.socket.is_none(), "a refusal under `refuse = handshake` switched protocols");
    assert_eq!(over.header("retry-after"), Some(SHED_RETRY_AFTER.as_secs().to_string().as_str()));
    assert_eq!(over.body, "unavailable: too many connections");
    hang_up(admitted).await;
    app.stop().await;
}
