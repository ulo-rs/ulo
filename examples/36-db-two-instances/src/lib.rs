use serde::{Deserialize, Serialize};
use ulo::{App, AppBuilder, BoxError, Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_config::{Config, Env, Values};
use ulo_db_sqlx::sqlx::{self, PgPool};
use ulo_db_sqlx::{Postgres, SqlxModule};
use ulo_health::HealthModule;
use ulo_health_http::HealthHttp;
use ulo_http::{Created, Json};

/// The two qualifiers that tell the pools apart.
pub struct Primary;
pub struct Replica;

#[derive(Serialize)]
pub struct Note {
    pub id: i64,
    pub title: String,
}

#[derive(Deserialize)]
pub struct NewNote {
    pub title: String,
}

async fn all(pool: &PgPool) -> Result<Vec<Note>, sqlx::Error> {
    let rows: Vec<(i64, String)> = sqlx::query_as("select id, title from notes order by id").fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, title)| Note { id, title }).collect())
}

/// Reads and writes the primary.
#[injectable]
pub struct PrimaryNotes {
    pool: Dep<PgPool, Primary>,
}

impl PrimaryNotes {
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn add(&self, title: &str) -> Result<Note, sqlx::Error> {
        let (id, title) = sqlx::query_as("insert into notes (title) values ($1) returning id, title").bind(title).fetch_one(&*self.pool).await?;
        Ok(Note { id, title })
    }

    pub async fn all(&self) -> Result<Vec<Note>, sqlx::Error> {
        all(&self.pool).await
    }
}

/// Reads the replica.
#[injectable]
pub struct ReplicaNotes {
    pool: Dep<PgPool, Replica>,
}

impl ReplicaNotes {
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn all(&self) -> Result<Vec<Note>, sqlx::Error> {
        all(&self.pool).await
    }
}

#[injectable]
pub struct NotesController {
    primary: Dep<PrimaryNotes>,
    replica: Dep<ReplicaNotes>,
}

#[routes]
impl NotesController {
    #[ulo_http::post("/notes")]
    async fn create(&self, Json(new): Json<NewNote>) -> Result<Created<Json<Note>>, BoxError> {
        let note = self.primary.add(&new.title).await?;
        Ok(Created::at(format!("/notes/{}", note.id), Json(note)))
    }

    #[ulo_http::get("/primary/notes")]
    async fn read_primary(&self) -> Result<Json<Vec<Note>>, BoxError> {
        Ok(Json(self.primary.all().await?))
    }

    #[ulo_http::get("/replica/notes")]
    async fn read_replica(&self) -> Result<Json<Vec<Note>>, BoxError> {
        Ok(Json(self.replica.all().await?))
    }
}

/// `APP_PRIMARY__URL` and `APP_REPLICA__URL` name the two databases.
pub fn config(env: Env, explicit: Values) -> Config {
    Config::builder().source(env.prefix("APP_").separator("__")).source(explicit).load()
}

pub struct AppModule {
    config: Config,
}

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        // One store module, read from two sections and keyed twice: `Dep<PgPool, Primary>` and
        // `Dep<PgPool, Replica>`, and two checks. Only the primary migrates; a replica's schema
        // arrives by replication.
        m.import(SqlxModule::<Postgres>::from_config(&self.config, "primary").migrate(sqlx::migrate!("./migrations")).keyed::<Primary>());
        m.import(SqlxModule::<Postgres>::from_config(&self.config, "replica").keyed::<Replica>());
        m.import(HealthModule::new());
        m.import(HealthHttp::new().detail(true));
        m.provide::<PrimaryNotes>();
        m.provide::<ReplicaNotes>();
        m.controller::<NotesController>();
    }
}

pub fn builder(config: Config) -> AppBuilder {
    App::builder(AppModule { config })
}
