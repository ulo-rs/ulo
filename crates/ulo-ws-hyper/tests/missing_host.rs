//! An HTTP/1.1 request with no `Host` field on the standalone server's port, over a raw socket:
//! RFC 6455 §4.2.1 requires `Host` in a handshake, and this server reads requests off the wire, so
//! it answers the HTTP server's own refusal, the 400 problem document with `Connection: close`,
//! and nothing pipelined behind it is answered. An absolute-form target with no `Host` is refused
//! too; an empty `Host`, an absolute-form target naming another `Host` and a handshake carrying
//! one switch protocols, and HTTP/1.0 reaches the handshake, which refuses it for its version. The
//! HTTP server's port answers the same handshake with the same refusal, and a handshake carrying
//! `Host` there switches. Run on tokio and on smol, the two listeners sharing the connection code.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_net::TcpStream;
use futures_util::future::{self, Either};
use futures_util::{AsyncReadExt, AsyncWriteExt};
use ulo::{App, AppHandle, Module, ModuleDef, ModuleIdentity, Runtime, Signal, Timer, Transport, controller, handlers};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_listener_smol::SmolListener;
use ulo_ws::{Ws, WsModule};
use ulo_ws_hyper::ServerOn;

/// The longest any one wait lasts before it fails the test.
const WAIT: Duration = Duration::from_secs(5);

/// The detail of the HTTP server's refusal of a request with no `Host`.
const DETAIL: &str = "an HTTP/1.1 request carries no `Host`";

#[controller]
struct Own;

#[handlers]
#[ulo_ws::gateway(path = "/own", port = own)]
impl Own {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

#[controller]
struct Shared;

#[handlers]
#[ulo_ws::gateway(path = "/shared")]
impl Shared {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// The standalone server's gateway, and with `shared` one on the HTTP server's port.
struct Root {
    shared: bool,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Own>();
        if self.shared {
            m.import(WsModule::for_root());
            m.controller::<Shared>();
        }
    }
}

/// An app serving until the test closes it, and where each of its servers listens.
struct Served {
    own: SocketAddr,
    http: Option<SocketAddr>,
    handle: AppHandle,
    timer: Arc<dyn Timer>,
}

impl Served {
    /// The app with the standalone server, and with `shared` the HTTP server too, on `runtime`.
    async fn start<L: ulo_server_hyper::Listener>(runtime: impl Runtime, shared: bool) -> Served {
        let app = App::builder(Root { shared })
            .runtime(runtime)
            .drain_timeout(Duration::from_secs(2))
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(ServerOn::<L>::new("127.0.0.1:0"));
        let app = if shared { app.bind(ulo_http_hyper::Server::new("127.0.0.1:0")) } else { app };
        let app = app.listen().await.unwrap_or_else(|error| panic!("the app did not listen: {error}"));
        let addr = |key: &str| app.addresses().iter().find(|bound| bound.transport == key).map(|bound| bound.addr);
        let own = addr(<Ws as Transport>::KEY).unwrap_or_else(|| panic!("the standalone server bound nothing: {:?}", app.addresses()));
        let http = addr("http");
        let handle = app.handle();
        let runtime = Arc::clone(handle.runtime().unwrap_or_else(|| panic!("the app has no runtime")));
        let timer = Arc::clone(&runtime) as Arc<dyn Timer>;
        drop(runtime.spawn(Box::pin(async move {
            let _ = app.serve(future::pending::<Signal>()).await;
        })));
        Served { own, http, handle, timer }
    }

    async fn within<F: Future>(&self, what: &str, fut: F) -> F::Output {
        match future::select(std::pin::pin!(fut), self.timer.sleep(WAIT)).await {
            Either::Left((output, _)) => output,
            Either::Right(((), _)) => panic!("{what} did not happen within {WAIT:?}"),
        }
    }

    /// Writes `request` on a fresh connection to `addr` and reads until the server closes it.
    async fn to_close(&self, addr: SocketAddr, request: &str) -> String {
        self.within(&format!("the server's close after {request:?}"), async {
            let mut stream = connect(addr, request).await;
            let mut answer = Vec::new();
            stream.read_to_end(&mut answer).await.unwrap_or_else(|error| panic!("reading the answer failed: {error}"));
            String::from_utf8_lossy(&answer).into_owned()
        })
        .await
    }

    /// Writes `request` on a fresh connection to `addr` and reads the answer's head.
    async fn head(&self, addr: SocketAddr, request: &str) -> String {
        self.within(&format!("the answer to {request:?}"), async {
            let mut stream = connect(addr, request).await;
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0u8];
                stream.read_exact(&mut byte).await.unwrap_or_else(|error| panic!("reading the answer failed after {head:?}: {error}"));
                head.push(byte[0]);
            }
            String::from_utf8_lossy(&head).into_owned()
        })
        .await
    }

    async fn stop(self) {
        let _ = self.within("the app's close", self.handle.close(Signal::new("test"))).await;
    }
}

