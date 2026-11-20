//! A message whose handler is still running when its connection ends has its execution cancelled
//! as `Disconnected`: the handler sees the client go, whether the client sent a Close or the
//! socket was cut.

mod support;

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::json;
use tokio::sync::Notify;
use ulo::{CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_ws::WsModule;

use support::{Running, hang_up, send_json, within};

/// The execution of the message held in its handler, and the signal that it is held.
#[derive(Clone, Default)]
struct Held(Arc<(Mutex<Option<ExecutionRef>>, Notify)>);

#[controller]
struct Waiting {
    held: Dep<Held>,
}

#[handlers]
#[ulo_ws::gateway(path = "/wait", port = own)]
impl Waiting {
    /// Hands the test its execution, then waits for a cancellation only the connection's end fires.
    #[ulo_ws::message("wait")]
    async fn wait(&self, exec: ExecutionRef) -> &'static str {
        *self.held.0.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(exec.clone());
        self.held.0.1.notify_one();
        exec.cancelled().await;
        "cancelled"
    }
}

struct Root(Held);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(self.0.clone());
        m.controller::<Waiting>();
    }
}

/// The held message's execution once its handler is waiting.
async fn hold(app: &Running, held: &Held) -> (support::Socket, ExecutionRef) {
    let mut socket = app.connect("/wait", &[]).await;
    send_json(&mut socket, &json!({ "event": "wait", "id": 1 })).await;
    within("the handler holding the message", held.0.1.notified()).await;
    let exec = held.0.0.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
    assert_eq!(exec.cancel_reason(), None, "cancelled before the connection ended");
    (socket, exec)
}

#[tokio::test]
async fn a_message_running_when_the_client_closes_is_cancelled() {
    let held = Held::default();
    let app = Running::start(Root(held.clone()), ulo_ws_hyper::Server::new("127.0.0.1:0")).await;

    let (socket, exec) = hold(&app, &held).await;
    hang_up(socket).await;
    within("the execution's cancellation after the client's Close", exec.cancelled()).await;
    assert_eq!(exec.cancel_reason(), Some(CancelReason::Disconnected));

    let (socket, exec) = hold(&app, &held).await;
    // Dropped without a Close: the socket is cut.
    drop(socket);
    within("the execution's cancellation after the socket was cut", exec.cancelled()).await;
    assert_eq!(exec.cancel_reason(), Some(CancelReason::Disconnected));
    app.stop().await;
}
