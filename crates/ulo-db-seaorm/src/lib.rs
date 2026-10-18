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

mod migrate;
mod options;
mod store;

pub use migrate::{SeaOrmLock, SeaOrmMigrations};
pub use options::SeaOrmOptions;
pub use store::SeaOrm;

/// `StoreModule` for this crate's store.
pub type SeaOrmModule = ulo_db::StoreModule<SeaOrm>;
