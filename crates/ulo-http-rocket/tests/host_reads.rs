//! Under rocket's `host_extensions: false`, a `Host<T>` read nothing supplies refuses the app at
//! `listen()`, whether an attribute's parameter declared the read or a hand-written handler's
//! `HttpHandler::reads_host`.

use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver, StartupError, injectable, routes,
};
use ulo_http::{Host, Http, HttpCx, HttpHandler, IntoReply, Method, Response};
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
