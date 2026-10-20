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
//! # TLS coverage
//!
//! MySQL over TLS is exercised: `tests/tls.rs` starts MySQL 8 with a certificate a test CA issued,
//! connects with `ssl-mode=verify_identity` and `ssl-ca` naming that CA, and shows a different CA
//! and a plaintext connect refused. What is classified follows who can fix it. A server certificate
//! rustls refuses is `Permanent`, an unknown issuer and a name mismatch among them, so a start over
//! a different CA or the wrong name fails at its first attempt; an expired or not-yet-valid one stays
//! retryable, since it reads the local clock. A root certificate (`ssl-ca`, `sslrootcert`) that
//! cannot be read or is not PEM is `Permanent` too, found by `create`'s own read of the named file. MySQL 8's `caching_sha2_password` authenticates over that
//! channel with no RSA exchange. The Postgres TLS path is not exercised by any test; SQLite has
//! none.
//!
//! # Authenticated Postgres
//!
//! SQLx's Postgres driver derives its SCRAM key on the thread that polls the connect. Measured
//! against `postgres:18` authenticating with `scram-sha-256`, over ten runs of the suite's stall
//! scenario (a one-thread runtime, a 5 ms ticker, a 50 ms bound), the longest gap between two ticks
//! was 6.6 to 7.6 ms in release (median 7.3) and 6.6 to 7.6 ms in debug (median 7.1), so the
//! derivation costs the thread about 2 ms and the connect stays where it is.
//!
//! # Pools
//!
//
//! The pool is lazy: `create` connects nothing and the start waits on `ping` under `.ready`. A pool
//! reports a refused connection as `PoolTimedOut` after its acquire timeout, so one attempt against
//! an unreachable server lasts `acquire_timeout_secs`, 5 s unless set (SQLx's own default is 30 s,
//! as long as the default total connect bound). A configuration error and a database error with
//! SQLSTATE class `28` or `3D000` (Postgres), `28000` or `42000` (MySQL) or SQLite's `CANTOPEN` are
//! `ulo::Permanent` and end the start at once, as do a `Tls` error (a TLS configuration SQLx cannot
//! use) and a server certificate rustls refuses (TLS coverage above); any other `Io`,
//! `PoolTimedOut` and `WorkerCrashed` are retried.
//!
//! SQLx opens a SQLite file read-write without creating it. `sqlite:data.db?mode=rwc` creates a
//! missing file, and a missing one without it fails the first ping as a permanent error.
//!
//! [`check_scheme`] and [`validate_url`] are public for `ulo-db-seaorm`, which validates with
//! SQLx's option parser before SeaORM sees a URL.
//!
//! # A `PoolConnection` moved off the runtime
//!
//! A `PoolConnection` returns to its pool through a task SQLx spawns in its `Drop`, and SQLx tests
//! for a tokio context at that spawn. Dropped on a thread with no tokio context, such as one user
//! code moved the connection to with `std::thread::spawn`:
//!
//! - With `runtime-tokio` alone, as this crate builds SQLx, the drop panics with "this
//!   functionality requires a Tokio context". Measured by running
//!   `ulo-db-sqlx-smol-probe`'s `tests/smol_drop.rs` without `runtime-smol`.
//! - If another crate in the build compiles SQLx's `runtime-smol` in beside it, the return task
//!   goes to smol's global executor, which starts threads the application did not ask for. Measured
//!   in `ulo-db-sqlx-smol-probe`'s `tests/smol_drop.rs`: the pool's `after_release` callback ran on a thread named `smol-*`.
//!
//! The remedy in both is to drop the connection on a tokio thread, or to hold it in a task spawned
//! on the app's runtime.
//!
//! # Migrations
//!
//! SQLx's own `Migrator` takes a lock on Postgres and MySQL and none on SQLite. `ulo-db`'s lock
//! wraps it for all three, with the primitive each engine supplies: `pg_advisory_lock`, `GET_LOCK`
//! and an OS lock on `<file>.ulo-migrate.lock`. SQLx's lock nests inside, under its own key.
//!
//! On Postgres and MySQL the lock pins one pooled connection while the migrator takes another, so
//! `max_connections` must be at least 2 when `.migrate(..)` is written. A migration using
//! `CREATE INDEX CONCURRENTLY` can deadlock against a session waiting on an advisory lock.

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod classify;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub use classify::is_permanent_error;
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
