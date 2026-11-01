//! An HTTP app with `HealthModule` and `HealthHttp` imported, served through a backend that
//! serves nothing and called in process through `AppService`, with every wait under a real-clock
//! deadline.

#![allow(dead_code)]

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use http_body_util::BodyExt;
use tokio::sync::Notify;
use ulo::{App, AppHandle, Bound, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_health::{Check, Health, HealthModule, Probe};
use ulo_health_http::HealthHttp;
use ulo_http::{AppService, Backend, BackendLimits, ConnInfo, HeaderMap, HttpBody, HttpConfig, Request, Response, Routing, StatusCode};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;

/// The longest any one wait in a test may take, on the real clock.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// Keeps the `AppService` it is handed at `bind`; serves nothing.
#[derive(Clone, Default)]
pub struct Keeper {
    svc: Arc<OnceLock<AppService>>,
    drained: Arc<Notify>,
}

impl Backend for Keeper {
    const NAME: &'static str = "keeper";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(&mut self, _listeners: Vec<BoundListener>, _tls: Option<Arc<ServerConfig>>, svc: AppService, _cfg: &HttpConfig) -> Result<(), BoxError> {
        let _ = self.svc.set(svc);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.drained.notified().await;
        Ok(())
    }

    async fn drain(&self) {
        self.drained.notify_one();
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// A check a test flips: up while `up` is set, down with `reason` when it is not.
pub struct Toggle {
    name: &'static str,
    probe: Probe,
    bound: Bound,
    pub up: AtomicBool,
    reason: &'static str,
    /// When set, the check never answers.
    stuck: bool,
}

impl Toggle {
    pub fn readiness(name: &'static str) -> Arc<Toggle> {
        Arc::new(Toggle { name, probe: Probe::Readiness, bound: Bound::Default, up: AtomicBool::new(true), reason: "the check is down", stuck: false })
    }

    pub fn liveness(name: &'static str) -> Arc<Toggle> {
        Arc::new(Toggle { name, probe: Probe::Liveness, bound: Bound::Default, up: AtomicBool::new(true), reason: "the check is down", stuck: false })
    }

    pub fn down_with(name: &'static str, reason: &'static str) -> Arc<Toggle> {
        Arc::new(Toggle { name, probe: Probe::Readiness, bound: Bound::Default, up: AtomicBool::new(false), reason, stuck: false })
    }

    /// A readiness check that never answers, under `bound`.
    pub fn stuck(name: &'static str, bound: Duration) -> Arc<Toggle> {
        Arc::new(Toggle { name, probe: Probe::Readiness, bound: Bound::After(bound), up: AtomicBool::new(true), reason: "", stuck: true })
    }

    pub fn set(&self, up: bool) {
        self.up.store(up, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct Reason(&'static str);

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for Reason {}

/// The check as an `Arc<dyn Check>` the module contributes; the test keeps its own clone.
pub struct Contributed(pub Arc<Toggle>);

impl Check for Contributed {
    fn name(&self) -> &str {
        self.0.name
    }

    fn check(&self) -> BoxFuture<'_, Health> {
        Box::pin(async move {
            if self.0.stuck {
                std::future::pending::<()>().await;
            }
            if self.0.up.load(Ordering::SeqCst) { Health::Up } else { Health::down(Reason(self.0.reason)) }
        })
    }

    fn bound(&self) -> Bound {
        self.0.bound
    }

    fn probe(&self) -> Probe {
        self.0.probe
    }
}

/// The root: the health module, the HTTP probes as configured, and each check.
pub struct Root {
    pub health: HealthModule,
    pub http: HealthHttp,
    pub checks: Vec<Arc<Toggle>>,
    /// A secret some check's reason carries, registered so redaction replaces it.
    pub secret: Option<&'static str>,
}

impl Root {
    pub fn new() -> Root {
        Root { health: HealthModule::new(), http: HealthHttp::new(), checks: Vec::new(), secret: None }
    }

    pub fn check(mut self, check: &Arc<Toggle>) -> Root {
        self.checks.push(Arc::clone(check));
        self
    }
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(self.health.clone());
        m.import(self.http.clone());
        for check in &self.checks {
            let check: Arc<dyn Check> = Arc::new(Contributed(Arc::clone(check)));
            m.contribute::<dyn Check>().value(check);
        }
        if let Some(secret) = self.secret {
            m.secret(&ulo::Secret::new(secret));
        }
    }
}

pub struct Running {
    svc: AppService,
    pub handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

pub async fn start(root: Root) -> Running {
    let keeper = Keeper::default();
    let app = App::builder(root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app wires: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app connects: {error}"))
        .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()))
        .listen();
    let app = tokio::time::timeout(PATIENCE, app).await.expect("listen finished in time").unwrap_or_else(|error| panic!("the app listens: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let svc = keeper.svc.get().cloned().expect("the backend was handed the service");
    Running { svc, handle, serving }
}

/// A response whose body was read to its end.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub routing: Option<Routing>,
}

impl Reply {
    /// The handler's name when a handler answered, `None` otherwise.
    pub fn handler(&self) -> Option<&'static str> {
        match &self.routing {
            Some(Routing::Matched { handler, .. }) => Some(handler.name()),
            _ => None,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| panic!("a JSON body, got {:?}: {error}", String::from_utf8_lossy(&self.body)))
    }
}

impl Running {
    pub async fn get(&self, path: &str) -> Reply {
        self.send(http::Method::GET, path).await
    }

    pub async fn send(&self, method: http::Method, path: &str) -> Reply {
        let (head, ()) = http::Request::builder().method(method).uri(path).body(()).expect("the request builds").into_parts();
        let request = Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
        let response: Response = tokio::time::timeout(PATIENCE, self.svc.call(request)).await.expect("the service answered in time");
        let (parts, body) = response.into_parts();
        let collected = tokio::time::timeout(PATIENCE, body.collect()).await.expect("the body ended in time").expect("the body yields no error");
        let routing = parts.extensions.get::<Routing>().cloned();
        Reply { status: parts.status, headers: parts.headers, body: collected.to_bytes().to_vec(), routing }
    }

    /// The serve task, for a test that closed the app itself.
    pub async fn serving_end(self) {
        let _ = self.serving.await;
    }

    pub async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}
