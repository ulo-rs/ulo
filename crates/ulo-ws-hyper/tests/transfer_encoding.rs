//! A WebSocket handshake carrying `Transfer-Encoding`, over a raw socket on the standalone
//! server's port and on the HTTP server's port. A handshake has no body, and hyper removes a
//! `Content-Length` beside `Transfer-Encoding` as it parses the head, so `Transfer-Encoding` is
//! what is left of a request two readers can frame differently: both ports refuse it with the
//! HTTP server's 400 problem document and `Connection: close`, close the connection, and answer
//! nothing pipelined behind it, whatever the coding (`chunked`, `gzip, chunked`) and with
//! `Content-Length` before or after it. `identity` is refused by hyper as it parses the head,
//! before either service runs. A request that is no handshake keeps its framing on the HTTP
//! server's port: it is read by its `Transfer-Encoding`, answered, and the request pipelined behind
//! it is answered too. A handshake without the header switches. Run on tokio and on smol, the
//! two listeners sharing the connection code.

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
use ulo_listener_tokio::TokioListener;
use ulo_ws::{Ws, WsModule};
use ulo_ws_hyper::ServerOn;

/// The longest any one wait lasts before it fails the test.
const WAIT: Duration = Duration::from_secs(5);

/// The detail of the refusal of a handshake carrying `Transfer-Encoding`.
const HANDSHAKE_TE: &str = "a WebSocket handshake carries `Transfer-Encoding`";

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

#[controller]
struct Hit;

#[handlers]
impl Hit {
    #[ulo_http::post("/hit")]
    fn hit(&self) -> &'static str {
        "hit"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Own>();
        m.controller::<Shared>();
        m.controller::<Hit>();
    }
}

/// An app serving the standalone server and the HTTP server until the test closes it.
struct Served {
    own: SocketAddr,
    http: SocketAddr,
    handle: AppHandle,
    timer: Arc<dyn Timer>,
}

