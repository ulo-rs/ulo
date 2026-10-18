//! The Diesel store for `ulo-db` (integrations DESIGN §4.6): a deadpool `Pool` of diesel-async
//! connections for Postgres and MySQL, and for SQLite through `SyncConnectionWrapper`.
//!
//! ```ignore
//! m.import(DieselModule::<Postgres>::from_config(&config, "database")
//!     .migrate(DieselMigrations::new(diesel_migrations::embed_migrations!())));
//! ```
//!
//! diesel-async's Postgres and MySQL connections are async, so no query blocks a worker. The crate's
//! blocking path is `Given`, a `SpawnBlocking` over the handle of the runtime the app was given: the
//! name check proves the app's runtime is tokio's, not that the thread polling a query has tokio's
//! context, and diesel-async's default `SpawnBlocking` builds a hidden runtime where it finds none,
//! a runtime found rather than given. The migrations run through the same `Given` on that runtime's
//! blocking pool.
//!
//! `build` of deadpool's pool connects nothing, so `.lazy()` is offered. diesel-async's errors lose
//! the SQLSTATE, so wrong credentials and an unknown database arrive alike as
//! `DatabaseError(Unknown, message)`; the classification is in `classify`.

mod classify;
mod lock;
mod migrate;
mod options;
mod store;

pub use lock::DieselLock;
pub use migrate::DieselMigrations;
pub use options::DieselOptions;
#[cfg(feature = "sqlite")]
pub use store::Given;
#[cfg(feature = "mysql")]
pub use store::{MySql, MySqlPool};
#[cfg(feature = "postgres")]
pub use store::{PgPool, Postgres};
#[cfg(feature = "sqlite")]
pub use store::{Sqlite, SqlitePool};

/// `StoreModule` for this crate's stores: `DieselModule::<Postgres>`.
pub type DieselModule<S> = ulo_db::StoreModule<S>;
