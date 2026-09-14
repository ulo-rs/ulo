//! A claimed error answers with everything a handler can answer with.
//!
//! An error handler is instantiated at the same type its transport's interceptor is, so claiming an
//! error and answering `Empty`, a stream, or an `Err` are all reachable. The two pinned here are the
//! ones no reply frame could express while the chain answered the bare payload: a claim that sends
//! no data, and a claim that answers an error of a different kind from the one raised.

use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;
use ulo::enhancer::{ChainError, ErrorHandler};
use ulo::rpc::{RpcContext, RpcError, RpcHandlerOutput, RpcHandlerResult};
use ulo::{UloFactory, async_trait, controller, module};
use ulo_macros::{message_pattern, patterns};

/// Claims by answering `Empty`: the call is over and carries no data back.
pub struct ClaimsWithNothing;

#[async_trait]
impl ErrorHandler<RpcContext, RpcHandlerResult> for ClaimsWithNothing {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &RpcContext,
    ) -> Option<RpcHandlerResult> {
        Some(Ok(RpcHandlerOutput::Empty))
    }
}

/// Claims by answering an error of its own, replacing the one the handler raised.
///
/// `Forbidden` is one of the variants the adapters classify as a dispatch failure, so it reaches the
/// caller as a wire-`err` frame — a shape a claim could not produce at all while the chain answered
/// an `RpcData`.
pub struct ClaimsWithAnError;

#[async_trait]
impl ErrorHandler<RpcContext, RpcHandlerResult> for ClaimsWithAnError {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &RpcContext,
    ) -> Option<RpcHandlerResult> {
        Some(Err(RpcError::Forbidden("reshaped by the chain".into())))
    }
}

#[controller]
pub struct FailingController {}

#[patterns]
impl FailingController {
    #[message_pattern("claims.fail")]
    async fn fail(&self) -> RpcHandlerResult {
        Err(RpcError::Internal("handler said no".into()))
    }
}

#[module(controllers: [FailingController])]
pub struct ClaimsModule;

async fn boot<F>(configure: F) -> u16
where
    F: FnOnce(&mut UloFactory) + Send + 'static,
{
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = UloFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(ClaimsModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
}

async fn call(port: u16, pattern: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .expect("the connection must stay readable");
    serde_json::from_str(&line).expect("the reply must be JSON")
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_claim_can_answer_with_no_data() {
    let port = boot(|f| {
        f.use_global_rpc_error_handler(Arc::new(ClaimsWithNothing));
    })
    .await;

    let reply = call(port, "claims.fail").await;

    assert_eq!(reply["response"], serde_json::Value::Null, "reply: {reply}");
    assert!(
        reply.get("err").is_none(),
        "an empty claim is a completed call, not a wire error: {reply}"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_claim_can_answer_with_an_error_of_its_own() {
    let port = boot(|f| {
        f.use_global_rpc_error_handler(Arc::new(ClaimsWithAnError));
    })
    .await;

    let reply = call(port, "claims.fail").await;

    assert_eq!(
        reply["err"]["status"], "forbidden",
        "the chain's error replaces the handler's `internal`: {reply}"
    );
}
