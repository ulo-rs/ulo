//! Every WebSocket adapter answers a peer's Close frame with a Close frame.
//!
//! RFC 6455 §5.5.1: *"If an endpoint receives a Close frame and did not previously send a Close
//! frame, the endpoint MUST send a Close frame in response."* Without the answer a client that
//! started the closing handshake observes an abnormal closure — a browser reports `wasClean: false`
//! and code `1006` on its `close` event — and cannot tell a clean shutdown from a dropped network.
//!
//! The frames are read off the socket by hand (`common::ws_raw`): what is asserted is what went on
//! the wire. One gateway serves the four adapters that upgrade on the HTTP port and one serves the
//! adapter that listens on a port of its own; the case is the same against each.

use std::net::SocketAddr;
use std::time::Duration;

use ulo::UloFactory;
use ulo::module;
use ulo_macros::{new, subscriptions, websocket_gateway};

use crate::common::TestServer;
use crate::common::ws_raw::{Raw, ReadEnd};

#[websocket_gateway("/ws")]
pub struct SamePortGateway {}

#[subscriptions]
impl SamePortGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
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

/// A Close carrying `1000` and a reason is answered with a Close before the TCP connection ends.
/// `tungstenite` echoes the code and the reason it was given, and both are asserted.
async fn case_answers_a_close_with_a_close(addr: SocketAddr) {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    assert!(
        c.status().contains("101"),
        "the upgrade was refused: {}",
        c.status()
    );

    let mut body = 1000u16.to_be_bytes().to_vec();
    body.extend_from_slice(b"bye");
    c.send(true, 0x8, &body, true).await;

    let frame = match c.read_frame(Duration::from_secs(3)).await {
        Ok(f) => f,
        Err(ReadEnd::Eof) => {
            panic!("the server closed the TCP connection without answering the Close")
        }
        Err(ReadEnd::Timeout) => {
            panic!("the server neither answered the Close nor hung up within 3s")
        }
        Err(ReadEnd::Io(e)) => panic!("reading the answer failed: {e}"),
    };
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

#[tokio::test]
async fn axum_answers_a_close_with_a_close() {
    case_answers_a_close_with_a_close(same_port(ulo_http_axum::AxumAdapter::new()).await).await;
}

#[tokio::test]
async fn salvo_answers_a_close_with_a_close() {
    case_answers_a_close_with_a_close(same_port(ulo_http_salvo::SalvoAdapter::new()).await).await;
}

#[tokio::test]
async fn poem_answers_a_close_with_a_close() {
    case_answers_a_close_with_a_close(same_port(ulo_http_poem::PoemAdapter::new()).await).await;
}

#[tokio::test]
async fn rocket_answers_a_close_with_a_close() {
    case_answers_a_close_with_a_close(same_port(ulo_http_rocket::RocketAdapter::new()).await).await;
}

#[tokio::test]
async fn tungstenite_answers_a_close_with_a_close() {
    case_answers_a_close_with_a_close(
        separate_port(ulo_ws_tungstenite::TungsteniteAdapter::new()).await,
    )
    .await;
}
