use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{App, AppBuilder, BoxError, Dep, Module, ModuleDef, ModuleIdentity, injectable, module, routes};
use ulo_http::middleware::Next;
use ulo_http::{CallError, Cors, HeaderValue, Host, Method, Middleware, PreDispatch, Request, Response};

/// Counts the requests the API-key entry saw.
#[injectable]
pub struct Hits {
    #[injectable(default)]
    pub count: AtomicUsize,
}

/// Admits a request carrying `x-api-key: sesame`; anything else is a 401 before routing's handler runs.
#[injectable]
pub struct ApiKey {
    hits: Dep<Hits>,
}

impl Middleware for ApiKey {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        self.hits.count.fetch_add(1, Ordering::SeqCst);
        if req.headers().get("x-api-key").is_none_or(|key| key != "sesame") {
            return Err(CallError::unauthorized("ApiKey").into());
        }
        next.run(req).await
    }
}

/// What an entry put in the request for a handler to take as `Host<Tenant>`: `x-tenant` copied into
/// the request's `http::Extensions`, as a host's auth layer would.
#[derive(Clone)]
pub struct Tenant(pub String);

struct TagTenant;

impl Middleware for TagTenant {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if let Some(tenant) = req.headers().get("x-tenant").and_then(|value| value.to_str().ok()).map(str::to_owned) {
            req.head.extensions.insert(Tenant(tenant));
        }
        next.run(req).await
    }
}

/// A tower layer: `x-served-by: ulo` on every response, the answers it did not produce included.
pub fn served_by() -> tower::util::MapResponseLayer<fn(Response) -> Response> {
    tower::util::MapResponseLayer::new(|mut reply: Response| {
        reply.headers_mut().insert("x-served-by", HeaderValue::from_static("ulo"));
        reply
    })
}

/// The edge of the app, declared as module metadata: the layer and CORS for every request, the
/// API-key entry for `/admin/*` minus the health probe.
pub struct EdgeModule;

impl Module for EdgeModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<Hits>();
        m.provide::<ApiKey>();
        m.export::<Hits>();
        m.meta::<PreDispatch>()
            .layer(served_by())
            .apply_value(TagTenant)
            .apply_value(Cors::new().allow_origin("https://app.example").allow_methods([Method::GET]))
            .apply_for::<ApiKey>(["/admin/*"])
            .exclude(["/admin/health"]);
    }
}

#[injectable]
pub struct PagesController;

#[routes]
impl PagesController {
    #[ulo_http::get("/")]
    fn index(&self) -> &'static str {
        "public"
    }

    #[ulo_http::get("/tenant")]
    fn tenant(&self, Host(tenant): Host<Tenant>) -> String {
        tenant.0
    }

    #[ulo_http::get("/tenant-or-none")]
    fn tenant_or_none(&self, tenant: Option<Host<Tenant>>) -> String {
        tenant.map_or_else(|| "none".to_owned(), |Host(tenant)| tenant.0)
    }

    #[ulo_http::get("/admin/panel")]
    fn panel(&self) -> &'static str {
        "panel"
    }

    #[ulo_http::get("/admin/health")]
    fn health(&self) -> &'static str {
        "ok"
    }
}

#[module(imports = [EdgeModule], controllers = [PagesController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
