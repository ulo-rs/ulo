use ulo::{App, AppBuilder, Dep, Many, injectable, module, routes};

pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}

macro_rules! plugin {
    ($($ty:ident => $name:literal),*) => {$(
        #[injectable]
        pub struct $ty;
        impl Plugin for $ty {
            fn name(&self) -> &'static str {
                $name
            }
        }
    )*};
}

plugin!(MetricsPlugin => "metrics", TracingPlugin => "tracing", ExtraPlugin => "extra");

/// Bound by nothing in `AppModule`, so every read of it below is `None`.
#[injectable]
pub struct AuditLog;

pub trait Mailer: Send + Sync {}

#[injectable]
pub struct Host {
    pub plugins: Many<dyn Plugin>,
    pub audit: Option<Dep<AuditLog>>,
    pub mailer: Option<Dep<dyn Mailer>>,
}

#[injectable]
pub struct HostController {
    host: Dep<Host>,
}

#[routes]
impl HostController {
    #[ulo_http::get("/plugins")]
    fn plugins(&self) -> String {
        self.host.plugins.iter().map(|p| p.name()).collect::<Vec<_>>().join(",")
    }

    #[ulo_http::get("/optional")]
    fn optional(&self) -> String {
        format!("audit={} mailer={}", self.host.audit.is_some(), self.host.mailer.is_some())
    }
}

/// Contributes to the collection without knowing what else does.
#[module(providers = [into dyn Plugin: [ExtraPlugin]])]
pub struct ExtrasModule;

#[module(
    imports = [ExtrasModule],
    providers = [Host, into dyn Plugin: [MetricsPlugin, TracingPlugin]],
    controllers = [HostController],
)]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
