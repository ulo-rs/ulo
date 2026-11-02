//! On axum, whose request extensions reach the app (`host_extensions: true`), `listen()` checks no
//! `Host<T>` read, and the requests show what the check under `host_extensions: false` decides:
//! `/admin/{page}` behind `/admin/*` minus `/admin/health` reads the entry's `Tenant` on every
//! request reaching it while the route `/admin/health` exists, `POST /admin/health` answered 405
//! there, and without that route `GET /admin/health` reaches it with nothing inserted.

use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::middleware::Next;
use ulo_http::{Host, Middleware, PreDispatch, Request, Response};
use ulo_http_axum::Embedded;
use ulo_runtime_tokio::Tokio;

#[derive(Clone)]
struct Tenant;

/// Inserts `Tenant`, which the `supplies` written after it declares.
#[injectable]
struct Supplier;

impl Middleware for Supplier {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        req.head.extensions.insert(Tenant);
        next.run(req).await
    }
}

#[injectable]
struct AdminHealth;

#[routes]
impl AdminHealth {
    #[ulo_http::get("/admin/health")]
    fn health(&self) -> &'static str {
        "health"
    }
}

#[injectable]
struct AdminPage;

#[routes]
impl AdminPage {
    #[ulo_http::get("/admin/{page}")]
    fn page(&self, _tenant: Host<Tenant>) -> &'static str {
        "page"
    }

    #[ulo_http::post("/admin/{page}")]
    fn post_page(&self, _tenant: Host<Tenant>) -> &'static str {
        "posted"
    }
}

struct AdminBehindAnExclusion {
    health_route: bool,
}

impl Module for AdminBehindAnExclusion {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if self.health_route {
            m.controller::<AdminHealth>();
        }
        m.controller::<AdminPage>();
        m.provide::<Supplier>();
        m.meta::<PreDispatch>().apply_for::<Supplier>(["/admin/*"]).exclude(["/admin/health"]).supplies::<Tenant>();
    }
}

/// The app served as axum's fallback; the port, and the sender that stops it.
async fn serve(health_route: bool) -> (u16, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let server = Embedded::new();
    let embedded = server.handle();
    let app = App::builder(AdminBehindAnExclusion { health_route })
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
    (port, stop, serving)
}

/// The status and body of one HTTP/1.1 request to `port`.
async fn send(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.expect("the host accepts");
    let head = format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\ncontent-length: 0\r\n\r\n");
    stream.write_all(head.as_bytes()).await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or_else(|| panic!("no status line: {answer}"));
    let body = answer.split_once("\r\n\r\n").map_or_else(String::new, |(_, body)| body.to_owned());
    (status, body)
}

#[tokio::test]
async fn with_the_excluded_path_routed_above_every_request_reaching_the_route_below_carries_the_value() {
    let (port, stop, serving) = serve(true).await;
    assert_eq!(send(port, "GET", "/admin/reports").await, (200, "page".to_owned()));
    assert_eq!(send(port, "POST", "/admin/reports").await, (200, "posted".to_owned()));
    assert_eq!(send(port, "GET", "/admin/health").await, (200, "health".to_owned()));
    // The static route decides the method for its path: `POST` never reaches `/admin/{page}`.
    assert_eq!(send(port, "POST", "/admin/health").await.0, 405);
    let _ = stop.send(());
    serving.await.expect("the serving task ends");
}

#[tokio::test]
async fn without_the_excluded_path_routed_above_a_request_reaches_the_route_below_without_the_value() {
    let (port, stop, serving) = serve(false).await;
    assert_eq!(send(port, "GET", "/admin/reports").await, (200, "page".to_owned()));
    // Excluded, so the entry does not run, and `/admin/{page}` reads a `Tenant` nothing inserted.
    for method in ["GET", "POST"] {
        let (status, body) = send(port, method, "/admin/health").await;
        assert_eq!(status, 500, "{method} /admin/health: {body}");
    }
    let _ = stop.send(());
    serving.await.expect("the serving task ends");
}
