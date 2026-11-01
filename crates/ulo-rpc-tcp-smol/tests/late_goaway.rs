//! A connection the server accepted before its drain and registered after it, its TLS handshake
//! finishing late, still receives `goaway`. Without it the client keeps sending over a connection
//! the closing server no longer reads.
//!
//! One connection holds a call in flight, so the server stays draining. The late connection's
//! client opens its TCP connection, waits until the server has accepted it, and only once the
//! drain has begun sends its TLS `ClientHello`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_net::TcpStream;
use futures_rustls::TlsConnector;
use futures_rustls::client::TlsStream;
use futures_util::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, Timer, injectable, routes};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::ServerName;
use ulo_net::rustls::{self, ClientConfig};
use ulo_rpc::{CallHeaders, Codec, Data, Frame};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;
use ulo_test_certs::localhost;
use ulo_transport::{Classify, ErrorKind};

const HOLD: &str = "late_goaway.hold";

/// How long a read waits for a frame the server writes at once.
const PATIENCE: Duration = Duration::from_secs(3);

#[derive(Debug)]
struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("never raised")
    }
}

impl std::error::Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// Raised by the held call once its handler runs.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Raised by the test to let the held call answer.
static RELEASED: AtomicBool = AtomicBool::new(false);

async fn until(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        OnSmol::runtime().sleep(Duration::from_millis(5)).await;
    }
}

async fn within<T>(what: &str, fut: impl Future<Output = T>) -> T {
    let expired = OnSmol::runtime().sleep(PATIENCE);
    match futures_util::future::select(std::pin::pin!(fut), expired).await {
        futures_util::future::Either::Left((output, _)) => output,
        futures_util::future::Either::Right(_) => panic!("{what}: nothing within {PATIENCE:?}"),
    }
}

#[injectable]
struct Holder;

#[routes]
impl Holder {
    #[ulo_rpc::message("late_goaway.hold")]
    async fn hold(&self) -> Result<u32, Never> {
        RUNNING.store(true, Ordering::SeqCst);
        until(&RELEASED).await;
        Ok(0)
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

fn connector() -> TlsConnector {
    let roots = localhost().roots();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

async fn handshake(tcp: TcpStream) -> TlsStream<TcpStream> {
    let name = ServerName::try_from("localhost").expect("a DNS name");
    connector().connect(name, tcp).await.expect("the TLS handshake with the server completes")
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) {
    let bytes = Codec::Json.encode_frame(frame).expect("the frame encodes");
    writer.write_all(&(bytes.len() as u32).to_be_bytes()).await.expect("the prefix is written");
    writer.write_all(&bytes).await.expect("the frame is written");
    writer.flush().await.expect("the frame is flushed");
}

/// The next frame, `None` at the connection's end; panics if none arrives within [`PATIENCE`].
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R, what: &str) -> Option<Frame> {
    within(what, async {
        let mut prefix = [0u8; 4];
        reader.read_exact(&mut prefix).await.ok()?;
        let mut body = vec![0; u32::from_be_bytes(prefix) as usize];
        reader.read_exact(&mut body).await.ok()?;
        Some(Codec::Json.decode_frame(&body).expect("the server's frame decodes"))
    })
    .await
}

#[test]
fn a_connection_registered_after_the_drain_began_receives_goaway() {
    OnSmol::block_on(async {
        let server = App::builder(Root)
            .runtime(OnSmol::runtime())
            .wire()
            .expect("the server wires")
            .connect()
            .await
            .expect("the server connects")
            .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0").tls(Tls::from_pem(localhost().cert_pem(), localhost().key_pem()))))
            .listen()
            .await
            .expect("the server binds");
        let addr: SocketAddr = server.addresses().first().map(|bound| bound.addr).expect("the server's address");
        let handle = server.handle();
        let serving = ulo::spawn_with(&OnSmol::runtime(), server.serve(std::future::pending::<Signal>()));

        let mut holding = handshake(TcpStream::connect(addr).await.expect("the holding connection opens")).await;
        let hold = Frame::Req { id: 1, pattern: HOLD.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
        write_frame(&mut holding, &hold).await;
        within("the held call reaching its handler", until(&RUNNING)).await;

        // Accepted by the server, whose connection task then waits for the `ClientHello`.
        let late = TcpStream::connect(addr).await.expect("the late connection opens");
        OnSmol::runtime().sleep(Duration::from_millis(200)).await;

        let closer = handle.clone();
        let closing = ulo::spawn_with(&OnSmol::runtime(), async move { closer.close(Signal::new("late goaway test")).await });
        assert_eq!(read_frame(&mut holding, "the holding connection").await, Some(Frame::Goaway), "the drain's `goaway` did not reach the holding connection");
        assert!(handle.is_draining(), "the server is not draining");

        let mut late = handshake(late).await;
        let first = read_frame(&mut late, "the late connection").await;
        assert_eq!(first, Some(Frame::Goaway), "the connection registered after the drain began received no `goaway`");

        RELEASED.store(true, Ordering::SeqCst);
        assert!(matches!(read_frame(&mut holding, "the held call's answer").await, Some(Frame::Res { id: 1, .. })));
        drop((holding, late));
        let _ = closing.await;
        let _ = serving.await;
    });
}
