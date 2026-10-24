//! The Diesel store for `ulo-db` (integrations DESIGN §4.6): a deadpool `Pool` of diesel-async
//! connections for Postgres and MySQL, and for SQLite through `SyncConnectionWrapper`.
//!
//! ```ignore
//! m.import(DieselModule::<Postgres>::from_config(&config, "database")
//!     .migrate(DieselMigrations::new(diesel_migrations::embed_migrations!())));
//! ```
//!
//! One feature per engine, `postgres`, `mysql` and `sqlite`, each bringing its store. Every store
//! needs the app to run on tokio, and an app on any other runtime fails `connect` with
//! `ulo_db::UnsupportedRuntime` inside `ulo::Permanent`.
//!
//! diesel-async's Postgres and MySQL connections are async, so no query blocks a worker. The crate's
//! blocking path is `Given`, a `SpawnBlocking` over the handle of the runtime the app was given: the
//! name check proves the app's runtime is tokio's, not that the thread polling a query has tokio's
//! context, and diesel-async's default `SpawnBlocking` builds a hidden runtime where it finds none,
//! a runtime found rather than given. The migrations run through `Given` on that runtime's blocking
//! pool.
//!
//! `build` of deadpool's pool connects nothing, so `.lazy()` is offered. diesel-async's errors lose
//! the SQLSTATE, so wrong credentials and an unknown database arrive alike as
//! `DatabaseError(Unknown, message)`, and the store tells a permanent failure from a retryable one
//! by the message. A server writing its messages in a language other than English reports bad
//! credentials in text the store does not recognise, and the start then retries them to the connect
//! bound.
//!
//! `validate` parses a Postgres URL with tokio-postgres's parser and a MySQL URL with mysql_async's,
//! the parsers diesel-async connects through. A SQLite location is a path, `:memory:`, a `file:`
//! URI or `sqlite://<path>`.
//!
//! `.migrate(..)` runs `diesel_migrations`, which takes no lock, under `ulo-db`'s migration lock:
//! `pg_advisory_lock` and `GET_LOCK` on a session of their own, and for a SQLite file an OS lock on
//! `<file>.ulo-migrate.lock` beside it, which is left in place. A migration using
//! `CREATE INDEX CONCURRENTLY` can deadlock against a session waiting on the advisory lock.
//!
//! The SQLite wait polls the file lock and makes its last attempt 10 ms before the bound, so that
//! its answer reaches `ulo-db`'s clock first. A lock freed in those last 10 ms is missed by design:
//! the start fails with the retryable lock timeout, and the restart takes the lock. A wait whose
//! thread panics is `Permanent` and logged at `error` with the panic's message.
//!
//! # TLS coverage
//!
//! No test exercises a TLS connection through this store, for Postgres or MySQL.

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
