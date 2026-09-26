//! A claimed error answers with everything a handler can answer with.
//!
//! An error handler is instantiated at the same type its transport's interceptor is, so claiming an
//! error and answering `Empty`, a stream, or an `Err` are all reachable. Two are pinned here: a
//! claim that sends no data, and a claim that answers an error of a different kind from the one
//! raised.

use std::sync::Arc;
use std::time::Duration;
use ulo::dispatch::Items;

use serial_test::serial;
use ulo::enhancer::{ChainError, ErrorHandler};
use ulo::rpc::{RpcContext, RpcError, RpcHandlerOutput, RpcHandlerResult};
use ulo::{UloFactory, async_trait, controller, module};
use ulo_macros::{message_pattern, patterns, use_guards};

/// Claims by answering `Empty`: the call is over and carries no data back.
pub struct ClaimsWithNothing;

#[async_trait]
impl ErrorHandler<RpcContext, RpcHandlerResult> for ClaimsWithNothing {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &RpcContext,
    ) -> Option<RpcHandlerResult> {
        Some(Ok(Items::Empty))
    }
}

/// Claims by answering an error of its own, replacing the one the handler raised.
///
/// The call reached a controller, so the reshaped `Forbidden` rides the `response` lane as the
/// canonical envelope, with its own kind.
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

/// Refuses every call, so the guard path has something to reject.
pub struct AlwaysRefuse;

#[async_trait]
impl ulo::enhancer::Guard<RpcContext> for AlwaysRefuse {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        false
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

    #[message_pattern("claims.guarded")]
    #[use_guards(AlwaysRefuse {})]
    async fn guarded(&self) -> RpcHandlerResult {
        Ok(Items::Empty)
    }
}

#[module(controllers: [FailingController])]
pub struct ClaimsModule;

async fn boot<F>(configure: F) -> u16
where
    F: FnOnce(&mut UloFactory) + Send + 'static,
{
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut factory = UloFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(ClaimsModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
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

/// What the caller sees when a failure reaches the wire unclaimed.
///
/// Two failures, one of each kind the dispatcher can produce: a handler that returned an error, and
/// a guard that refused. Both are the same event to a caller — the call did not succeed — and this
/// records which frame each arrives in.
#[serial]
#[tokio::test]
async fn an_unclaimed_failure_names_its_kind() {
    let port = boot(|_| {}).await;

    let from_handler = call(port, "claims.fail").await;
    let from_guard = call(port, "claims.guarded").await;

    assert_eq!(
        from_handler["response"]["kind"], "Internal",
        "a handler's error: {from_handler}"
    );
    // The envelope carries the error's own message, not its `Display` rendering — no
    // `Internal error:` prefix. The RPC examples print this exact frame.
    assert_eq!(
        from_handler["response"]["message"], "handler said no",
        "a handler's error: {from_handler}"
    );
    assert_eq!(
        from_guard["response"]["kind"], "Forbidden",
        "a guard's refusal: {from_guard}"
    );
}

#[serial]
#[tokio::test]
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
#[tokio::test]
async fn a_claim_can_answer_with_an_error_of_its_own() {
    let port = boot(|f| {
        f.use_global_rpc_error_handler(Arc::new(ClaimsWithAnError));
    })
    .await;

    let reply = call(port, "claims.fail").await;

    assert_eq!(
        reply["response"]["kind"], "Forbidden",
        "the chain's error replaces the handler's `internal`: {reply}"
    );
}
