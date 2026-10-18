//! The SeaORM store for `ulo-db` (integrations DESIGN §4.2, §4.6): `DatabaseConnection` as the
//! client, with the three SQL engines' URL schemes.
//!
//! ```ignore
//! m.import(SeaOrmModule::from_config(&config, "database").migrate(SeaOrmMigrations::<Migrator>::new()));
//! ```
//!
//! SeaORM's `Database::connect` writes the whole URL, password included, into its parse and
//! no-driver errors. The store therefore validates with SQLx's option parser, which SeaORM's SQL
//! drivers use underneath, after the scheme check it shares with `ulo-db-sqlx`, so no malformed URL
//! reaches `Database::connect`. The whole URL and its password component are both registered as
//! secrets, and the options' `Debug` writes the URL redacted.
//!
//! # Features
//!
//! `postgres`, `mysql` and `sqlite` select the engines, and with none on the crate accepts no URL.
//! `tls`, on by default, is `ulo-db-sqlx`'s: SQLx over rustls with ring. The application depends on
//! `sea-orm` 2.0 itself for entities and queries, with the same SQLx the build already holds.
//!
//! # Connecting and migrating
//!
//! The pool is lazy, so the start waits on `.ready` unless the module is `.lazy()`. An attempt's
//! bound should exceed `acquire_timeout_secs`, which SQLx's 30 s default applies when unset.
//! Migrations run through `sea-orm-migration`'s `up` under `ulo-db`'s lock, taken with the
//! primitives `ulo-db-sqlx` supplies for each engine on the SQLx pool SeaORM holds.

mod migrate;
mod options;
mod store;

pub use migrate::{SeaOrmLock, SeaOrmMigrations};
pub use options::SeaOrmOptions;
pub use store::SeaOrm;

/// `StoreModule` for this crate's store.
pub type SeaOrmModule = ulo_db::StoreModule<SeaOrm>;
