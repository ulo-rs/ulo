use serde::{Deserialize, Serialize};
use ulo_config::{Config, Defaults, Env, Values};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub greeting: String,
    pub host: String,
    pub ports: Ports,
}

/// One port per transport; `0` lets the OS choose.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Ports {
    pub http: u16,
    pub ws: u16,
    pub rpc: u16,
    pub grpc: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            greeting: "hello from the defaults".into(),
            host: "127.0.0.1".into(),
            ports: Ports { http: 3000, ws: 3001, rpc: 7000, grpc: 50051 },
        }
    }
}

/// Defaults, then `APP_*` from the environment `main` hands in (`APP_PORTS__WS=3101`), then the
/// values `main` sets in code.
pub fn config(env: Env, explicit: Values) -> Config {
    Config::builder().source(Defaults::of::<AppConfig>()).source(env.prefix("APP_").separator("__")).source(explicit).load()
}
