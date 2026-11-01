# Divergences: race 3, D2 (`ulo-db-sqlx`)

Files written: `crates/ulo-db-sqlx/Cargo.toml`, `src/lib.rs`, `src/options.rs`, `src/classify.rs`,
`src/postgres.rs`, `src/mysql.rs`, `src/sqlite.rs`, `src/migrate.rs`. No cargo was run. SQLx 0.9.0
items used were read in the registry (`sqlx-core`, `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite`
0.9.0); `File::try_lock`, `File::unlock` and `TryLockError` were read in the local toolchain's
`std/src/fs.rs`. Every public signature is as the skeleton gave it.

## Divergences

1. **A parse failure never prints the URL.** The design writes the problem "from its category, with the
   userinfo replaced by the redaction marker". The problem text here omits the URL altogether, so
   nothing of a malformed URL, userinfo or not, reaches a message. The text carries SQLx's own
   configuration detail (`invalid port number`) only while it contains none of the URL, its userinfo
   or a password in either spelling (`options.rs`, `leaks`); otherwise the category alone. Reason: a
   password that sits outside the authority (an unescaped `/`, a `?password=` parameter) is not
   found by any userinfo replacement.
2. **`acquire_timeout` defaults to 5 s, not SQLx's 30 s.** Reason: the design requires an attempt's
   bound above the pool's `acquire_timeout` and the default total bound is 30 s, so with SQLx's
   default one attempt against a refused connection (reported only as `PoolTimedOut`, PROBES_db.md
   §8.1) consumes the whole bound and `.retries(..)` never gets a second one. This also changes
   what a handler waits for a pooled connection under load; `acquire_timeout_secs` overrides it.
3. **`validate` checks more than the URL.** `max_connections = 0`, `min_connections` above
   `max_connections` (10 when unset) and `acquire_timeout_secs = 0` are problems at their own keys.
   An engine whose feature is off is a problem at `url` naming the feature.
4. **The Postgres and MySQL migration lock refuses a pool of one connection.** The lock pins a
   pooled connection while SQLx's migrator takes another from the same pool, so `max_connections = 1`
   would wait on itself until the acquire timeout. `lock` returns an error naming the setting. It is
   not a `validate` problem because `validate` cannot know whether `.migrate(..)` is written.
5. **Lock connections are closed, not pooled.** The design says each primitive is "released when the
   connection dies". Here the connection is marked `close_on_drop` the moment it is acquired, and
   `unlock` releases explicitly and then `close()`s it. A `lock` future that `ulo-db` drops at its
   bound, or a guard dropped without `unlock`, therefore never returns a lock-holding session to the
   pool. Cost: one reconnect per migration run.
6. **Postgres bounds the wait on the server.** `pg_advisory_lock` has no timeout argument, and
   dropping the awaiting future would leave the statement blocked on a session. `lock` sets the
   session's `lock_timeout` to `bound` (millisecond floor of 1, since 0 disables it) and maps
   SQLSTATE `55P03` to "not obtained within the bound". Not verified here: that `lock_timeout`
   interrupts an advisory-lock wait (PostgreSQL's documentation says it applies to any lock wait;
   no probe was run). If it does not, `ulo-db`'s outer bound is the only bound and the session is
   closed on drop.
