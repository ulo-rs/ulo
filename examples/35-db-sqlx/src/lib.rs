use serde::{Deserialize, Serialize};
use ulo::{App, AppBuilder, BoxError, Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_config::{Config, Env, Values};
use ulo_db_sqlx::sqlx::{self, PgPool};
use ulo_db_sqlx::{Postgres, SqlxModule};
use ulo_health::HealthModule;
use ulo_health_http::HealthHttp;
use ulo_http::{Created, Json};

#[derive(Serialize)]
pub struct Note {
    pub id: i64,
    pub title: String,
}

#[derive(Deserialize)]
pub struct NewNote {
    pub title: String,
}

/// The application's queries go through the library's own pool, injected by type.
#[injectable]
pub struct Notes {
    pool: Dep<PgPool>,
}

impl Notes {
    pub async fn add(&self, title: &str) -> Result<Note, sqlx::Error> {
        let (id, title) = sqlx::query_as("insert into notes (title) values ($1) returning id, title").bind(title).fetch_one(&*self.pool).await?;
        Ok(Note { id, title })
    }

    pub async fn all(&self) -> Result<Vec<Note>, sqlx::Error> {
        let rows: Vec<(i64, String)> = sqlx::query_as("select id, title from notes order by id").fetch_all(&*self.pool).await?;
        Ok(rows.into_iter().map(|(id, title)| Note { id, title }).collect())
    }
}

#[injectable]
pub struct NotesController {
    notes: Dep<Notes>,
}

#[routes]
impl NotesController {
    #[ulo_http::post("/notes")]
    async fn create(&self, Json(new): Json<NewNote>) -> Result<Created<Json<Note>>, BoxError> {
        let note = self.notes.add(&new.title).await?;
        Ok(Created::at(format!("/notes/{}", note.id), Json(note)))
    }

    #[ulo_http::get("/notes")]
    async fn list(&self) -> Result<Json<Vec<Note>>, BoxError> {
        Ok(Json(self.notes.all().await?))
    }
}

/// The configuration is `APP_DATABASE__URL` and, optionally, the pool's other options.
pub fn config(env: Env, explicit: Values) -> Config {
    Config::builder().source(env.prefix("APP_").separator("__")).source(explicit).load()
}

pub struct AppModule {
    config: Config,
    lazy: bool,
}

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let store = SqlxModule::<Postgres>::from_config(&self.config, "database");
        if self.lazy {
            // `connect` does not wait for the database, and the migrations need it, so a lazy
            // store runs none: the schema is the deployment's.
            m.import(store.lazy());
        } else {
            m.import(store.migrate(sqlx::migrate!("./migrations")));
        }
        m.import(HealthModule::new());
        m.import(HealthHttp::new().detail(true));
        m.provide::<Notes>();
        m.controller::<NotesController>();
    }
}

/// `connect` waits for the database, up to 30 s, then applies the migrations under a lock.
pub fn builder(config: Config) -> AppBuilder {
    App::builder(AppModule { config, lazy: false })
}

/// `connect` returns at once; readiness is false until the first ping reaches the database.
pub fn lazy_builder(config: Config) -> AppBuilder {
    App::builder(AppModule { config, lazy: true })
}
