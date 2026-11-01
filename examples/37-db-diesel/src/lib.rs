use diesel::dsl::sql;
use diesel::sql_types::BigInt;
use diesel_async::RunQueryDsl;
use ulo::{App, AppBuilder, BoxError, Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_config::{Config, Env, Values};
use ulo_db_diesel::{DieselModule, Sqlite, SqlitePool};
use ulo_http::Path;

/// SQLite is a synchronous library: a query occupies its thread until it ends. The store runs each
/// one on the blocking pool of the runtime the app was given, so the runtime's own threads keep
/// polling other tasks meanwhile.
#[injectable]
pub struct Counter {
    pool: Dep<SqlitePool>,
}

impl Counter {
    /// Counts the rows of a recursive query up to `n`: the work is SQLite's, and grows with `n`.
    pub async fn count_to(&self, n: i64) -> Result<i64, BoxError> {
        let mut conn = self.pool.get().await?;
        let query = format!("(with recursive c(x) as (select 1 union all select x + 1 from c where x < {n}) select count(*) from c)");
        Ok(diesel::select(sql::<BigInt>(&query)).get_result(&mut *conn).await?)
    }
}

#[injectable]
pub struct CountController {
    counter: Dep<Counter>,
}

#[routes]
impl CountController {
    #[ulo_http::get("/count/{n}")]
    async fn count(&self, Path(n): Path<i64>) -> Result<String, BoxError> {
        Ok(self.counter.count_to(n).await?.to_string())
    }
}

/// `APP_DATABASE__URL` names the file; it defaults to an in-memory database.
pub fn config(env: Env, explicit: Values) -> Config {
    Config::builder()
        .source(Values::new().set("database.url", ":memory:"))
        .source(env.prefix("APP_").separator("__"))
        .source(explicit)
        .load()
}

pub struct AppModule {
    config: Config,
}

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(DieselModule::<Sqlite>::from_config(&self.config, "database"));
        m.provide::<Counter>();
        m.controller::<CountController>();
    }
}

pub fn builder(config: Config) -> AppBuilder {
    App::builder(AppModule { config })
}
