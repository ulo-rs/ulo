//! Under `Miss::Forward` a scoped pre-dispatch entry speaks for the app's own routes only: a
//! request a route of the app matches runs it, a 405 included, and a request no route of the app
//! matches goes back to rocket untouched, so the host's own `/admin/host` is answered by the host
//! and not refused with the app's 401.

use rocket::fairing::AdHoc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers, injectable};
use ulo_http::embed::Miss;
use ulo_http::middleware::Next;
use ulo_http::{CallError, Middleware, PreDispatch, Request, Response};
use ulo_http_rocket::Embedded;
use ulo_runtime_tokio::Tokio;

/// Refuses with 401 unless the request carries `x-key: open`.
#[injectable]
struct ApiKeyAuth;

impl Middleware for ApiKeyAuth {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if req.headers().get("x-key").is_none_or(|key| key != "open") {
            return Err(CallError::unauthorized("ApiKey").into());
        }
        next.run(req).await
    }
}

/// Unscoped: rewrites `/admin/old` to `/admin/gone`, a path no route of the app matches, so its
/// miss cannot be forwarded.
struct Rewrite;

impl Middleware for Rewrite {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if req.path() == "/admin/old" {
            req.set_path("/admin/gone")?;
        }
        next.run(req).await
    }
}

#[controller]
struct Admin;

#[handlers]
impl Admin {
    #[ulo_http::get("/admin/app")]
    fn app(&self) -> &'static str {
        "app"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Admin>();
        m.provide::<ApiKeyAuth>();
        m.meta::<PreDispatch>().apply_value(Rewrite).apply_for::<ApiKeyAuth>(["/admin/*"]);
    }
}

/// The host's own route, ranked below the app's catch-all, so it answers only what the app
/// forwards.
#[rocket::get("/admin/host", rank = 200)]
fn host_route() -> &'static str {
    "host"
}

/// The status and body of one HTTP/1.1 request to `port`.
async fn send(port: u16, method: &str, path: &str, key: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.expect("the host accepts");
    let key = key.map(|key| format!("x-key: {key}\r\n")).unwrap_or_default();
    let head = format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\n{key}connection: close\r\ncontent-length: 0\r\n\r\n");
    stream.write_all(head.as_bytes()).await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or_else(|| panic!("no status line: {answer}"));
    let body = answer.split_once("\r\n\r\n").map_or_else(String::new, |(_, body)| body.to_owned());
    (status, body)
}

#[tokio::test]
async fn a_scoped_entry_runs_for_the_apps_routes_and_a_forwarded_miss_reaches_the_host_untouched() {
    let server = Embedded::new().on_miss(Miss::Forward);
    let embedded = server.handle();
    let app = App::builder(Root)
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
    let bound = std::sync::Mutex::new(Some(bound));
    let host = rocket::custom(figment)
        .attach(AdHoc::on_liftoff("bound port", move |rocket| {
            let port = rocket.config().port;
            Box::pin(async move {
                if let Some(bound) = bound.lock().ok().and_then(|mut bound| bound.take()) {
                    let _ = bound.send(port);
                }
            })
        }))
        .mount("/", ulo_http_rocket::routes(&embedded))
        .mount("/", rocket::routes![host_route]);
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        let signal = async move {
            let _ = stopped.await;
            Signal::new("test")
        };
        ulo_http_rocket::run(app, &embedded, host, signal).await
    });
    let port = port.await.expect("rocket lifts off");

    // The app's route, and its 405, run the scoped entry.
    assert_eq!(send(port, "GET", "/admin/app", None).await.0, 401, "GET /admin/app, anonymous");
    assert_eq!(send(port, "DELETE", "/admin/app", None).await.0, 401, "DELETE /admin/app, anonymous");
    assert_eq!(send(port, "GET", "/admin/app", Some("open")).await, (200, "app".to_owned()));
    assert_eq!(send(port, "DELETE", "/admin/app", Some("open")).await.0, 405);
    assert_eq!(send(port, "OPTIONS", "/admin/app", None).await.0, 401, "OPTIONS /admin/app, anonymous");

    // A miss the app answers itself, its path rewritten, is the app's surface: the scope refuses
    // the anonymous caller before the app's own 404.
    assert_eq!(send(port, "GET", "/admin/old", None).await.0, 401, "GET /admin/old, anonymous");
    assert_eq!(send(port, "GET", "/admin/old", Some("open")).await.0, 404);

    // A path no route of the app matches is the host's: its own route answers it, and what
    // nothing routes is rocket's 404, not the app's 401.
    assert_eq!(send(port, "GET", "/admin/host", None).await, (200, "host".to_owned()), "the host's route, anonymous");
    let (status, body) = send(port, "GET", "/admin/nowhere", None).await;
    assert_eq!(status, 404, "{body}");
    assert!(!body.contains("ApiKey"), "the app answered the miss: {body}");

    let _ = stop.send(());
    serving.await.expect("the serving task ends").expect("rocket stops cleanly");
}
