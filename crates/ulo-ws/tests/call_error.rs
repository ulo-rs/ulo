//! A boxed `ConnectRefused` keeps its kind and its close code through `ulo_ws::call_error` and
//! through the renderer of a message's error, where `CallError::from_boxed` alone answers it
//! `Internal`.

mod support;

use futures_util::{Stream, stream};
use serde_json::json;
use ulo::{BoxError, ErrorHandler, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_transport::{CallError, ErrorKind};
use ulo_ws::{CloseCode, ConnectRefused, Reply, WithCloseCode, Ws, WsCx, WsModule};

use support::{Running, hang_up, next_json, send_json};

fn refusal() -> (CloseCode, ConnectRefused) {
    let code = CloseCode::new(4403).expect("an application's code");
    (code, ConnectRefused::code(code, "not a member").expect("a reason within 123 bytes"))
}

#[test]
fn the_helper_keeps_a_refusal_s_code_and_kind() {
    let (code, refused) = refusal();
    let call = ulo_ws::call_error(Box::new(refused.clone()));
    assert_eq!((call.kind(), call.close_code(), call.message()), (ErrorKind::Forbidden, Some(code), "not a member"));
    assert_eq!(call.source_as::<ConnectRefused>(), Some(&refused), "the refusal stays the source");
    assert_eq!(ConnectRefused::from(call).close_code(), code, "a rebuilt refusal closes with the same code");

    let kinded = ulo_ws::call_error(Box::new(ConnectRefused::kind(ErrorKind::NotFound, "no room")));
    assert_eq!((kinded.kind(), kinded.message()), (ErrorKind::NotFound, "no room"), "the kind's prefix is not the message");
    let rebuilt = ConnectRefused::from(kinded);
    assert_eq!((rebuilt.close_code(), rebuilt.reason()), (CloseCode::of_kind(ErrorKind::NotFound), "not_found: no room"));
    for reason in ["", "not_found", "not_found: not_found", "not_found: "] {
        let refused = ConnectRefused::kind(ErrorKind::NotFound, reason);
        let rebuilt = ConnectRefused::from(ulo_ws::call_error(Box::new(refused.clone())));
        assert_eq!((rebuilt.close_code(), rebuilt.reason()), (refused.close_code(), refused.reason()), "{reason:?}");
    }
    let coded = ConnectRefused::from(CallError::new(ErrorKind::NotFound, "bare").with_close_code(CloseCode::of_kind(ErrorKind::NotFound)));
    assert_eq!(ConnectRefused::from(ulo_ws::call_error(Box::new(coded.clone()))).reason(), coded.reason(), "a reason with no prefix");
}

#[test]
fn from_boxed_alone_loses_the_code_and_any_other_error_falls_through() {
    let (_, refused) = refusal();
    let bare = CallError::from_boxed(Box::new(refused));
    assert_eq!((bare.kind(), bare.close_code()), (ErrorKind::Internal, None));

    let own = ulo_ws::call_error(Box::new(CallError::new(ErrorKind::Conflict, "taken")));
    assert_eq!((own.kind(), own.message()), (ErrorKind::Conflict, "taken"));
    let other = ulo_ws::call_error("plain".into());
    assert_eq!(other.kind(), ErrorKind::Internal);
}

/// Reshapes every error into a boxed refusal of kind `Conflict`.
struct Reshape;

impl ErrorHandler<Ws> for Reshape {
    async fn handle(&self, _err: BoxError, _cx: &WsCx) -> Result<Reply, BoxError> {
        Err(Box::new(ConnectRefused::kind(ErrorKind::Conflict, "taken")))
    }
}

#[controller]
struct Refuses;

#[handlers]
#[ulo_ws::gateway(path = "/refuses")]
impl Refuses {
    #[ulo_ws::message("refuse")]
    fn refuse(&self) -> Result<String, ConnectRefused> {
        Err(ConnectRefused::kind(ErrorKind::NotFound, "no room"))
    }

    #[ulo_ws::message("streamed")]
    #[error_handlers(value = Reshape)]
    fn streamed(&self) -> impl Stream<Item = Result<u32, CallError>> + use<> {
        stream::iter([Ok(1), Err(CallError::new(ErrorKind::Unavailable, "gone"))])
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Refuses>();
    }
}

/// Planted break: the message's renderer calling `CallError::from_boxed` instead of the helper,
/// which answers the refusal `internal`.
#[tokio::test]
async fn a_refusal_a_message_returns_is_sent_as_its_kind() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/refuses", &[]).await;
    send_json(&mut socket, &json!({ "event": "refuse", "id": 1 })).await;
    let answer = next_json(&mut socket).await;
    assert_eq!((answer["error"]["kind"].as_str(), answer["error"]["message"].as_str()), (Some("not_found"), Some("no room")), "{answer}");
    hang_up(socket).await;
    app.stop().await;
}

/// Planted break: the late path's renderer calling `CallError::from_boxed` on what the error
/// handlers left, which answers the reshaped refusal `internal`.
#[tokio::test]
async fn a_refusal_an_error_handler_reshapes_a_stream_item_into_is_sent_as_its_kind() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/refuses", &[]).await;
    send_json(&mut socket, &json!({ "event": "streamed", "id": 1 })).await;
    let first = next_json(&mut socket).await;
    assert_eq!(first["data"], 1, "{first}");
    let answer = next_json(&mut socket).await;
    assert_eq!((answer["error"]["kind"].as_str(), answer["error"]["message"].as_str()), (Some("conflict"), Some("taken")), "{answer}");
    hang_up(socket).await;
    app.stop().await;
}