async fn connect(addr: SocketAddr, request: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.unwrap_or_else(|error| panic!("connecting to {addr} failed: {error}"));
    stream.write_all(request.as_bytes()).await.unwrap_or_else(|error| panic!("writing the request failed: {error}"));
    stream
}

/// A handshake's request line, `Host` line or none, and the lines RFC 6455 §4.2.1 asks for.
fn handshake(line: &str, host: Option<&str>) -> String {
    let host = host.map(|host| format!("Host:{host}\r\n")).unwrap_or_default();
    format!("{line}\r\n{host}Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
}

/// The status lines in `answer`: `HTTP/1.x`, a space and three digits, which the refusal's detail,
/// naming `HTTP/1.1`, does not hold.
fn status_lines(answer: &str) -> usize {
    answer
        .match_indices("HTTP/1.")
        .filter(|(at, _)| answer.as_bytes().get(at + 8) == Some(&b' ') && answer.as_bytes().get(at + 9..at + 12).is_some_and(|code| code.iter().all(u8::is_ascii_digit)))
        .count()
}

/// The value of header `name`, given in lowercase, in the answer's head.
fn header<'a>(answer: &'a str, name: &str) -> Option<&'a str> {
    let head = answer.split("\r\n\r\n").next().unwrap_or_default();
    head.split("\r\n").filter_map(|line| line.split_once(':')).find(|(key, _)| key.trim().eq_ignore_ascii_case(name)).map(|(_, value)| value.trim())
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// The answer is the HTTP server's refusal of a request with no `Host`, and the only one written.
fn refused(what: &str, answer: &str) {
    assert!(answer.starts_with("HTTP/1.1 400 "), "{what}: {answer:?}");
    assert_eq!(header(answer, "connection"), Some("close"), "{what}: {answer:?}");
    assert_eq!(header(answer, "content-type"), Some("application/problem+json"), "{what}: {answer:?}");
    assert!(body(answer).contains(DETAIL), "{what}: {answer:?}");
    assert_eq!(status_lines(answer), 1, "{what}: the request pipelined behind it was answered: {answer:?}");
}

/// Every case on the standalone server's port.
async fn standalone_cases(app: &Served) {
    let addr = app.own;
    let authority = addr.to_string();
    let valid = handshake("GET /own HTTP/1.1", Some(&authority));

    for (what, request) in [
        ("a handshake with no `Host`", handshake("GET /own HTTP/1.1", None)),
        ("an absolute-form target, no `Host`", handshake(&format!("GET http://{authority}/own HTTP/1.1"), None)),
        ("a request that is no handshake, no `Host`", "GET /own HTTP/1.1\r\n\r\n".to_owned()),
    ] {
        refused(what, &app.to_close(addr, &(request + &valid)).await);
    }

    for (what, request) in [
        ("an empty `Host`", handshake("GET /own HTTP/1.1", Some(""))),
        ("an absolute-form target naming another `Host`", handshake(&format!("GET http://{authority}/own HTTP/1.1"), Some("other.example"))),
        ("a handshake with `Host`", valid.clone()),
    ] {
        let head = app.head(addr, &request).await;
        assert!(head.starts_with("HTTP/1.1 101 "), "{what}: {head:?}");
    }

    let answer = app.to_close(addr, &handshake("GET /own HTTP/1.0", None)).await;
    assert!(answer.contains(" 400 "), "HTTP/1.0, no `Host`: {answer:?}");
    assert_eq!(body(&answer), "a WebSocket handshake is HTTP/1.1", "HTTP/1.0, no `Host`, is refused by the handshake for its version: {answer:?}");
}

#[tokio::test]
async fn the_standalone_server_refuses_a_handshake_with_no_host_as_the_http_port_does() {
    let app = Served::start::<ulo_listener_tokio::TokioListener>(ulo_runtime_tokio::Tokio::current(), true).await;
    standalone_cases(&app).await;

    let http = app.http.unwrap_or_else(|| panic!("the HTTP server bound nothing"));
    let own = app.to_close(app.own, &handshake("GET /own HTTP/1.1", None)).await;
    let shared = app.to_close(http, &handshake("GET /shared HTTP/1.1", None)).await;
    refused("a handshake with no `Host` on the HTTP port", &shared);
    for name in ["connection", "content-type", "content-length"] {
        assert_eq!(header(&own, name), header(&shared, name), "`{name}` on the two ports: {own:?} and {shared:?}");
    }
    assert_eq!(body(&own), body(&shared), "the refusal's body on the two ports");
    let head = app.head(http, &handshake("GET /shared HTTP/1.1", Some(&http.to_string()))).await;
    assert!(head.starts_with("HTTP/1.1 101 "), "a handshake with `Host` on the HTTP port: {head:?}");
    app.stop().await;
}

#[test]
fn the_standalone_server_on_smol_refuses_a_handshake_with_no_host() {
    OnSmol::block_on(async {
        let app = Served::start::<SmolListener>(OnSmol::runtime(), false).await;
        standalone_cases(&app).await;
        app.stop().await;
    });
}
