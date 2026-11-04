//! Under `Miss::Forward` a path holding a `\` or a dot segment is refused only where the app
//! routes it, whatever the method, a `{*rest}` route and an `Upgrade` request's upgrade path
//! included: a path the app does not route reaches rocket's own route as the client sent it, and
//! one it routes answers the app's 400 with the CORS entry's headers alone, no other entry having
//! seen the path.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use rocket::fairing::AdHoc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::embed::Miss;
use ulo_http::middleware::Next;
use ulo_http::{Cors, HttpBody, Middleware, PreDispatch, Request, Response, UpgradeHandler, Upgrades};
use ulo_http_rocket::Embedded;
use ulo_runtime_tokio::Tokio;

const ORIGIN: &str = "https://app.example";

/// The paths an unscoped entry received, as `METHOD path`, in order.
type Recorded = Arc<Mutex<Vec<String>>>;

/// Records every request it receives, as an unscoped static-file layer reads it.
struct Record(Recorded);

impl Middleware for Record {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(format!("{} {}", req.method(), req.path()));
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

    #[ulo_http::get("/rest/{*rest}")]
    fn rest(&self) -> &'static str {
        "app rest"
    }
}

/// Takes upgrades on `/sock/{room}`.
struct Taker;

impl UpgradeHandler for Taker {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/sock/{room}".into()]
    }

    fn upgrade(&self, _req: Request) -> BoxFuture<'static, Response> {
        Box::pin(async { Response::new(HttpBody::from("upgraded")) })
    }
}

struct Root(Recorded);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Api>();
        m.meta::<Upgrades>().register(Taker);
        m.meta::<PreDispatch>()
            .apply_value(Record(Arc::clone(&self.0)))
            .apply_value(Cors::new().allow_origin(ORIGIN).allow_credentials(true).expose_headers([http::header::ETAG]));
    }
}

/// The host's own routes, ranked below the app's catch-all, so they answer only what the app
/// forwards.
#[rocket::get("/files/<name>", rank = 200)]
fn host_files(name: &str) -> String {
    format!("host file {name}")
}

#[rocket::get("/sock/<name>", rank = 200)]
fn host_sock(name: &str) -> String {
    format!("host sock {name}")
}

#[rocket::get("/app/files/<name>", rank = 200)]
fn host_app_files(name: &str) -> String {
    format!("host app file {name}")
}

/// One HTTP/1.1 answer: the status, the head and the body.
struct Answer {
    status: u16,
    head: String,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (found, value) = line.split_once(':')?;
            found.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    /// Asserts the app's 400 carrying what the CORS entry adds for the allowed origin.
    fn assert_refused_with_cors(&self, what: &str) {
        assert_eq!(self.status, 400, "{what}: {}\n{}", self.head, self.body);
        assert_eq!(self.header("access-control-allow-origin"), Some(ORIGIN), "{what}: {}", self.head);
        assert_eq!(self.header("access-control-allow-credentials"), Some("true"), "{what}: {}", self.head);
        assert_eq!(self.header("access-control-expose-headers"), Some("etag"), "{what}: {}", self.head);
        assert_eq!(self.header("access-control-allow-methods"), None, "{what}: nothing of the preflight");
    }
}

