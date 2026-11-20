//! Through a host that hands the app the full path and leaves the `.nested_at` prefix to it, an
//! `Upgrade` request outside the prefix is the host's: no upgrade handler takes it and no scoped
//! entry runs for it, whatever upgrade path its full path would match. Under the prefix the
//! upgrade handler takes it, after the scoped entries.

use std::borrow::Cow;

use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, controller, handlers, injectable};
use ulo_http::embed::{Embed, EmbedLimits, Embedded, Forwardable, Miss, Service};
use ulo_http::middleware::{Middleware, Next};
use ulo_http::{ConnInfo, HttpBody, PreDispatch, Request, Response, UpgradeHandler, Upgrades};
use ulo_runtime_tokio::Tokio;

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
    #[ulo_http::get("/page")]
    fn page(&self) -> &'static str {
        "page"
    }
}

/// Takes every upgrade on a one- or two-segment path, answering 200 `taken`.
struct Taker;

impl UpgradeHandler for Taker {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/{room}".into(), "/{room}/socket".into()]
    }

    fn upgrade(&self, _req: Request) -> BoxFuture<'static, Response> {
        Box::pin(async { Response::new(HttpBody::from("taken")) })
    }
}

/// Sets `x-scoped: ran` on the response it passes.
#[injectable]
struct Scoped;

impl Middleware for Scoped {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        let mut response = next.run(req).await?;
        response.headers_mut().insert("x-scoped", http::HeaderValue::from_static("ran"));
        Ok(response)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Api>();
        m.provide::<Scoped>();
        m.meta::<Upgrades>().register(Taker);
        m.meta::<PreDispatch>().apply_for::<Scoped>(["/*"]);
    }
}

/// The app's answer to an upgrade request for `target` as the host hands it over, whole.
async fn upgrade(service: &Service<FullPath>, target: &str) -> Response {
    let (head, ()) = http::Request::get(target).header("upgrade", "websocket").body(()).expect("a request").into_parts();
    let req = Request { head: head.clone(), body: HttpBody::from(""), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
    service.respond(&head, req).await
}

async fn served(miss: Miss) -> (App<ulo::app::Bound>, Service<FullPath>) {
    let server = Embedded::<FullPath>::new().nested_at("/app").on_miss(miss);
    let handle = server.handle();
    let bound = App::builder(Root)
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
    (bound, handle.service())
}

async fn body(response: Response) -> String {
    use http_body_util::BodyExt;
    let bytes = response.into_body().collect().await.expect("the body reads").to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Each full path matches an upgrade path as written, `/{room}` or `/{room}/socket`, and none is
/// under `/app`: a sibling sharing the prefix's letters, the prefix with an encoded slash in either
/// case, an encoded letter or dot, doubled and tripled leading slashes, another case, the gateway
/// path alone, and an absolute-form target naming it.
const OUTSIDE: [&str; 12] = [
    "/socket",
    "/apps",
    "/apps/socket",
    "/app%2Fsocket",
    "/app%2fsocket",
    "/%61pp/socket",
    "/app%2e%2e/socket",
    "//app/socket",
    "///app/socket",
    "/APP/socket",
    "/App/socket",
    "http://host/socket",
];

/// What went wrong for each target outside the prefix, empty when nothing did: the upgrade
/// handler taking it, a scoped entry running for it, or an answer other than `expected`.
async fn outside_failures(service: &Service<FullPath>, expected: fn(&Response) -> bool) -> Vec<String> {
    let mut failures = Vec::new();
    for target in OUTSIDE {
        let response = upgrade(service, target).await;
        let status = response.status();
        let scoped = response.headers().get("x-scoped").is_some();
        let answered = expected(&response);
        let taken = body(response).await == "taken";
        if taken || scoped || !answered {
            failures.push(format!("{target}: status {status}, taken {taken}, scoped entry ran {scoped}"));
        }
    }
    failures
}

#[tokio::test]
async fn an_upgrade_outside_the_prefix_is_forwarded_untaken_under_miss_forward() {
    let (_bound, service) = served(Miss::Forward).await;
    let failures = outside_failures(&service, |response| response.extensions().get::<Forwardable>().is_some()).await;
    assert!(failures.is_empty(), "each should be forwardable, untaken, with no scoped entry run:\n{}", failures.join("\n"));
}

#[tokio::test]
async fn an_upgrade_outside_the_prefix_is_the_apps_404_under_miss_final() {
    let (_bound, service) = served(Miss::Final).await;
    let failures = outside_failures(&service, |response| response.status() == 404).await;
    assert!(failures.is_empty(), "each should be the app's 404, untaken, with no scoped entry run:\n{}", failures.join("\n"));
}

#[tokio::test]
async fn an_upgrade_under_the_prefix_is_taken_after_the_scoped_entries() {
    let (_bound, service) = served(Miss::Forward).await;
    for target in ["/app/socket", "/app/lobby/socket"] {
        let response = upgrade(&service, target).await;
        assert_eq!(response.headers().get("x-scoped").map(|v| v.as_bytes()), Some(b"ran".as_slice()), "{target}");
        assert_eq!(body(response).await, "taken", "{target}");
    }
}

/// A dot segment that leaves the prefix once resolved is refused under it, never taken.
#[tokio::test]
async fn an_upgrade_climbing_out_of_the_prefix_by_a_dot_segment_is_not_taken() {
    let (_bound, service) = served(Miss::Final).await;
    for target in ["/app/../socket", "/app/%2e%2e/socket", "/app/./socket"] {
        let response = upgrade(&service, target).await;
        assert_eq!(response.status(), 400, "{target}");
        assert_ne!(body(response).await, "taken", "{target}");
    }
}
