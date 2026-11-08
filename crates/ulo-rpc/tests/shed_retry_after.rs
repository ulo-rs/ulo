//! A request refused over `max_inflight` carries `Server::shed_retry_after` as its `RetryAfter`
//! detail: one second unset, the HTTP and gRPC servers' default, and the value set otherwise,
//! written in whole seconds rounded up. The server holds one call in flight at a bound of one, so
//! the second is refused.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use ulo::{App, AppHandle, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_rpc::{Payload, RpcClient, RpcError};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, Count, ErrorKind};

/// How long the test waits for any one step.
const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// Released by the test once the refusal has arrived, which ends the held call.
static RELEASE: Notify = Notify::const_new();

/// Raised once the held call is running, so the second call finds the bound taken.
static HELD: Notify = Notify::const_new();

#[controller]
struct Holder;

#[handlers]
impl Holder {
    #[ulo_rpc::message("shed.hold")]
    async fn hold(&self, n: Payload<u32>) -> Result<u32, Never> {
        HELD.notify_one();
        RELEASE.notified().await;
        Ok(n.0)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Holder>();
    }
}

/// The `RetryAfter` of the call refused while another holds the server's one place.
async fn refused_retry_after(server: ulo_rpc::Server<Tcp>) -> Option<Duration> {
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server.max_inflight(Count::Max(1)))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle: AppHandle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");

    let held = tokio::spawn({
        let client = client.clone();
        async move { client.request::<u32, u32>("shed.hold", &1).await }
    });
    tokio::time::timeout(PATIENCE, HELD.notified()).await.expect("the first call reached its handler");
    let refused: Result<u32, RpcError> =
        tokio::time::timeout(PATIENCE, client.request::<u32, u32>("shed.hold", &2)).await.expect("the second call answered in time");
    let error = refused.expect_err("the call over the bound was served");
    assert_eq!(error.kind(), ErrorKind::Unavailable, "{error:?}");
    RELEASE.notify_one();
    let answered = tokio::time::timeout(PATIENCE, held).await.expect("the held call answered in time").expect("the held call's task");
    assert_eq!(answered.expect("the held call is answered"), 1);
    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
    error.details().retry_after()
}

// One test, so the two runs share the statics in sequence rather than at once.
#[tokio::test]
async fn a_refused_request_carries_the_servers_shed_retry_after() {
    let unset = refused_retry_after(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0"))).await;
    assert_eq!(unset, Some(Duration::from_secs(1)), "the refusal's retry hint unset is one second");
    let set = refused_retry_after(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")).shed_retry_after(Duration::from_millis(2_500))).await;
    assert_eq!(set, Some(Duration::from_secs(3)), "the refusal carries `shed_retry_after` in whole seconds rounded up");
}
