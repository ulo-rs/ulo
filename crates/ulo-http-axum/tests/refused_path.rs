//! Inside axum, a path holding a `\` or a dot segment answers the app's 400 with the CORS entry's
//! response headers, a preflight's included, and no unscoped entry sees it.

use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::middleware::Next;
use ulo_http::{Cors, Middleware, PreDispatch, Request, Response};
use ulo_http_axum::Embedded;
use ulo_runtime_tokio::Tokio;

const ORIGIN: &str = "https://app.example";

/// Records the path of every request it receives, as an unscoped static-file layer reads it.
struct Record(Arc<Mutex<Vec<String>>>);

impl Middleware for Record {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(req.path().to_owned());
        next.run(req).await
    }
}

#[injectable]
struct Api;

#[routes]
impl Api {
    #[ulo_http::get("/api/{name}")]
    fn named(&self) -> &'static str {
        "app"
    }
}

struct Root(Arc<Mutex<Vec<String>>>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Api>();
        m.meta::<PreDispatch>().apply_value(Record(Arc::clone(&self.0))).apply_value(Cors::new().allow_origin(ORIGIN));
    }
}

/// The status and the head, lowercased, of one HTTP/1.1 answer.
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

#[tokio::test]
async fn a_refused_path_answers_400_with_cors_headers_and_reaches_no_entry() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let server = Embedded::new();
    let embedded = server.handle();
    let app = App::builder(Root(Arc::clone(&recorded)))
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
    let allowed = format!("access-control-allow-origin: {ORIGIN}");
    for path in ["/api/a%5Cb", "/api/..", "/nowhere/%2e%2e", "/x\\y"] {
        for method in ["GET", "POST"] {
            let (status, head) = send(port, method, path, &origin).await;
            assert_eq!(status, 400, "{method} {path}: {head}");
            assert!(head.contains(&allowed), "{method} {path}: {head}");
        }
        let (status, head) = send(port, "GET", path, "origin: https://elsewhere.example\r\n").await;
        assert_eq!(status, 400, "{path}: {head}");
        assert!(!head.contains("access-control-allow-origin"), "{path}: {head}");
        let (status, head) = send(port, "OPTIONS", path, &format!("{origin}access-control-request-method: GET\r\n")).await;
        assert_eq!(status, 400, "a preflight to {path}: {head}");
        assert!(head.contains(&allowed) && !head.contains("access-control-allow-methods"), "a preflight to {path}: {head}");
    }
    let (status, head) = send(port, "GET", "/api/plain", &origin).await;
    assert_eq!(status, 200, "{head}");
    assert_eq!(*recorded.lock().unwrap_or_else(PoisonError::into_inner), ["/api/plain"], "an unscoped entry received a refused path");

    let _ = stop.send(());
    serving.await.expect("the serving task ends");
}
