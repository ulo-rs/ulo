use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ulo::{App, AppBuilder, BoxFuture, Dep, injectable, module};
use ulo_health::{Check, Health, HealthModule};
use ulo_health_grpc::HealthGrpc;
use ulo_health_http::HealthHttp;

/// How long readiness is false before the app refuses work, so a load balancer stops sending first.
pub const LAG: Duration = Duration::from_secs(2);

/// Cold until `warm_up` is called; the check below reports it.
#[injectable]
pub struct Cache {
    #[injectable(default)]
    warm: AtomicBool,
}

impl Cache {
    pub fn warm_up(&self) {
        self.warm.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct Cold;

impl fmt::Display for Cold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the cache is cold")
    }
}

impl std::error::Error for Cold {}

/// The module that owns the cache also owns the question about it: the health module collects
/// every `dyn Check` any module contributes.
#[injectable]
pub struct CacheCheck {
    cache: Dep<Cache>,
}

impl Check for CacheCheck {
    fn name(&self) -> &str {
        "cache"
    }

    fn check(&self) -> BoxFuture<'_, Health> {
        let warm = self.cache.warm.load(Ordering::SeqCst);
        Box::pin(async move { if warm { Health::Up } else { Health::down(Cold) } })
    }
}

#[module(providers = [Cache, into dyn Check: [CacheCheck]], exports = [Cache])]
pub struct CacheModule;

#[module(imports = [
    HealthModule::new().lag(LAG),
    HealthHttp::new().detail(true),
    HealthGrpc::new(),
    CacheModule,
])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