async fn send(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> Answer {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.expect("the host accepts");
    let mut head = format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\ncontent-length: 0\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or_else(|| panic!("no status line: {answer:?}"));
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    Answer { status, head: head.to_owned(), body: body.to_owned() }
}

/// A running rocket host with the app mounted at `nested`, or at `/` under `Miss::Forward`.
struct Host {
    port: u16,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl Host {
    async fn start(recorded: &Recorded, nested: Option<&'static str>) -> Host {
        let mut server = Embedded::new().on_miss(Miss::Forward);
        if let Some(prefix) = nested {
            server = server.nested_at(prefix);
        }
        let embedded = server.handle();
        let app = App::builder(Root(Arc::clone(recorded)))
            .runtime(Tokio::current())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(server)
            .listen()
            .await
            .expect("the app listens inside rocket");
        let figment = rocket::Config::figment().merge(("address", "127.0.0.1")).merge(("port", 0)).merge(("log_level", "off"));
        let (bound, port) = oneshot::channel::<u16>();
        let bound = Mutex::new(Some(bound));
        let host = rocket::custom(figment)
            .attach(AdHoc::on_liftoff("bound port", move |rocket| {
                let port = rocket.config().port;
                Box::pin(async move {
                    if let Some(bound) = bound.lock().ok().and_then(|mut bound| bound.take()) {
                        let _ = bound.send(port);
                    }
                })
            }))
            .mount(nested.unwrap_or("/"), ulo_http_rocket::routes(&embedded))
            .mount("/", rocket::routes![host_files, host_sock, host_app_files]);
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("test")
            };
            ulo_http_rocket::run(app, &embedded, host, signal).await.expect("rocket stops cleanly");
        });
        Host { port: port.await.expect("rocket lifts off"), stop, serving }
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        self.serving.await.expect("the serving task ends");
    }
}

fn assert_no_refused_path(recorded: &Recorded) {
    for seen in recorded.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        let lower = seen.to_ascii_lowercase();
        assert!(
            !seen.contains("..") && !seen.contains('\\') && !lower.contains("%5c") && !seen.contains("/./") && !lower.contains("%2e"),
            "an unscoped entry received a refused path: {seen}"
        );
    }
}

#[tokio::test]
async fn a_refused_path_the_app_does_not_route_reaches_the_host_and_one_it_routes_answers_400_with_cors_headers() {
    let recorded = Recorded::default();
    let host = Host::start(&recorded, None).await;
    let origin: &[(&str, &str)] = &[("origin", ORIGIN)];

    // No route of the app matches `/files/{name}`: rocket's route answers each with the segment as
    // rocket decodes it, and nothing of the app's CORS.
    let answer = send(host.port, "GET", "/files/plain", &[]).await;
    assert_eq!((answer.status, answer.body.as_str()), (200, "host file plain"));
    for (path, name) in [("/files/a%5Cb", "a\\b"), ("/files/a%5cb", "a\\b"), ("/files/%2e%2e", ".."), ("/files/..", "..")] {
        let answer = send(host.port, "GET", path, origin).await;
        assert_eq!((answer.status, answer.body.as_str()), (200, format!("host file {name}").as_str()), "{path}");
        assert_eq!(answer.header("access-control-allow-origin"), None, "{path}");
    }
    // Neither do these, though a route shares their first segment: the host's 404, not the app's 400.
    for path in ["/api/x/..", "/../api/x", "/nowhere/%5C", "/files/../api/x"] {
        let answer = send(host.port, "GET", path, origin).await;
        assert_ne!(answer.status, 400, "{path}: {}", answer.head);
        assert_eq!(answer.header("access-control-allow-origin"), None, "{path}");
    }

    // `/api/{name}` routes these whatever the method, so the app owns them: its 400, readable by
    // the allowed origin alone.
    for path in ["/api/a%5Cb", "/api/a%5cb", "/api/..", "/api/%2e%2e", "/api/a%5Cb/", "/api/./"] {
        send(host.port, "GET", path, origin).await.assert_refused_with_cors(&format!("GET {path}"));
        let answer = send(host.port, "GET", path, &[("origin", "https://elsewhere.example")]).await;
        assert_eq!((answer.status, answer.header("access-control-allow-origin")), (400, None), "{path}: {}", answer.body);
    }
    for method in ["POST", "PUT", "DELETE", "HEAD", "OPTIONS"] {
        send(host.port, method, "/api/a%5Cb", origin).await.assert_refused_with_cors(&format!("{method} /api/a%5Cb"));
    }
    // A preflight to an owned refused path is the 400, not the preflight's 204.
    let preflight = [("origin", ORIGIN), ("access-control-request-method", "GET")];
    send(host.port, "OPTIONS", "/api/a%5Cb", &preflight).await.assert_refused_with_cors("a preflight to /api/a%5Cb");

    // `/rest/{*rest}` owns every path under it.
    for path in ["/rest/../x", "/rest/./x", "/rest/a\\b", "/rest/x/..", "/rest/a/..%5C..%5Cetc"] {
        send(host.port, "GET", path, origin).await.assert_refused_with_cors(path);
    }

    let answer = send(host.port, "GET", "/api/plain", origin).await;
    assert_eq!((answer.status, answer.header("access-control-allow-origin")), (200, Some(ORIGIN)));
    assert_no_refused_path(&recorded);
    assert!(recorded.lock().unwrap_or_else(PoisonError::into_inner).contains(&"GET /api/plain".to_owned()), "the entry ran at all");
    host.stop().await;
}

