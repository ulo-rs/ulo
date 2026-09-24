//! Every WebSocket adapter speaks RFC 6455 the same way on the wire: the closing handshake a
//! peer starts is completed, a Ping is answered with a Pong, a fragmented message is reassembled
//! with a control frame allowed between its fragments, and a handler's own Close ends the
//! connection with no data frame after it.
//!
//! One contract, stamped once per adapter (`docs/explainers/testing-and-examples.md`). Five
//! adapters serve WebSocket, and what separates them is where each listens: axum, salvo, poem and
//! rocket upgrade on the HTTP port, and `ulo-ws-tungstenite` serves a port of its own. rocket
//! implements no `WsAdapter` and tungstenite no `HttpAdapter`, so each is stamped with the boot it
//! has, and the cases are the same against both.
//!
//! The frames are read off the socket by hand (`common::ws_raw`): what is asserted is what went on
//! the wire.

use std::net::SocketAddr;
use std::time::Duration;

use ulo::UloFactory;
use ulo::module;
use ulo::ws::{WsHandlerResult, WsMessage};
use ulo_macros::{new, subscriptions, websocket_gateway};

use crate::common::TestServer;
use crate::common::ws_raw::{Frame, Raw, ReadEnd};

/// The message an `echo` handler is sent, split across two frames by the fragmentation cases.
const ECHO: &str = r#"{"event":"echo","data":"0123456789"}"#;

#[websocket_gateway("/ws")]
pub struct SamePortGateway {}

#[subscriptions]
impl SamePortGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Answers with the length of the text it was handed: a payload that reached the handler with
    /// bytes added or lost between the fragments still parses, and shows another length.
    #[subscribe_message("echo")]
    async fn echo(&self, msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text(format!("len={}", msg.as_text().unwrap_or("").len())).into())
    }

    #[subscribe_message("bye")]
    async fn bye(&self, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::close_with(1000, "server done").into())
    }
}

#[module(providers: [SamePortGateway])]
struct SamePortModule;

/// The declared port is a key that pairs the gateway with the socket handed to
/// `use_websocket_listener`; nothing binds it.
#[websocket_gateway("/ws", port = 19771)]
pub struct SeparatePortGateway {}

#[subscriptions]
impl SeparatePortGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self, msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text(format!("len={}", msg.as_text().unwrap_or("").len())).into())
    }

    #[subscribe_message("bye")]
    async fn bye(&self, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::close_with(1000, "server done").into())
    }
}

#[module(providers: [SeparatePortGateway])]
struct SeparatePortModule;

async fn same_port(adapter: impl ulo::http::HttpAdapter + 'static) -> SocketAddr {
    let server = TestServer::start_adapter(UloFactory::new(), SamePortModule, adapter).await;
    SocketAddr::from(([127, 0, 0, 1], server.port))
}

async fn separate_port(adapter: impl ulo::ws::WsAdapter) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(SeparatePortModule).await.unwrap();
        app.use_websocket_adapter(adapter).unwrap();
        app.use_websocket_listener(19771, listener).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(
            bound
                .websocket
                .into_iter()
                .next()
                .expect("separate-port gateway must report an address"),
        );
        app.run().await;
    });
    addr_rx.await.expect("WebSocket server failed to start")
}

async fn upgraded(addr: SocketAddr) -> Raw {
    let c = Raw::connect(addr, "/ws", &[]).await;
    assert!(
        c.status().contains("101"),
        "the upgrade was refused: {}",
        c.status()
    );
    c
}

fn frame_or_panic(read: Result<Frame, ReadEnd>, waiting_for: &str) -> Frame {
    match read {
        Ok(f) => f,
        Err(ReadEnd::Eof) => {
            panic!("the server closed the TCP connection while {waiting_for} was awaited")
        }
        Err(ReadEnd::Timeout) => panic!("{waiting_for} did not arrive within the bound"),
        Err(ReadEnd::Io(e)) => panic!("reading {waiting_for} failed: {e}"),
    }
}

