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
//! The pool is lazy on all three engines, so the start waits on `.ready` unless the module is
//! `.lazy()`. An attempt's bound should exceed `acquire_timeout_secs`, which SQLx's 30 s default
//! applies when unset on Postgres and MySQL.
//!
//! SQLite is built through `ulo-db-sqlx`'s lazy pool and wrapped by
//! `SqlxSqliteConnector::from_sqlx_sqlite_pool`, because SeaORM's own SQLite connector queries
//! `sqlite_version()` while connecting (`sea-orm-migration` turns that feature on for every build)
//! and so would connect in `create`. The store's three pool options map onto SQLx's:
//! `max_connections` (1 when unset, as SeaORM's connector defaults it), `min_connections`, and
//! `acquire_timeout_secs` (5 s when unset, as `ulo-db-sqlx`'s pools). SeaORM's other `ConnectOptions`
//! knobs are not exposed and do not apply to the wrapped pool, so statement logging follows SQLx's
//! defaults. The wrapped path skips SeaORM's connect-time version check; the store's `ping` makes it
//! instead and refuses a SQLite older than 3.35, which `RETURNING` needs, as `Permanent`. An old
//! SQLite therefore fails the start under `.ready` and the first health check under `.lazy()`.
//!
//! Migrations run through `sea-orm-migration`'s `up` under `ulo-db`'s lock, taken with the
//! primitives `ulo-db-sqlx` supplies for each engine on the SQLx pool SeaORM holds.
//!
//! # TLS coverage
//!
//! No test exercises a TLS connection through this store. On Postgres and MySQL SeaORM builds its
//! own SQLx pool, over the SQLx and TLS features `ulo-db-sqlx` shares, so `ulo-db-sqlx`'s MySQL TLS
//! test exercises SQLx's handshake but not this store's pool or its error classification; only
//! SQLite uses `ulo-db-sqlx`'s pool.
//!
//! # A pooled connection moved off the runtime
//!
//! SeaORM holds an SQLx pool, so what `ulo-db-sqlx` documents applies. An SQLx `PoolConnection`
//! dropped on a thread with no tokio context, such as one user code moved to a plain
//! `std::thread`, panics in its `Drop` with "this functionality requires a Tokio context" when SQLx
//! is built with `runtime-tokio` alone. If another crate in the build compiles SQLx's
//! `runtime-smol` in beside it, the connection returns to its pool through a task spawned on smol's
//! global executor, which starts threads the application did not ask for. Both are measured by
//! `ulo-db-sqlx`'s `tests/smol_drop.rs`. The remedy is to drop the connection on a tokio thread, or
//! to hold it in a task spawned on the app's runtime.

mod migrate;
mod options;
mod store;

pub use migrate::{SeaOrmLock, SeaOrmMigrations};
pub use options::SeaOrmOptions;
pub use store::SeaOrm;

/// `StoreModule` for this crate's store.
pub type SeaOrmModule = ulo_db::StoreModule<SeaOrm>;