#[tokio::test]
async fn an_upgrade_path_owns_a_refused_path_only_for_an_upgrade_request() {
    let recorded = Recorded::default();
    let host = Host::start(&recorded, None).await;

    let upgrade = [("origin", ORIGIN), ("connection", "upgrade"), ("upgrade", "websocket")];
    send(host.port, "GET", "/sock/a%5Cb", &upgrade).await.assert_refused_with_cors("an Upgrade request to /sock/a%5Cb");

    // No route of the app matches `/sock/{room}`, so a plain request is the host's.
    let answer = send(host.port, "GET", "/sock/a%5Cb", &[("origin", ORIGIN)]).await;
    assert_eq!((answer.status, answer.body.as_str()), (200, "host sock a\\b"), "{}", answer.head);
    assert_eq!(answer.header("access-control-allow-origin"), None);
    assert_no_refused_path(&recorded);
    host.stop().await;
}

#[tokio::test]
async fn nested_at_a_prefix_the_app_owns_what_it_routes_under_the_prefix() {
    let recorded = Recorded::default();
    let host = Host::start(&recorded, Some("/app")).await;
    let origin: &[(&str, &str)] = &[("origin", ORIGIN)];

    for path in ["/app/api/a%5Cb", "/app/rest/../x", "/app/api/.."] {
        send(host.port, "GET", path, origin).await.assert_refused_with_cors(path);
    }
    let answer = send(host.port, "GET", "/app/files/a%5Cb", origin).await;
    assert_eq!((answer.status, answer.body.as_str()), (200, "host app file a\\b"), "{}", answer.head);
    for path in ["/app/../api/x", "/app/nowhere/.."] {
        let answer = send(host.port, "GET", path, origin).await;
        assert_ne!(answer.status, 400, "{path}: {}", answer.head);
    }
    assert_no_refused_path(&recorded);
    host.stop().await;
}

#[injectable]
struct Unreachable;

#[routes]
impl Unreachable {
    #[ulo_http::get("/files/a%5Cb")]
    fn encoded(&self) -> &'static str {
        "never"
    }
}

struct UnreachableRoot;

impl Module for UnreachableRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Unreachable>();
    }
}

#[tokio::test]
async fn a_route_whose_static_segment_holds_a_backslash_is_refused_when_the_app_listens_inside_rocket() {
    let bound = App::builder(UnreachableRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(Embedded::new().on_miss(Miss::Forward));
    let Err(error) = bound.listen().await else {
        panic!("a route no request reaches listened");
    };
    let ulo::StartupError::Configure(errors) = &error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    assert_eq!(errors.len(), 1, "{error}");
    let expected = "`Unreachable::encoded`: the route `/files/a%5Cb` can never match: its static segment `a%5Cb` holds a `\\`, raw or as \
                    `%5C`, and a request path holding one is refused with 400 before routing";
    assert!(error.to_string().contains(expected), "{expected}\n{error}");
}