impl Served {
    async fn start<L: ulo_server_hyper::Listener>(runtime: impl Runtime) -> Served {
        let app = App::builder(Root)
            .runtime(runtime)
            .drain_timeout(Duration::from_secs(2))
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(ServerOn::<L>::new("127.0.0.1:0"))
            .bind(ulo_http_hyper::ServerOn::<L>::new("127.0.0.1:0"))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
        let addr = |key: &str| {
            app.addresses().iter().find(|bound| bound.transport == key).map(|bound| bound.addr).unwrap_or_else(|| panic!("no {key} server bound: {:?}", app.addresses()))
        };
        let own = addr(<Ws as Transport>::KEY);
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

    /// Writes `request` on a fresh connection to `addr` and reads `count` answer heads, each
    /// followed by the `Content-Length` bytes of its body.
    async fn answers(&self, addr: SocketAddr, request: &str, count: usize) -> Vec<String> {
        self.within(&format!("{count} answers to {request:?}"), async {
            let mut stream = connect(addr, request).await;
            let mut answers = Vec::new();
            for _ in 0..count {
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0u8];
                    stream.read_exact(&mut byte).await.unwrap_or_else(|error| panic!("reading an answer failed after {answers:?} and {head:?}: {error}"));
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let length = header(&head, "content-length").map_or(0, |length| length.parse().unwrap_or_else(|error| panic!("`Content-Length` in {head:?}: {error}")));
                let mut body = vec![0u8; length];
                stream.read_exact(&mut body).await.unwrap_or_else(|error| panic!("reading the body of {head:?} failed: {error}"));
                answers.push(head + &String::from_utf8_lossy(&body));
            }
            answers
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

/// A handshake for `path` on `authority` with `extra` header lines after the ones RFC 6455
/// §4.2.1 asks for, and `body` after the head.
fn handshake(path: &str, authority: &str, extra: &str, body: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{extra}\r\n{body}"
    )
}

/// The handshakes carrying `Transfer-Encoding` that reach the server's service, each with an
/// empty chunked body so a server reading the body by the header would find the request after it.
fn refused_handshakes(path: &str, authority: &str) -> [(&'static str, String); 4] {
    [
        ("`Transfer-Encoding: chunked`", handshake(path, authority, "Transfer-Encoding: chunked\r\n", "0\r\n\r\n")),
        ("`Content-Length` before `Transfer-Encoding`", handshake(path, authority, "Content-Length: 5\r\nTransfer-Encoding: chunked\r\n", "0\r\n\r\n")),
        ("`Content-Length` after `Transfer-Encoding`", handshake(path, authority, "Transfer-Encoding: chunked\r\nContent-Length: 5\r\n", "0\r\n\r\n")),
        ("`Transfer-Encoding: gzip, chunked`", handshake(path, authority, "Transfer-Encoding: gzip, chunked\r\n", "0\r\n\r\n")),
    ]
}

/// The status lines in `answer`.
fn status_lines(answer: &str) -> usize {
    answer
        .match_indices("HTTP/1.")
        .filter(|(at, _)| answer.as_bytes().get(at + 8) == Some(&b' ') && answer.as_bytes().get(at + 9..at + 12).is_some_and(|code| code.iter().all(u8::is_ascii_digit)))
        .count()
}

/// The value of header `name` in the answer's head.
fn header<'a>(answer: &'a str, name: &str) -> Option<&'a str> {
    let head = answer.split("\r\n\r\n").next().unwrap_or_default();
    head.split("\r\n").filter_map(|line| line.split_once(':')).find(|(key, _)| key.trim().eq_ignore_ascii_case(name)).map(|(_, value)| value.trim())
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// Every case on the port at `addr`, a gateway at `path`.
async fn cases(app: &Served, addr: SocketAddr, path: &str, port: &str) {
    let authority = addr.to_string();
    let valid = handshake(path, &authority, "", "");
    for (what, request) in refused_handshakes(path, &authority) {
        let answer = app.to_close(addr, &(request + &valid)).await;
        assert!(answer.starts_with("HTTP/1.1 400 "), "{what} on the {port} port: {answer:?}");
        assert_eq!(header(&answer, "connection"), Some("close"), "{what} on the {port} port: {answer:?}");
        assert_eq!(header(&answer, "content-type"), Some("application/problem+json"), "{what} on the {port} port: {answer:?}");
        assert!(body(&answer).contains(HANDSHAKE_TE), "{what} on the {port} port: {answer:?}");
        assert_eq!(status_lines(&answer), 1, "{what} on the {port} port: the handshake pipelined behind it was answered: {answer:?}");
    }

    let identity = handshake(path, &authority, "Transfer-Encoding: identity\r\n", "") + &valid;
    let answer = app.to_close(addr, &identity).await;
    assert!(answer.starts_with("HTTP/1.1 400 "), "`Transfer-Encoding: identity` on the {port} port: {answer:?}");
    assert_eq!(status_lines(&answer), 1, "`Transfer-Encoding: identity` on the {port} port: the handshake pipelined behind it was answered: {answer:?}");

    let [switched] = app.answers(addr, &valid, 1).await.try_into().unwrap_or_else(|answers: Vec<String>| panic!("{answers:?}"));
    assert!(switched.starts_with("HTTP/1.1 101 "), "a handshake with no `Transfer-Encoding` on the {port} port: {switched:?}");
}

/// A `POST` on the HTTP server's port with `extra` header lines and an empty chunked body, the
/// same `POST` pipelined behind it with no body.
fn posts(authority: &str, extra: &str) -> String {
    format!("POST /hit HTTP/1.1\r\nHost: {authority}\r\n{extra}\r\n0\r\n\r\nPOST /hit HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\n\r\n")
}

async fn all_cases(app: &Served) {
    cases(app, app.own, "/own", "standalone").await;
    cases(app, app.http, "/shared", "HTTP").await;

    let authority = app.http.to_string();
    let chunked = app.answers(app.http, &posts(&authority, "Transfer-Encoding: chunked\r\n"), 2).await;
    for answer in &chunked {
        assert!(answer.starts_with("HTTP/1.1 200 "), "a `POST` read by `Transfer-Encoding: chunked`, then one pipelined: {chunked:?}");
        assert_eq!(body(answer), "hit", "{chunked:?}");
    }
    let both = app.to_close(app.http, &posts(&authority, "Content-Length: 5\r\nTransfer-Encoding: chunked\r\n")).await;
    assert!(both.starts_with("HTTP/1.1 200 "), "a `POST` carrying both length headers is read by `Transfer-Encoding`: {both:?}");
    assert_eq!(status_lines(&both), 1, "a `POST` carrying both length headers closes its connection after the answer: {both:?}");
}

#[tokio::test]
async fn a_handshake_carrying_transfer_encoding_is_refused_and_closed_on_both_ports() {
    let app = Served::start::<TokioListener>(ulo_runtime_tokio::Tokio::current()).await;
    all_cases(&app).await;
    app.stop().await;
}

#[test]
fn a_handshake_carrying_transfer_encoding_is_refused_and_closed_on_both_ports_on_smol() {
    OnSmol::block_on(async {
        let app = Served::start::<SmolListener>(OnSmol::runtime()).await;
        all_cases(&app).await;
        app.stop().await;
    });
}
