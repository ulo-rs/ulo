//! A guard's refusal is one event in both of a gateway's phases: the connect guard's and the
//! message guard's `Ok(false)`, from one guard type, reach the global error handlers of their phase
//! as the same `GuardRejected` naming that guard, and unclaimed render the one kind both phases
//! share, `forbidden`: a Close with 3003 for the connection, the `forbidden` envelope for the
//! message.

mod support;

use std::sync::Arc;

use serde_json::json;
use ulo::{AnyErrorHandler, BoxError, ErrorHandler, Guard, GuardRejected, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_ws::{ConnectCx, ConnectReply, Reply, Ws, WsConnect, WsCx, WsModule};

use support::{Record, Running, close_frame, hang_up, next_json, send_json};

/// The upgrade header whose presence the connect guard refuses.
const REFUSE: &str = "x-refuse";

/// Refuses a connection whose upgrade carries [`REFUSE`], and every message it guards.
struct Deny;

impl Guard<WsConnect> for Deny {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(!cx.head().headers().contains_key(REFUSE))
    }
}

impl Guard<Ws> for Deny {
    async fn can_activate(&self, _cx: &WsCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

/// The phase each global error handler ran in and the guard the `GuardRejected` it was offered
/// names, `None` for any other error.
#[derive(Clone)]
struct Offered(Record<(&'static str, Option<&'static str>)>);

fn rejected_by(err: &BoxError) -> Option<&'static str> {
    err.downcast_ref::<GuardRejected>().map(|rejected| rejected.guard)
}

/// Records what it is offered and hands it on.
struct ObserveConnect(Offered);

impl ErrorHandler<WsConnect> for ObserveConnect {
    async fn handle(&self, err: BoxError, _cx: &ConnectCx) -> Result<ConnectReply, BoxError> {
        self.0.0.push(("connect", rejected_by(&err)));
        Err(err)
    }
}

/// Records what it is offered and hands it on.
struct ObserveMessage(Offered);

impl ErrorHandler<Ws> for ObserveMessage {
    async fn handle(&self, err: BoxError, _cx: &WsCx) -> Result<Reply, BoxError> {
        self.0.0.push(("message", rejected_by(&err)));
        Err(err)
    }
}

#[controller]
struct Guarded;

#[handlers]
#[ulo_ws::gateway(path = "/guarded", port = own, connect_guards(value = Deny))]
impl Guarded {
    #[ulo_ws::message("open")]
    fn open(&self) -> &'static str {
        "open"
    }

    #[ulo_ws::message("secret")]
    #[guards(value = Deny)]
    fn secret(&self) -> &'static str {
        "secret"
    }
}

struct Root(Offered);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        let connect: Arc<AnyErrorHandler<WsConnect>> = Arc::new(ObserveConnect(self.0.clone()));
        m.enhancer::<AnyErrorHandler<WsConnect>>().value(connect);
        let message: Arc<AnyErrorHandler<Ws>> = Arc::new(ObserveMessage(self.0.clone()));
        m.enhancer::<AnyErrorHandler<Ws>>().value(message);
        m.controller::<Guarded>();
    }
}

#[tokio::test]
async fn a_connect_guard_and_a_message_guard_refuse_with_one_event_and_one_kind() {
    let offered = Offered(Record::new());
    let app = Running::start(Root(offered.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let deny = std::any::type_name::<Deny>();

    let mut refused = app.connect("/guarded", &[(REFUSE, "1")]).await;
    let (code, reason) = close_frame(&mut refused).await.expect("the refused connection's Close carries a code");
    assert_eq!((code, reason.as_str()), (3003, "forbidden"), "the connect guard's refusal");
    hang_up(refused).await;
    assert_eq!(offered.0.at_least(1, "the connect phase's error handler").await, vec![("connect", Some(deny))]);

    let mut admitted = app.connect("/guarded", &[]).await;
    send_json(&mut admitted, &json!({ "event": "open", "id": 1 })).await;
    assert_eq!(next_json(&mut admitted).await, json!({ "id": 1, "data": "open" }), "the connect guard admits");
    send_json(&mut admitted, &json!({ "event": "secret", "id": 2 })).await;
    let answer = next_json(&mut admitted).await;
    assert_eq!((answer["id"].clone(), answer["error"]["kind"].clone(), answer["error"]["message"].clone()), (json!(2), json!("forbidden"), json!("forbidden")), "{answer}");
    assert_eq!(
        offered.0.at_least(2, "the message phase's error handler").await,
        vec![("connect", Some(deny)), ("message", Some(deny))],
        "one event, naming one guard, in both phases"
    );
    hang_up(admitted).await;
    app.stop().await;
}
