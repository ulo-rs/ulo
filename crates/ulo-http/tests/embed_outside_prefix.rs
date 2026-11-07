//! Under `Miss::Forward`, through a host that hands the app the full path and leaves the
//! `.nested_at` prefix to it, a path outside the prefix is the host's whatever it holds: one an app
//! route would match once the prefix were stripped, a `\` or a dot segment included, is answered
//! forwardable. Under the prefix, the app refuses such a path with its 400.

use ulo::{App, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_http::embed::{Embed, EmbedLimits, Embedded, Forwardable, Miss, Service};
use ulo_http::{ConnInfo, Cors, HttpBody, PreDispatch, Request, Response};
use ulo_runtime_tokio::Tokio;

const ORIGIN: &str = "https://app.example";

/// A host that hands the app every request with its full path, mounted or not.
struct FullPath;

impl Embed for FullPath {
    const NAME: &'static str = "full-path";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE.without_strips_prefix("the test host hands over every path whole")
    }
}

#[controller]
struct Api;

#[handlers]
impl Api {
    #[ulo_http::get("/api/{name}")]
    fn named(&self) -> &'static str {
        "app"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Api>();
        m.meta::<PreDispatch>().apply_value(Cors::new().allow_origin(ORIGIN));
    }
}

/// The app's answer to `GET path` as the host hands it over, the path whole.
async fn send(service: &Service<FullPath>, path: &str) -> Response {
    let (head, ()) = http::Request::get(path).header("origin", ORIGIN).body(()).expect("a request").into_parts();
    let req = Request { head: head.clone(), body: HttpBody::from(""), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
    service.respond(&head, req).await
}

#[tokio::test]
async fn a_refused_path_outside_the_prefix_is_forwarded_and_one_under_it_is_the_apps_400() {
    let server = Embedded::<FullPath>::new().nested_at("/app").on_miss(Miss::Forward);
    let handle = server.handle();
    let _bound = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the app listens");
    let service = handle.service();

    // `/api/{name}` would match each of these with the prefix stripped, but none is under it.
    for path in ["/api/a%5Cb", "/api/..", "/api/%2e%2e", "/elsewhere/a%5Cb"] {
        let response = send(&service, path).await;
        assert!(response.extensions().get::<Forwardable>().is_some(), "{path}: {} not forwardable", response.status());
        assert_eq!(response.headers().get("access-control-allow-origin"), None, "{path}");
    }

    for path in ["/app/api/a%5Cb", "/app/api/.."] {
        let response = send(&service, path).await;
        assert_eq!(response.status(), 400, "{path}");
        assert!(response.extensions().get::<Forwardable>().is_none(), "{path}: the app's path forwarded");
        assert_eq!(response.headers().get("access-control-allow-origin").map(|v| v.as_bytes()), Some(ORIGIN.as_bytes()), "{path}");
    }

    let response = send(&service, "/app/files/a%5Cb").await;
    assert!(response.extensions().get::<Forwardable>().is_some(), "a path under the prefix no route matches: {}", response.status());
}
