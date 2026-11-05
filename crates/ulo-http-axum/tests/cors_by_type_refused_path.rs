//! Inside axum, a `Cors` applied by type, `apply::<Cors>()` or `apply_for::<Cors>(..)`, adds
//! nothing to the 400 of a path holding a `\` or a dot segment, a preflight's included, while it
//! answers a served path: only a `Cors` given to `apply_value` decorates that 400.


use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::{Cors, PreDispatch};
use ulo_http_axum::Embedded;
use ulo_runtime_tokio::Tokio;

const ORIGIN: &str = "https://app.example";

#[injectable]
struct Api;

#[routes]
impl Api {
    #[ulo_http::get("/api/{name}")]
    fn named(&self) -> &'static str {
        "app"
    }
}


struct Root(bool);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Api>();
        m.value(Cors::new().allow_origin(ORIGIN));
        if self.0 {
            m.meta::<PreDispatch>().apply::<Cors>();
        } else {
            m.meta::<PreDispatch>().apply_for::<Cors>(["/api/*"]);
        }
    }
}

async fn send(port: u16, method: &str, path: &str, headers: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.expect("axum accepts");
    let head = format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\n{headers}connection: close\r\ncontent-length: 0\r\n\r\n");
    stream.write_all(head.as_bytes()).await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or_else(|| panic!("no status line: {answer:?}"));
    let head = answer.split_once("\r\n\r\n").map_or(answer.as_str(), |(head, _)| head).to_ascii_lowercase();
    (status, head)
}

async fn probe(by_type: bool) {
    let server = Embedded::new();
    let embedded = server.handle();
    let app = App::builder(Root(by_type))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the app listens inside axum");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("the bound address").port();
    let router = Router::new().fallback_service(embedded.service());
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        let signal = async move {
            let _ = stopped.await;
            Signal::new("test")
        };
        ulo_http_axum::run(app, &embedded, axum::serve(listener, router), signal).await.expect("axum stops cleanly");
    });
    let origin = format!("origin: {ORIGIN}\r\n");
    let (status, head) = send(port, "GET", "/api/plain", &origin).await;
    assert_eq!(status, 200, "{head}");
    assert!(head.contains("access-control-allow-origin"), "control: the by-type Cors is active on a served path: {head}");
    for path in ["/api/a%5Cb", "/api/..", "/nowhere/%2e%2e", "/x\\y"] {
        let (status, head) = send(port, "GET", path, &origin).await;
        assert_eq!(status, 400, "{path}: {head}");
        assert!(!head.contains("access-control-allow-origin"), "by_type={by_type} {path}: {head}");
        let (status, head) = send(port, "OPTIONS", path, &format!("{origin}access-control-request-method: GET\r\n")).await;
        assert_eq!(status, 400, "preflight {path}: {head}");
        assert!(!head.contains("access-control-allow"), "by_type={by_type} preflight {path}: {head}");
    }
    let _ = stop.send(());
    serving.await.expect("the serving task ends");
}

#[tokio::test]
async fn apply_by_type_adds_nothing_to_the_refusal() {
    probe(true).await;
}

#[tokio::test]
async fn apply_for_by_type_adds_nothing_to_the_refusal() {
    probe(false).await;
}
