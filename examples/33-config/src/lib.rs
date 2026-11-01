use std::path::Path;

use serde::{Deserialize, Serialize};
use ulo::{App, AppBuilder, Dep, Module, ModuleDef, ModuleIdentity, Secret, injectable, routes};
use ulo_config::{Config, ConfigModule, Defaults, DotEnv, Env, File, Values};
use ulo_http::Json;

#[derive(Clone, Debug, Deserialize)]
pub struct AppConfig {
    pub greeting: String,
    pub http: Http,
    pub database: Database,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Http {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Database {
    pub url: Secret<DbUrl>,
    pub pool_size: u32,
    pub timeout_secs: u32,
}

/// A value type that quotes its input when it refuses it, as `Url` and `IpAddr` do. The `Secret`
/// around it is what keeps the quotation out of the report.
#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "String")]
pub struct DbUrl(String);

impl DbUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DbUrl {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        if text.starts_with("postgres://") { Ok(DbUrl(text)) } else { Err(format!("`{text}` is not a postgres:// url")) }
    }
}

/// The bottom layer. `Defaults::of` needs `Serialize`, which a `Secret` does not have, so the
/// defaults are a type of their own and the URL has none.
#[derive(Serialize)]
struct Fallbacks {
    greeting: &'static str,
    http: HttpFallbacks,
    database: DatabaseFallbacks,
}

#[derive(Serialize)]
struct HttpFallbacks {
    host: &'static str,
    port: u16,
}

#[derive(Serialize)]
struct DatabaseFallbacks {
    pool_size: u32,
    timeout_secs: u32,
}

impl Default for Fallbacks {
    fn default() -> Self {
        Fallbacks {
            greeting: "hello from the defaults",
            http: HttpFallbacks { host: "0.0.0.0", port: 3000 },
            database: DatabaseFallbacks { pool_size: 5, timeout_secs: 30 },
        }
    }
}

/// Defaults, then `app.toml`, then `.env`, then the environment `main` hands in, then the values
/// `main` sets in code: each source overrides the ones before it.
pub fn config(dir: &Path, env: Env, explicit: Values) -> Config {
    Config::builder()
        .source(Defaults::of::<Fallbacks>())
        .source(File::toml(dir.join("app.toml")))
        .source(DotEnv::file(dir.join(".env")).optional().prefix("APP_").separator("__"))
        .source(env.prefix("APP_").separator("__"))
        .source(explicit)
        .load()
}

#[derive(Serialize)]
pub struct Settings {
    pub greeting: String,
    pub host: String,
    pub port: u16,
    pub pool_size: u32,
    pub timeout_secs: u32,
    pub database_url: String,
}

#[injectable]
pub struct SettingsController {
    config: Dep<AppConfig>,
}

#[routes]
impl SettingsController {
    #[ulo_http::get("/settings")]
    fn settings(&self) -> Json<Settings> {
        let c = &*self.config;
        Json(Settings {
            greeting: c.greeting.clone(),
            host: c.http.host.clone(),
            port: c.http.port,
            pool_size: c.database.pool_size,
            timeout_secs: c.database.timeout_secs,
            database_url: format!("{:?}", c.database.url),
        })
    }
}

/// Holds the `Config` because `ConfigModule` reads its sections at registration.
pub struct AppModule {
    config: Config,
}

impl AppModule {
    pub fn new(config: Config) -> Self {
        AppModule { config }
    }
}

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(ConfigModule::<AppConfig>::new(&self.config).deny_unread());
        m.controller::<SettingsController>();
    }
}

pub fn builder(config: Config) -> AppBuilder {
    App::builder(AppModule::new(config))
}
