//! Four transports in one app: the controllers of 15, 23, 25 and 29 over one store, one
//! configuration and one set of health probes, closed by one `close`.

pub mod config;
pub mod grpc;
pub mod http;
pub mod rpc;
pub mod store;
pub mod ws;

use std::time::Duration;

use ulo::{App, AppBuilder, Module, ModuleDef, ModuleIdentity, module};
use ulo_config::{Config, ConfigModule};
use ulo_health::HealthModule;
use ulo_health_http::HealthHttp;
use ulo_ws::WsModule;

pub use config::{AppConfig, Ports, config};
pub use store::{Gate, Store};

/// How long readiness is false before the app stops accepting (34).
pub const LAG: Duration = Duration::from_secs(2);
/// The window in-flight executions on every transport share (10).
pub const DRAIN: Duration = Duration::from_secs(5);

#[module(imports = [WsModule::for_root(), HealthModule::new().lag(LAG), HealthHttp::new().detail(true)])]
pub struct Services;

/// Holds the `Config` because `ConfigModule` reads its sections at registration (33). The
/// controllers are registered here, beside the configuration they read: a sibling module's
/// controller cannot see what `ConfigModule` exports.
pub struct AppModule {
    config: Config,
}

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(ConfigModule::<AppConfig>::new(&self.config).deny_unread());
        m.import(Services);
        m.import(store::StoreModule);
        m.controller::<http::HttpApi>();
        m.controller::<ws::WsApi>();
        m.controller::<rpc::RpcApi>();
        m.controller::<grpc::GrpcApi>();
    }
}

pub fn builder(config: Config) -> AppBuilder {
    App::builder(AppModule { config }).drain_timeout(DRAIN)
}
