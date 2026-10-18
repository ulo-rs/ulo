use std::marker::PhantomData;
use std::sync::Arc;

use http::header::{CONTENT_TYPE, HeaderValue};
use ulo::scope::Auto;
use ulo::{
    BoxError, BoxFuture, Construct, ConstructError, Controller, Dep, Dependencies, HandlerSpec, Module, ModuleDef,
    ModuleIdentity, Mount, Resolver,
};
use ulo_health::{Readiness, Report};
use ulo_http::__private::HttpHandler;
use ulo_http::{Http, HttpBody, HttpCx, Method, Response, StatusCode};

use crate::body::{MEDIA_TYPE, render};

/// `/health/live`, `/health/ready` and `/health/started`, imported as a module beside
/// `ulo_health::HealthModule`.
///
/// Each route answers `GET` (and `HEAD`, which the router answers from it) with 200 when its probe
/// passes and 503 when it does not, the body `application/health+json`. The routes read
/// `Dep<Readiness>`, which the health module exports globally. Each is a controller of this module
/// mounted at its configured path, so a path set here is the path served, under an embedding's
/// mount prefix where there is one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthHttp {
    pub(crate) live: String,
    pub(crate) ready: String,
    pub(crate) started: String,
    pub(crate) detail: bool,
}

impl HealthHttp {
    pub fn new() -> Self {
        HealthHttp {
            live: String::from("/health/live"),
            ready: String::from("/health/ready"),
            started: String::from("/health/started"),
            detail: false,
        }
    }

    pub fn live_path(mut self, path: impl Into<String>) -> Self {
        self.live = path.into();
        self
    }

    pub fn ready_path(mut self, path: impl Into<String>) -> Self {
        self.ready = path.into();
        self
    }

    pub fn started_path(mut self, path: impl Into<String>) -> Self {
        self.started = path.into();
        self
    }

    /// Adds the draft's `checks` object, each check with its redacted reason. Off by default, since
    /// it tells an outsider about internals.
    pub fn detail(mut self, on: bool) -> Self {
        self.detail = on;
        self
    }
}

impl Default for HealthHttp {
    fn default() -> Self {
        HealthHttp::new()
    }
}

impl Module for HealthHttp {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Detail(self.detail));
        m.controller::<ProbeRoute<Live>>().at(normalize(&self.live));
        m.controller::<ProbeRoute<Ready>>().at(normalize(&self.ready));
        m.controller::<ProbeRoute<Started>>().at(normalize(&self.started));
    }
}

/// Whether the routes write the `checks` object; bound by `HealthHttp` and read by its routes alone.
struct Detail(bool);

/// The probe a route answers. A controller's mount names no value, so each probe is a type and
/// each route a controller of its own, mounted at `/` under the prefix its path sets.
trait Kind: Send + Sync + 'static {
    const NAME: &'static str;
    const PROBE: Which;
}

#[derive(Clone, Copy)]
enum Which {
    Live,
    Ready,
    Started,
}

struct Live;
struct Ready;
struct Started;

impl Kind for Live {
    const NAME: &'static str = "live";
    const PROBE: Which = Which::Live;
}

impl Kind for Ready {
    const NAME: &'static str = "ready";
    const PROBE: Which = Which::Ready;
}

impl Kind for Started {
    const NAME: &'static str = "started";
    const PROBE: Which = Which::Started;
}

struct ProbeRoute<P> {
    readiness: Dep<Readiness>,
    detail: Dep<Detail>,
    _probe: PhantomData<fn() -> P>,
}

impl<P: Kind> Construct for ProbeRoute<P> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Readiness>>("readiness").field::<Dep<Detail>>("detail");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(ProbeRoute { readiness: r.dep::<Readiness>().await?, detail: r.dep::<Detail>().await?, _probe: PhantomData })
    }
}

impl<P: Kind> Controller for ProbeRoute<P> {
    fn mount(m: &mut Mount<'_>) {
        let handler = HttpHandler::new(Method::GET, "/", |cx: HttpCx| -> BoxFuture<'static, Result<Response, BoxError>> {
            Box::pin(answer::<P>(cx))
        });
        m.handler(HandlerSpec::<Http, _>::new(P::NAME, handler).route("/"));
    }
}

async fn answer<P: Kind>(cx: HttpCx) -> Result<Response, BoxError> {
    let route = cx.exec().get::<ProbeRoute<P>>().await?;
    let report = probe(&route.readiness, P::PROBE).await;
    let body = render(&report, route.detail.0, cx.app());
    let mut reply = Response::new(HttpBody::from(body));
    *reply.status_mut() = if report.is_up() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    reply.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
    Ok(reply)
}

async fn probe(readiness: &Readiness, which: Which) -> Arc<Report> {
    match which {
        Which::Live => readiness.liveness().await,
        Which::Ready => readiness.readiness().await,
        Which::Started => readiness.startup().await,
    }
}

/// A path with one leading `/` and no trailing one, the form `.at(..)` joins to the route `/`.
fn normalize(path: &str) -> String {
    format!("/{}", path.trim_matches('/'))
}
