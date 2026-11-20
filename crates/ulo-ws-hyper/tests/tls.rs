//! `Server::tls`: the standalone WebSocket server serves `wss`, offering `http/1.1` alone in ALPN,
//! and a message round-trips over the TLS connection; a client offering only `h2` gets an alert at
//! the handshake.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig};
use ulo_test_certs::localhost;
use ulo_ws::Payload;

const PATIENCE: Duration = Duration::from_secs(5);

#[controller]
struct Echo;

#[handlers]
#[ulo_ws::gateway(path = "/echo", port = own)]
impl Echo {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Echo>();
    }
}

struct Served {
    addr: SocketAddr,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn start() -> Served {
    let cert = localhost();
    let server = ulo_ws_hyper::Server::new("127.0.0.1:0").tls(Tls::from_pem(cert.cert_pem().as_bytes(), cert.key_pem().as_bytes()));
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("{error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .bind(server)
        .listen()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let [bound] = app.addresses().try_into().expect("one bound address");
    assert!(bound.tls, "the bound address reports TLS");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Served { addr: bound.addr, handle, serving }
}

impl Served {
    async fn stop(self) {
        let _ = self.handle.close(Signal::new("tls")).await;
        let _ = self.serving.await;
    }

    /// A TLS connection trusting the fixture certificate, offering `alpn`.
    async fn tls(&self, alpn: &[&[u8]]) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let mut config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("the default versions")
            .with_root_certificates(localhost().roots())
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let tcp = TcpStream::connect(self.addr).await?;
        let name = ServerName::try_from("localhost").expect("a DNS name");
        tokio::time::timeout(PATIENCE, TlsConnector::from(Arc::new(config)).connect(name, tcp)).await.expect("the handshake ends in time")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wss_connection_negotiates_http_1_1_and_round_trips_a_message() {
    let server = start().await;
    let tls = server.tls(&[b"h2", b"http/1.1"]).await.expect("the TLS handshake succeeds");
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"http/1.1".as_slice()), "the server offers `http/1.1` alone");

    let (mut socket, response) = tokio::time::timeout(PATIENCE, tokio_tungstenite::client_async(format!("wss://localhost:{}/echo", server.addr.port()), tls))
        .await
        .expect("the upgrade ends in time")
        .expect("the upgrade succeeds over TLS");
    assert_eq!(response.status(), 101);
    socket.send(Message::text(json!({ "event": "echo", "id": 1, "data": "over tls" }).to_string())).await.expect("the message is sent");
    let reply = match tokio::time::timeout(PATIENCE, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Value>(&text).expect("a JSON reply"),
        other => panic!("expected the echo, got {other:?}"),
    };
    assert_eq!(reply["data"], json!("over tls"), "{reply}");
    let _ = socket.close(None).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_offering_only_h2_is_refused_at_the_handshake() {
    let server = start().await;
    let refused = server.tls(&[b"h2"]).await;
    assert!(refused.is_err(), "a client offering only `h2` completed the handshake");
    server.stop().await;
}
