//! Under rocket's `host_extensions: false`, a `Host<T>` read nothing supplies refuses the app at
//! `listen()`, whether an attribute's parameter declared the read or a hand-written handler's
//! `HttpHandler::reads_host`.

use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver, StartupError, injectable, routes,
};
use ulo_http::middleware::Next;
use ulo_http::{Host, Http, HttpCx, HttpHandler, IntoReply, Method, Middleware, PreDispatch, Request, Response};
use ulo_http_rocket::Embedded;
use ulo_runtime_tokio::Tokio;

#[derive(Clone)]
struct Tenant;

#[derive(Clone)]
struct Region;

#[injectable]
struct Attributed;

#[routes]
impl Attributed {
    #[ulo_http::get("/tenant")]
    fn tenant(&self, _tenant: Host<Tenant>) -> &'static str {
        "tenant"
    }
}

struct Written;

impl Construct for Written {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Written)
    }
}

async fn region(cx: HttpCx) -> Result<Response, BoxError> {
    Ok("region".into_reply(&cx)?)
}

impl Controller for Written {
    fn mount(m: &mut Mount<'_>) {
        let handler = HttpHandler::new(Method::GET, region).reads_host::<Region>();
        m.handler(HandlerSpec::<Http, _>::new("region", handler).route("/region"));
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Attributed>();
        m.controller::<Written>();
    }
}

#[tokio::test]
async fn a_host_read_nothing_supplies_is_refused_for_both_spellings() {
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let Err(error) = app.bind(Embedded::new()).listen().await else { panic!("listen() accepted Host reads nothing supplies") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains("tenant` on `GET /tenant` reads `Host<"), "{text}");
    assert!(text.contains("region` on `GET /region` reads `Host<"), "{text}");
    assert!(text.contains("Tenant"), "{text}");
    assert!(text.contains("Region"), "{text}");
}

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
struct Paged;

#[routes]
impl Paged {
    #[ulo_http::get("/pages/{page}")]
    fn page(&self, _tenant: Host<Tenant>) -> &'static str {
        "page"
    }
}

struct OnePage;

impl Module for OnePage {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Paged>();
        m.provide::<Supplier>();
        m.meta::<PreDispatch>().apply_for::<Supplier>(["/pages/me"]).supplies::<Tenant>();
    }
}

#[tokio::test]
async fn a_supplies_after_a_scoped_entry_matching_some_paths_of_a_route_does_not_reach_it() {
    // The entry runs on `/pages/me` alone, so `/pages/7` would read a `Tenant` nothing inserted.
    let app = App::builder(OnePage).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let Err(error) = app.bind(Embedded::new()).listen().await else { panic!("listen() accepted a Host read the entry reaches on one path") };
    let text = error.to_string();
    assert!(text.contains("page` on `GET /pages/{page}` reads `Host<"), "{text}");
}