/// RFC 6455 §5.5.1: a Close received is answered with a Close, and the TCP connection then ends.
/// `tungstenite` echoes the code and the reason it was given, and both are asserted.
async fn case_answers_a_close_with_a_close(addr: SocketAddr) {
    let mut c = upgraded(addr).await;

    let mut body = 1000u16.to_be_bytes().to_vec();
    body.extend_from_slice(b"bye");
    c.send(true, 0x8, &body, true).await.unwrap();

    let frame = frame_or_panic(c.read_frame(Duration::from_secs(3)).await, "the Close");
    assert_eq!(
        frame.opcode, 0x8,
        "the first frame after a Close was not a Close: opcode {:#x}, payload {:?}",
        frame.opcode, frame.payload
    );
    assert_eq!(
        frame.close_code(),
        Some(1000),
        "the answering Close carries another code"
    );
    assert_eq!(
        frame.close_reason(),
        "bye",
        "the answering Close carries another reason"
    );
    assert!(
        c.eof(Duration::from_secs(3)).await,
        "the Close was answered and the TCP connection was then held open"
    );
}

/// RFC 6455 §5.5.2 and §5.5.3: a Ping is answered with a Pong, and the Pong carries the same
/// application data.
async fn case_answers_a_ping_with_a_pong(addr: SocketAddr) {
    let mut c = upgraded(addr).await;
    c.send(true, 0x9, b"probe-payload", true).await.unwrap();

    let frame = frame_or_panic(c.read_frame(Duration::from_secs(3)).await, "the Pong");
    assert_eq!(
        frame.opcode, 0xA,
        "the frame answering a Ping was not a Pong: opcode {:#x}",
        frame.opcode
    );
    assert_eq!(
        frame.payload, b"probe-payload",
        "the Pong carries other data than the Ping did"
    );
}

/// RFC 6455 §5.4: a message split across a first fragment and a continuation frame reaches the
/// handler whole.
async fn case_reassembles_a_fragmented_message(addr: SocketAddr) {
    let mut c = upgraded(addr).await;
    let (head, tail) = ECHO.split_at(12);
    c.send(false, 0x1, head.as_bytes(), true).await.unwrap();
    c.send(true, 0x0, tail.as_bytes(), true).await.unwrap();

    let frame = frame_or_panic(c.read_frame(Duration::from_secs(3)).await, "the echo");
    assert_eq!(
        frame.opcode, 0x1,
        "the answer was not a text frame: opcode {:#x}",
        frame.opcode
    );
    assert_eq!(
        frame.text(),
        format!("len={}", ECHO.len()),
        "the handler did not see the whole message"
    );
}

/// RFC 6455 §5.4: a control frame between two fragments is answered, and the fragments still
/// make one message.
async fn case_answers_a_ping_between_fragments(addr: SocketAddr) {
    let mut c = upgraded(addr).await;
    let (head, tail) = ECHO.split_at(12);
    c.send(false, 0x1, head.as_bytes(), true).await.unwrap();
    c.send(true, 0x9, b"mid", true).await.unwrap();
    c.send(true, 0x0, tail.as_bytes(), true).await.unwrap();

    let mut pong = None;
    let mut text = None;
    while pong.is_none() || text.is_none() {
        let frame = frame_or_panic(
            c.read_frame(Duration::from_secs(3)).await,
            "the Pong and the echo",
        );
        match frame.opcode {
            0xA => pong = Some(frame),
            0x1 => text = Some(frame),
            other => panic!("unexpected frame between the fragments: opcode {other:#x}"),
        }
    }
    assert_eq!(
        pong.unwrap().payload,
        b"mid",
        "the Pong carries other data than the Ping did"
    );
    assert_eq!(
        text.unwrap().text(),
        format!("len={}", ECHO.len()),
        "the handler did not see the whole message"
    );
}

