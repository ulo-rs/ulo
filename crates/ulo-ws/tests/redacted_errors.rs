//! A `CallError`'s message and details reach a WebSocket client through the app's redaction: a
//! registered secret's value is sent as `[redacted]` in a message's error envelope and in the close
//! reason of a refused connection. A secret under four characters is not matched, and a message
//! holding no secret is sent byte for byte.

mod support;

use serde_json::json;
use ulo::{BoxError, Guard, Module, ModuleDef, ModuleIdentity, Secret, controller, handlers};
use ulo_transport::{CallError, Detail, Details, ErrorKind};
use ulo_ws::{ConnectCx, Payload, WsConnect, WsModule};

use support::{Running, close_frame, hang_up, next_json, send_json};

const SECRET: &str = "s3cr3t-token-xyz";
const SHORT: &str = "abc";

/// Refuses a connection carrying `x-refuse` with the secret in the error's message.
struct Refuses;

impl Guard<WsConnect> for Refuses {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        match cx.head().headers().get("x-refuse") {
            Some(_) => Err(CallError::new(ErrorKind::Forbidden, format!("key {SECRET} refused")).into()),
            None => Ok(true),
        }
    }
}

#[controller]
struct Leaks;

#[handlers]
#[ulo_ws::gateway(path = "/leaks", connect_guards(value = Refuses))]
impl Leaks {
    /// Fails with `text` as the message, the secret also in a `Json` detail when `text` is `secret`.
    #[ulo_ws::message("fail")]
    fn fail(&self, text: Payload<String>) -> Result<String, CallError> {
        Err(match text.0.as_str() {
            "secret" => CallError::new(ErrorKind::NotFound, format!("key {SECRET} refused"))
                .with_details(Details::new().with(Detail::Json(json!({ "tried": SECRET })))),
            "short" => CallError::new(ErrorKind::NotFound, format!("key {SHORT} refused")),
            other => CallError::new(ErrorKind::NotFound, other.to_owned()),
        })
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.secret(&Secret::new(SECRET));
        m.secret(&Secret::new(SHORT));
        m.import(WsModule::for_root());
        m.controller::<Leaks>();
    }
}

/// Planted break: the message's error envelope written unredacted.
#[tokio::test]
async fn a_registered_secret_in_a_message_s_error_is_sent_redacted_and_a_short_one_is_not() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/leaks", &[]).await;
    let answers = [
        ("secret", json!({ "kind": "not_found", "message": "key [redacted] refused", "details": [{ "type": "json", "value": { "tried": "[redacted]" } }] })),
        ("short", json!({ "kind": "not_found", "message": format!("key {SHORT} refused"), "details": [] })),
        ("the room is full", json!({ "kind": "not_found", "message": "the room is full", "details": [] })),
    ];
    for (n, (text, error)) in answers.into_iter().enumerate() {
        send_json(&mut socket, &json!({ "event": "fail", "id": n, "data": text })).await;
        assert_eq!(next_json(&mut socket).await, json!({ "id": n, "error": error }), "{text}");
    }
    hang_up(socket).await;
    app.stop().await;
}

/// Planted break: the connect refusal's reason sent as the error wrote it.
#[tokio::test]
async fn a_registered_secret_in_a_connect_refusal_s_reason_is_sent_redacted() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/leaks", &[("x-refuse", "1")]).await;
    assert_eq!(close_frame(&mut socket).await, Some((3003, "forbidden: key [redacted] refused".to_owned())));
    hang_up(socket).await;
    app.stop().await;
}
