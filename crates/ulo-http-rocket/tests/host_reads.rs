//! On rocket, declared without `host_extensions`, a `Host<T>` read nothing supplies refuses the app at
//! `listen()`, whether an attribute's parameter declared the read or a hand-written handler's
//! `HttpHandler::reads_host`; a scoped entry's exclusion counts against a route only where a
//! request can reach the route through it.

use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver, StartupError, controller, handlers, injectable,
};
use ulo_http::middleware::Next;
use ulo_http::{Host, Http, HttpCx, HttpHandler, IntoReply, Method, Middleware, PreDispatch, Request, Response};
use ulo_http_rocket::Embedded;
use ulo_runtime_tokio::Tokio;

#[derive(Clone)]
struct Tenant;

#[derive(Clone)]
struct Region;

#[controller]
struct Attributed;

#[handlers]
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

#[controller]
struct Paged;

#[handlers]
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

#[controller]
struct AdminHealth;

#[handlers]
impl AdminHealth {
    #[ulo_http::get("/admin/health")]
    fn health(&self) -> &'static str {
        "health"
    }
}

#[controller]
struct AdminPage;

#[handlers]
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

/// `/admin/{page}` reading `Host<Tenant>` behind `/admin/*` minus `/admin/health`, with the route
/// `/admin/health` when `health_route` is set.
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

async fn listen_admin(health_route: bool) -> Result<ulo::App<ulo::app::Bound>, StartupError> {
    let app = App::builder(AdminBehindAnExclusion { health_route })
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    app.bind(Embedded::new()).listen().await
}

#[tokio::test]
async fn an_exclusion_a_route_of_higher_precedence_answers_for_every_method_leaves_the_route_below_covered() {
    // `/admin/health` matches the excluded path, so the router answers every request on it there,
    // `GET` with its handler and `POST` with a 405 though `/admin/{page}` takes `POST`: no request
    // reaching `/admin/{page}` is one the entry skips.
    if let Err(error) = listen_admin(true).await {
        panic!("listen() refused a scope that runs on every request reaching `/admin/{{page}}`: {error}");
    }
}

#[tokio::test]
async fn an_exclusion_no_route_of_higher_precedence_answers_still_refuses_the_route_below() {
    // With no `/admin/health` route, `GET /admin/health` reaches `/admin/{page}` without the entry.
    let Err(error) = listen_admin(false).await else { panic!("listen() accepted a Host read the exclusion leaves unsupplied") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
    let text = error.to_string();
    for route in ["page` on `GET /admin/{page}` reads `Host<", "post_page` on `POST /admin/{page}` reads `Host<"] {
        assert!(text.contains(route), "{route} in {text}");
    }
    assert!(text.contains("does not cover `/admin/{page}`"), "{text}");
}

/// `/admin/{page}` behind `/admin/*` minus `/admin/health`, which a route claims, and
/// `/admin/status`, which none does.
struct AdminBehindTwoExclusions;

impl Module for AdminBehindTwoExclusions {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<AdminHealth>();
        m.controller::<AdminPage>();
        m.provide::<Supplier>();
        m.meta::<PreDispatch>().apply_for::<Supplier>(["/admin/*"]).exclude(["/admin/health", "/admin/status"]).supplies::<Tenant>();
    }
}

#[tokio::test]
async fn one_exclusion_no_route_of_higher_precedence_answers_refuses_the_route_below_beside_one_that_is_claimed() {
    let app = App::builder(AdminBehindTwoExclusions)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    // `GET /admin/status` reaches `/admin/{page}` without the entry, whatever `/admin/health` claims.
    let Err(error) = app.bind(Embedded::new()).listen().await else { panic!("listen() accepted a Host read `/admin/status` leaves unsupplied") };
    assert!(error.to_string().contains("page` on `GET /admin/{page}` reads `Host<"), "{error}");
}
