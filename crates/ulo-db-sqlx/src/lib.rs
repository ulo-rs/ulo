//! SQLx stores for `ulo-db` (integrations DESIGN §4.6): [`Postgres`], [`MySql`] and [`Sqlite`] as
//! [`Store`](ulo_db::Store)s over SQLx 0.9 with `runtime-tokio` alone.
//!
//! ```ignore
//! m.import(SqlxModule::<Postgres>::from_config(&config, "database")
//!     .migrate(sqlx::migrate!("./migrations")));
//! ```
//!
//! The application reads a bare `Dep<sqlx::PgPool>`. This crate re-exports `sqlx`, and an
//! application names it as `ulo_db_sqlx::sqlx`: `ulo-db-seaorm` shares the workspace's one SQLx, and
//! a second copy in a build is a different `TypeId` with the same path, which `wire()` reports as a
//! missing binding naming two versions of one crate.
//!
//! # Features
//!
//! `postgres`, `mysql` and `sqlite` select the engines. `tls`, on by default, enables SQLx's
//! `tls-rustls-ring-native-roots`: rustls over ring, the provider the rest of the workspace uses,
//! with the platform's roots. `rumqttc` brings aws-lc-rs, so an application linking `ulo-rpc-mqtt`
//! links both providers whatever this crate enables; that is MQTT's limit.
//!
//! # Migrations
//!
//! SQLx's own `Migrator` takes a lock on Postgres and MySQL and none on SQLite. `ulo-db`'s lock
//! wraps it for all three, with the primitive each engine supplies: `pg_advisory_lock`, `GET_LOCK`
//! and an OS lock on `<file>.ulo-migrate.lock`. SQLx's lock nests inside, under its own key.

mod classify;
mod migrate;
mod options;
#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

pub use sqlx;

pub use options::{Engine, MySqlOptions, PgOptions, SqliteOptions, check_scheme, validate_url};
#[cfg(feature = "mysql")]
pub use mysql::{MySql, MySqlLock};
#[cfg(feature = "postgres")]
pub use postgres::{PgLock, Postgres};
#[cfg(feature = "sqlite")]
pub use sqlite::{Sqlite, SqliteLock};

/// `StoreModule` for this crate's stores: `SqlxModule::<Postgres>`.
pub type SqlxModule<S> = ulo_db::StoreModule<S>;
