//! A WebSocket stream reply outlives the handler that returned it, and the
//! framework holds the execution open across it: a stream abandoned with items
//! still to come fires the execution's cancellation token, and one drained to
//! its end does not.
//!
//! WebSocket is the transport where this matters most and was proved last. Its
//! request scope is one message, so the execution behind a stream is the
//! narrowest of the four, and a producer that outlives it runs until the
//! process ends. HTTP, RPC over tcp and udp, and gRPC each pinned this;
//! `ws_handler_stream.rs` covers the drain-to-completion path only, which is
//! the half that passes whether or not the token is ever fired.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ulo::context::{HandlerContext, WsContext};
use ulo::module;
use ulo::websocket::{WsHandlerOutput, WsHandlerResult, WsMessage};
use ulo_macros::{new, subscriptions, websocket_gateway};

use crate::common::TestServer;

static ABANDONED_SAW_CANCEL: AtomicBool = AtomicBool::new(false);
static DRAINED_SAW_CANCEL: AtomicBool = AtomicBool::new(false);

#[websocket_gateway("/ws-tail")]
pub struct TailGateway {}

#[subscriptions]
impl TailGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Emits until cancelled. A bounded channel of one keeps the producer from
    /// running ahead of the consumer, so the abandonment is observed promptly.
    #[subscribe_message("forever")]
    async fn forever(&self, ctx: &WsContext) -> WsHandlerResult {
        let (tx, rx) = tokio::sync::mpsc::channel::<WsMessage>(1);
        let token = ctx.cancellation().clone();
        tokio::spawn(async move {
            let mut n = 0u32;
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        ABANDONED_SAW_CANCEL.store(true, Ordering::SeqCst);
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        n += 1;
                        if tx.send(WsMessage::text(n.to_string())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(WsHandlerOutput::Stream(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    /// A finite stream, and a watcher that records a cancellation if one comes.
    #[subscribe_message("three")]
    async fn three(&self, ctx: &WsContext) -> WsHandlerResult {
        let token = ctx.cancellation().clone();
        tokio::spawn(async move {
            token.cancelled().await;
            DRAINED_SAW_CANCEL.store(true, Ordering::SeqCst);
        });
        Ok(WsHandlerOutput::Stream(Box::pin(
            futures_util::stream::iter(
                ["one", "two", "three"]
                    .into_iter()
                    .map(|s| WsMessage::text(s.to_string())),
            ),
        )))
    }
}

#[module(providers: [TailGateway])]
struct TailModule;

/// Closing the socket with items still to come cancels the execution behind
/// the stream.
#[tokio_localset_test::localset_test]
async fn an_abandoned_ws_stream_cancels_the_work_feeding_it() {
    use tokio_tungstenite::tungstenite::Message;

    ABANDONED_SAW_CANCEL.store(false, Ordering::SeqCst);

    let server = TestServer::start(TailModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-tail", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    ws.send(Message::Text(r#"{"event":"forever"}"#.to_string().into()))
        .await
        .unwrap();

    // One item proves the producer is running before it is abandoned.
    let first = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("the producer must deliver before being abandoned");
    assert!(first.is_some(), "expected a first item");

    drop(ws);

    let mut cancelled = false;
    for _ in 0..100u8 {
        if ABANDONED_SAW_CANCEL.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        cancelled,
        "abandoning a WS stream must cancel the execution behind it, or the \
         producer runs until the process ends"
    );
}

/// The other direction: a stream read to its end is not a cancellation. Without
/// this, a token fired on every completed stream would pass the test above.
#[tokio_localset_test::localset_test]
async fn a_drained_ws_stream_is_not_cancelled() {
    use tokio_tungstenite::tungstenite::Message;

    DRAINED_SAW_CANCEL.store(false, Ordering::SeqCst);

    let server = TestServer::start(TailModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-tail", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    ws.send(Message::Text(r#"{"event":"three"}"#.to_string().into()))
        .await
        .unwrap();

    for expected in ["one", "two", "three"] {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("the stream must deliver every item")
            .expect("the socket stays open")
            .expect("a readable frame");
        assert_eq!(msg.to_text().unwrap(), expected);
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !DRAINED_SAW_CANCEL.load(Ordering::SeqCst),
        "a stream the client read to its end is completion, not cancellation"
    );
}
