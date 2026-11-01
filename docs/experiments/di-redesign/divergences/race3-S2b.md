# race 3, S2b: `ulo-db-diesel`

## Points resolved

- **How diesel-async's errors are classified: by matched messages, read after the error's type.**
  The driver's own connect in `ping`, which PROBES_db.md §3.6 recommends, cannot be written:
  `ping` holds only the pool, and `AsyncDieselConnectionManager` keeps its URL in a private field
  with no accessor (`diesel-async` 0.9.2, `src/pooled_connection/mod.rs:120-123`). So
  `classify::permanent_or_not` downcasts the error through deadpool's `PoolError`, diesel-async's
  `PoolError`, `ConnectionError` and `diesel::result::Error`; `InvalidCString` and
  `InvalidConnectionUrl` are permanent by variant; a `DatabaseError` or `BadConnection` message is
  matched, lowercased, against fragments: `authentication failed`, `no pg_hba.conf entry`,
  `is not permitted to log in`, `permission denied for database`, `password missing`,
  `invalid connection string`, `invalid configuration` (tokio-postgres's own refusals, which arrive
  as `UnableToSendCommand` with the error's text, `src/pg/error_helper.rs:50-53`),
  `access denied for user`, `unknown database`, `unable to open database file`,
  `file is not a database`, plus Postgres's `database "…" does not exist` and `role "…" does not
  exist`, anchored at the start so a missing relation stays retryable. Everything else is
  retryable, `UnableToSendCommand` and `BadConnection` included. The limit is in the crate's docs:
  a server whose `lc_messages` is not English translates the messages, and bad credentials are
  then retried to the connect bound. The Postgres texts for 28P01 and 3D000 and the MySQL texts for
  1045 and 1049 are PROBES_db.md §3.6 and §8.2's; the other fragments are not probed.
- **How the SQLite sidecar lock is taken off the worker: on the given runtime's blocking pool,
  polling `try_lock`.** `lock` reads the `main` database's file from
  `pragma_database_list` on a pooled connection (an absolute path, so replicas with different
  relative paths meet on one sidecar; empty for an in-memory or temporary database, which takes no
  lock), then `Given::run` opens `<file>.ulo-migrate.lock` and calls `File::try_lock` every 50 ms
  until the lock is held or the bound passes. `File::lock` would wait with no bound, and a
  blocking-pool thread cannot be cancelled when `ulo-db` gives up at its bound; the polling thread
  ends by itself at the bound. The sidecar is never deleted. `File::try_lock` and `TryLockError`
  are stable from 1.89 (read in the toolchain's `std/src/fs.rs`).
- **What `Given::get_runtime` does: it panics, naming why.** A `Given` is the handle of the
  runtime the app was given; a function with no argument has none to read, and looking one up or
  building one is what R4 replaces. The crate reaches neither caller (`SyncConnectionWrapper::new`,
  `AsyncConnection::establish`, `src/sync_connection_wrapper/mod.rs:225`, `:327`). A user who names
  `SyncConnectionWrapper<SqliteConnection, Given>` and calls either reaches the panic. The design
  gives no typed error on this path, and the trait gives the function no way to return one.

## Decisions the design leaves open

- **The migration lock's Postgres and MySQL session comes from `pool.manager().create()`**, a
  connection made by the pool's manager outside the pool (deadpool's `Manager::create`, public;
  `Pool::manager`, `deadpool` 0.13.1 `src/managed/pool.rs:372`). A pool of one keeps its
  connection free for the migrator, which `ulo-db-sqlx` refuses with `require_spare_connection`
  instead, and the session holding the lock is closed, never returned to a pool.
- **Postgres's wait is held to the bound on the server too:** `SET lock_timeout = <bound in ms>`
  on the lock's session before `pg_advisory_lock`, so a session `ulo-db` gives up on stops waiting.
  Not verified against a server: that `lock_timeout` applies to `pg_advisory_lock` is Postgres
  behaviour, not in the registry. `ulo-db`'s own bound holds either way.
- **The MySQL lock name is `ulo-db-sqlx`'s exactly:** `'ulo-db.migrate.' + database()`, and past 64
  characters `'ulo-db.migrate.#'` with the FNV-1a hash of the database name in 16 hex digits, read
  in D2's `crates/ulo-db-sqlx/src/mysql.rs` as it stood during this stage, so a SQLx store and a
  Diesel store migrating one database take one lock. If D2's spelling changes, this one should
  follow. `GET_LOCK`'s timeout is the bound in whole seconds, rounded up.
- **Migrations run on a pooled connection,** moved into a `Given::run` closure and wrapped there in
  `AsyncConnectionWrapper<Object<C>>`, so `AsyncConnectionWrapper::from` calls tokio's
  `BlockOn::get_runtime` on the blocking-pool thread. An in-memory SQLite database with a pool of
  one is then migrated on the connection the application uses. The source is passed as a private
  `Shared<M>` over the module's `Arc<DieselMigrations<M>>`, since `run_pending_migrations` takes its
  source by value.
- **`ping` is `pool.get()` then `SELECT 1`,** both errors through `permanent_or_not`.
- **`close` is deadpool's `Pool::close`,** which returns at once; a connection still checked out
  closes when its holder drops it. For SQLite the call runs through `Given::run`, since it drops the
  idle connections where it is called and a SQLite close can checkpoint the write-ahead log.
- **`validate`:** Postgres requires `postgres://` or `postgresql://`, then tokio-postgres's
  `Config::from_str`, then a host or `hostaddr`; MySQL requires `mysql://`, then
  `mysql_async::Opts::from_url`. Each is the parser diesel-async's `establish` runs
  (`src/pg/mod.rs:282`, `src/mysql/mod.rs:341`). The schemes are compared case-sensitively, as
  those parsers compare them. A parser's text is kept in the problem only while it holds none of
  the URL, its userinfo or a password form, as D2 does. SQLite accepts a path, `:memory:`, a
  `file:` URI and `sqlite://<path>` (diesel rewrites `sqlite://` to `file:`, read in `diesel`
  2.3.14 `src/sqlite/connection/raw.rs:46-50`), and refuses an empty location, a NUL byte, another
  `scheme://`, and `sqlite:<path>`, which diesel opens as a file named after the whole text.
  `max_size` zero is refused at `max_size`; `wait_timeout_secs` zero is deadpool's "do not wait".
- **`secrets`:** the password as written and percent-decoded, from the userinfo and from a
  `password=` query parameter, as D2's `password_forms`; none for SQLite.
- **`wait_timeout_secs` set** gives the pool `deadpool::Runtime::Tokio1`, which deadpool needs for
  any timeout (`src/managed/builder.rs:90-99`); unset, the pool takes no runtime.

## Divergences

- **`Cargo.toml` gains `tokio-postgres` and `mysql_async`, optional, behind `postgres` and `mysql`.**
  The plan's manifest line names `diesel`, `diesel-async` and `diesel_migrations` alone. They are
  declared as diesel-async 0.9.2 declares them (`tokio-postgres` 0.7.10, `mysql_async` 0.36.0 with
  `default-features = false` and `minimal-rust`), both already in the lock file through
  diesel-async, so the build holds one copy of each. They are crate-local entries, not workspace
  ones, as `ulo`'s `async-lock` is. Without them `validate` could check the scheme alone, and a
  malformed URL would pass `wire` and fail `connect`, against §4.2 and the first scenario of §4.9.
  Dropping them, if the coordinator prefers, removes `validate_postgres`'s and `validate_mysql`'s
  parse step and nothing else.
- **`tokio` gains `rt-multi-thread`,** for `block_in_place` (next entry). diesel-async's
  `migrations` and `sqlite` features enable it already.
- **A SQLite query dropped before its blocking task ends waits for that task on a multi-thread
  runtime.** The design states `Given::spawn_blocking` as `handle.spawn_blocking(..)` alone. The
  task holds a clone of the connection's `Arc<Mutex<_>>`; a connection back in the pool while the
  clone lives makes deadpool's recycle check panic, since `is_broken` reaches
  `exclusive_connection`, which panics on a shared `Arc` (`src/sync_connection_wrapper/mod.rs:450-466`,
  `:473-480`, `src/transaction_manager.rs:138-147`). diesel-async's own tokio `SpawnBlocking` waits
  in `Drop` through `block_in_place` and `Handle::block_on` for that reason (`:529-557`). `Given` does the same through a private
  `Joined` future, and on a current-thread runtime, where `block_in_place` panics, it does not
  wait, leaving that window open. `block_in_place` hands the worker's other tasks to another thread
  for the wait. The migrations, the lock and the close go through `Given::run`, which never waits
  on drop.
- **`Given` is compiled for every engine, not under `sqlite` alone.** The migrations of Postgres and
  MySQL run through it. Its re-export from `lib.rs` stays under `sqlite`, so the public surface is
  as the skeleton gives it; with only `postgres` or `mysql` it is a `pub` item in a private module.
  It gains two `pub(crate)` functions, `current()` and `run(..)`.
- **For Postgres and MySQL the migrations' `Given` is not the one `create` captured.** R4 has the
  migrations run "through the same `Given`". The pool type is frozen as `Pool<C>` and keeps no value
  of the crate's, so nothing carries `create`'s handle to `Migrations::run`. The migrations, the
  lock and the SQLite close read the handle with `Handle::try_current()` inside the task `ulo-db`
  runs them in on the given runtime: D1's `run_locked` spawns lock, migrations and unlock as one
  task through `spawn_with`, and its `close` runs through `spawn_with` (read in D1's
  `crates/ulo-db/src/migrate.rs` and `connect.rs` during this stage). That is the reading `create`
  makes. Outside a runtime, `Given::current` answers a `Permanent` error, never a hidden runtime.
  SQLite's connections each carry a `Given` over `create`'s handle, and the SQLite migration's
  queries go through it.
- **`create` reads the handle with `Handle::try_current()`, not `Handle::current()`,** so a `create`
  polled off a runtime fails with a `Permanent` error instead of panicking.
- **`DieselLock`'s doc says "the dedicated connection", not "a pooled connection",** for the
  manager-made session above, and states that dropping it releases the lock.
- **`DieselMigrations::source`'s `#[allow(dead_code)]` becomes a `cfg_attr`** applying only with no
  engine feature, where it is still unread.

## Hazards recorded

- When `ulo-db`'s migration bound passes, D1 aborts the task and drops the lock's guard, releasing
  the lock, while a `diesel_migrations` run already on its blocking-pool thread continues to its
  end. The start has then failed with `MigrationFailed`; a second replica can take the lock while
  the first one's migration is still running. Waiting for the thread in `Drop` would close that
  window by holding a worker for the rest of the migration, which the rules forbid.
- `pragma_database_list` needs SQLite 3.16.0 or later. Not verified against the `libsqlite3-sys`
  the workspace resolves.
- MySQL 8's `caching_sha2_password` without TLS through mysql_async built with no TLS feature: not
  probed for Diesel. Whatever it returns that matches no fragment is retried to the bound.

## Requests

- **Coordinator:** `Cargo.lock` gains `tokio-postgres` and `mysql_async` in `ulo-db-diesel`'s
  dependency list; both packages are already locked through diesel-async.

## Replaced on `master`, not carried over

Nothing of `master`'s crate is carried over; S-skel emptied it. The diesel-async 0.8 module over
`CheckedModule`, its pool wrapper, its health indicator and its redaction are replaced by the
`Store` impls, `ulo-db`'s check and `Secret` with `Store::secrets`.
