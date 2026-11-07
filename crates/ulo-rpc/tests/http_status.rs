//! An error's HTTP status is HTTP's alone: an RPC call failing with a `Conflict` that names a
//! status of its own through `ulo_http::WithHttpStatus` answers the envelope's `conflict` with its
//! message, as the same error without the status does.

use std::sync::Arc;
use std::time::Duration;

use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_http::{StatusCode, WithHttpStatus};
use ulo_rpc::{CallError, ErrorKind, Payload, RpcClient};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);

#[controller]
struct Pots;

#[handlers]
impl Pots {
    #[ulo_rpc::message("pots.claim")]
    fn claim(&self, Payload(with_status): Payload<bool>) -> Result<(), CallError> {
        let error = CallError::new(ErrorKind::Conflict, "the pot is taken");
        Err(if with_status { error.with_http_status(StatusCode::IM_A_TEAPOT) } else { error })
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Pots>();
    }
}

#[tokio::test]
async fn an_error_s_http_status_changes_no_rpc_rendering() {
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");

    for with_status in [false, true] {
        let answered = tokio::time::timeout(PATIENCE, client.request::<_, ()>("pots.claim", &with_status)).await;
        let error = answered.expect("in time").expect_err("a refusal");
        assert_eq!((error.kind(), error.message()), (ErrorKind::Conflict, "the pot is taken"), "with the status: {with_status}");
    }

    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
}