7. **MySQL's `GET_LOCK` timeout is whole seconds**, `bound` rounded up.
8. **SQLite's lock path comes from SQLite.** `lock` runs `select file from pragma_database_list where
   name = 'main'` on a pool connection and appends `.ulo-migrate.lock` to the answer. An empty answer
   (in-memory, temporary) takes no lock. Reason: SQLx keeps `in_memory` private and
   `to_url_lossy` builds a URL with `expect`, which `:memory:`'s `file:sqlx-in-memory-N` filename
   would trip. Consequence: the SQLite lock needs a working connection, so a missing file without
   `mode=rwc` fails here as `CANTOPEN` before any migration.
9. **The SQLite lock polls.** `File::lock` blocks without a bound, and a thread blocked in it after
   its future was dropped would hold the lock indefinitely. The thread polls `try_lock` every 50 ms
   up to `bound` and hands the file back through a `futures-channel` oneshot; a dropped receiver
   drops the file and releases the lock. This is the answer to "how the SQLite sidecar lock is taken
   off the worker".
10. **In-memory SQLite pools disable the idle and lifetime reapers** (`idle_timeout(None)`,
    `max_lifetime(None)`), found by reading the URL the way SQLx's `FromStr` does (`:memory:` or
    `mode=memory`). With SQLx's defaults the last connection closes after ten minutes idle and the
    data goes with it.
11. **SQLite's permanent code is "low byte 14"**, so `SQLITE_CANTOPEN`'s extended codes are permanent
    as well as `14`. The design lists `14`.
12. **`Cargo.toml` gains `futures-channel`** (workspace entry exists). No other dependency change.
    `mod classify` is compiled only when an engine feature is on.

## Points this area resolves

- **An attempt's bound against the pool's `acquire_timeout`:** divergence 2. One `.ready` attempt
  against an unreachable server lasts `acquire_timeout_secs` (5 s by default) plus the ping.
- **The hash for a MySQL lock name past 64 characters:** FNV-1a (64-bit) of the database name,
  written `'ulo-db.migrate.#'` plus 16 hex digits. `DefaultHasher` is not stable across Rust versions
  and every replica must compute one name. A collision makes two databases' migrations take turns
  and does nothing else. The 64 is counted in characters.
- **How the SQLite sidecar lock is taken off the worker:** divergence 9.
- **Secrets:** `Store::secrets` returns the password as written and percent-decoded, from the
  userinfo and from a `password=` query parameter, deduplicated. SQLite returns none.

## Requests

- **D1, `crates/ulo-db/src/connect.rs` and `migrate.rs`.** `lock`, `unlock` and `Migrations::run`
  acquire, query and drop `PoolConnection`s, and SQLx with `runtime-tokio` alone panics
  (`missing_rt`) on a thread with no tokio context. They have to run through `spawn_with` on the
  given runtime like `create`, `ping` and `close`, or on a context the runtime provides. `run_locked`
  as the skeleton gives it awaits them directly.
- **D1.** `unlock` must be awaited on every path out of `run_locked`, the failure of `Migrations::run`
  and the bound included; the guards also close their connections on drop, so a path that cannot
  await one leaks nothing.
- **S2a.** `check_scheme`, `validate_url` and `Engine` are public. The lock primitives are not
  (`PgLock`, `MySqlLock` and `SqliteLock` have private fields and no constructors); SeaORM's
  `MigrationLock` needs its own or a request back to D2 to expose `take_lock`-style helpers.

## Not verified

- `lock_timeout` on advisory locks (divergence 6).
- That `pragma_database_list` is available in the bundled SQLite (it is from 3.16, 2017).
- `GET_LOCK`'s result decodes as `Option<i64>` (read: `int_compatible` accepts every signed integer
  column type, and the function returns `INT`).
- That `Migrator::run`'s future is `Send` for `&Pool<DB>` (every bound read in the registry is, but
  nothing compiled).
- SQLx's `PgConnectOptions::from_str` calls `apply_pgpass`, which reads `~/.pgpass` and the
  environment during `validate`, and its parser logs `ignoring unrecognized connect parameter` at
  `warn` with the parameter's value (`sqlx-postgres` 0.9.0 `options/parse.rs`). Both are SQLx's;
  a misplaced secret in an unknown parameter would reach that log line. Not suppressible from here.
- Stage 2's R5 test (`runtime-smol` beside `runtime-tokio`) is the coordinator's.

## Replaced on `master`

`SqlxModule::postgres`/`postgres_named` and the other constructors returning `CheckedModule`, the
pool wrapper, the `health` feature's indicator, the crate's own URL redaction and SQLx 0.8 are not
carried over; the skeleton step had already removed them.