/// RFC 6455 §5.5.1: after sending a Close, an endpoint sends no more data frames. A handler's
/// own Close reaches the peer as written, and a message sent after it is answered with nothing.
async fn case_sends_no_data_frame_after_its_own_close(addr: SocketAddr) {
    let mut c = upgraded(addr).await;
    c.send_text(r#"{"event":"bye"}"#).await.unwrap();

    let frame = frame_or_panic(
        c.read_frame(Duration::from_secs(3)).await,
        "the handler's Close",
    );
    assert_eq!(
        frame.opcode, 0x8,
        "the handler's Close did not reach the peer as a Close: opcode {:#x}",
        frame.opcode
    );
    assert_eq!(frame.close_code(), Some(1000));
    assert_eq!(frame.close_reason(), "server done");

    // The server may already have hung up, and a failed write is that, not a finding.
    let _ = c.send_text(ECHO).await;
    if let Ok(frame) = c.read_frame(Duration::from_secs(2)).await {
        panic!(
            "a frame followed the server's own Close: opcode {:#x}, payload {:?}",
            frame.opcode, frame.payload
        );
    }
}

macro_rules! ws_adapter_suite {
    ($adapter_mod:ident, $boot:expr) => {
        mod $adapter_mod {
            #[tokio::test]
            async fn answers_a_close_with_a_close() {
                super::case_answers_a_close_with_a_close($boot.await).await;
            }

            #[tokio::test]
            async fn answers_a_ping_with_a_pong() {
                super::case_answers_a_ping_with_a_pong($boot.await).await;
            }

            #[tokio::test]
            async fn reassembles_a_fragmented_message() {
                super::case_reassembles_a_fragmented_message($boot.await).await;
            }

            #[tokio::test]
            async fn answers_a_ping_between_fragments() {
                super::case_answers_a_ping_between_fragments($boot.await).await;
            }

            #[tokio::test]
            async fn sends_no_data_frame_after_its_own_close() {
                super::case_sends_no_data_frame_after_its_own_close($boot.await).await;
            }
        }
    };
}

ws_adapter_suite!(axum, super::same_port(ulo_http_axum::AxumAdapter::new()));
ws_adapter_suite!(salvo, super::same_port(ulo_http_salvo::SalvoAdapter::new()));
ws_adapter_suite!(poem, super::same_port(ulo_http_poem::PoemAdapter::new()));
ws_adapter_suite!(
    rocket,
    super::same_port(ulo_http_rocket::RocketAdapter::new())
);
ws_adapter_suite!(
    tungstenite,
    super::separate_port(ulo_ws_tungstenite::TungsteniteAdapter::new())
);

/// A server that completes the upgrade and then writes nothing: the violation each case is
/// asserted against. A case that passed here could not fail against an adapter either.
mod silent {
    use std::net::SocketAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn boot() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut request = [0u8; 2048];
                    let _ = socket.read(&mut request).await;
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                              Connection: Upgrade\r\n\
                              Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                        )
                        .await;
                    std::future::pending::<()>().await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    #[should_panic(expected = "did not arrive within the bound")]
    async fn answers_a_close_with_a_close() {
        super::case_answers_a_close_with_a_close(boot().await).await;
    }

    #[tokio::test]
    #[should_panic(expected = "did not arrive within the bound")]
    async fn answers_a_ping_with_a_pong() {
        super::case_answers_a_ping_with_a_pong(boot().await).await;
    }

    #[tokio::test]
    #[should_panic(expected = "did not arrive within the bound")]
    async fn reassembles_a_fragmented_message() {
        super::case_reassembles_a_fragmented_message(boot().await).await;
    }

    #[tokio::test]
    #[should_panic(expected = "did not arrive within the bound")]
    async fn answers_a_ping_between_fragments() {
        super::case_answers_a_ping_between_fragments(boot().await).await;
    }

    #[tokio::test]
    #[should_panic(expected = "did not arrive within the bound")]
    async fn sends_no_data_frame_after_its_own_close() {
        super::case_sends_no_data_frame_after_its_own_close(boot().await).await;
    }
}
