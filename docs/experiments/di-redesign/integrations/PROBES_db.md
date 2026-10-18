# Race 3 probes: database and store premises

Run 2026-10-18 on macOS, rustc/cargo 1.98.1. Every probe is a scratch crate outside the repo under
`/private/tmp/claude-501/-Users-ifeanyi-Projects-Personal-open-source-toni-rs-toni-rs/a632688a-d566-4ff7-b727-8f28e1dd90c6/scratchpad/race3-probes-db/`
(`p1-sqlx`, `p2-seaorm`, `p2b-twosqlx`, `p3-diesel`, `p4-stores`; raw outputs in `out_*.txt` beside them).
Library sources were unpacked from crates.io into `src/` there for the "Read" claims.

Marks: **Probed** = ran code and saw the output; **Read** = read the pinned source or docs; **Unverified** = neither.

Every error-text check used the password `hunter2` inside the URL and reports whether the error's `Display` (and `Debug` where noted) contains it.

## Versions used

| Crate | Version | rust-version | Date |
| --- | --- | --- | --- |
| sqlx (+ -core, -postgres, -mysql, -sqlite) | 0.9.0 | **1.94.0** | 2026-05-21 |
| sea-orm | 2.0.4 | **1.94.0** | 2026-09-27 |
| sea-orm-migration | 2.0.4 | 1.85.0 (declared; depends on sea-orm 2.0.4) | 2026-09-27 |
| diesel | 2.3.14 | 1.86.0 | 2026-10-07 |
| diesel-async | 0.9.2 | 1.84.0 | 2026-06-19 |
| diesel_migrations | 2.3.2 | 1.86.0 | |
| tokio-postgres | 0.7.18 | 1.85 | 2026-06-12 |
| deadpool | 0.13.1 | 1.85 | 2026-08-26 |
| bb8 | 0.9.1 | 1.75 | 2025-11-24 |
| mobc | 0.9.0 | unknown | 2025-06-25 |
| mongodb | 3.9.1 | 1.88 | 2026-09-10 |
| redis | 1.7.1 (matches `Cargo.lock:4448`) | 1.88 | 2026-09-25 |
| libsqlite3-sys | 0.37.0 resolved with sqlx 0.9.0 (latest published 0.38.2) | | |

`cargo info sqlx` printed `0.9.0-alpha.1 (latest 0.9.0)` and `cargo info sea-orm` printed `2.0.0-rc.38 (latest 2.0.4)`
because the local index entry was stale; the crates.io API and the unpacked `Cargo.toml`s above are the source of the table.

Docker: only my own containers (`race3-probe-pg` postgres:16-alpine, `race3-probe-mysql` mysql:8, `race3-probe-redis` redis:7,
`race3-probe-mongo` mongo:7), started with `docker run --name race3-probe-*` and removed by name with `docker rm -f`. Passwords were `hunter2`.

---

## 1. SQLx

### 1.1 Does 0.9 exist, its runtime features, its MSRV

**Probed / Read.** sqlx 0.9.0 is published (2026-05-21). Runtime features, from `sqlx-0.9.0/Cargo.toml`: `runtime-tokio`,
`runtime-smol`, `runtime-async-std`, `runtime-async-global-executor`. The MSRV is **1.94.0**, for `sqlx`, `sqlx-sqlite` and `sea-orm` 2.0.4 alike.

Verdict: holds, with a correction. The 1.88 floor that the workspace states for most crates does not hold for `ulo-db-sqlx` or `ulo-db-seaorm`.
The brief allows a crate to state its own floor; these two need 1.94.

For MySQL, a second finding: with `default-features = false` and no TLS feature, connecting to a stock `mysql:8` (caching_sha2_password) fails at connect with

```
Configuration("RSA auth backend disabled; enable feature `mysql-rsa` (or `rsa` if using sqlx-mysql directly) or use TLS.")
```

(`out_p1_mig.txt` run before adding the feature). The module must choose a TLS feature or `mysql-rsa`. It is a `Configuration` error, so it is permanent.

### 1.2 With both runtime features, how a pool picks a runtime

**Read** `sqlx-core-0.9.0/src/rt/mod.rs`:

```rust
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output> {
    #[cfg(feature = "_rt-tokio")]
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return JoinHandle::Tokio(handle.spawn(fut));
    }
    // then: async-global-executor, else smol::spawn, else async_std, else missing_rt
```

`rt_tokio::available()` is `Handle::try_current().is_ok()`; `timeout` and `sleep` use the same test. With only `_rt-tokio` and no
runtime, `missing_rt` panics with "this functionality requires a Tokio context".

**Probed** (`p1-sqlx/src/bin/spawn.rs`, `out_p1_spawn.txt`): a pool with `min_connections(1)` and a 300 ms `idle_timeout`, eager connect; the
`after_connect` callback prints the thread of the maintenance task that reconnects.

```
== 4 eager connect inside rt.block_on, min_connections=1
  [4] after_connect ... on thread Some("main")          (the caller's own connect)
  [4] after_connect ... on thread Some("tok-worker")    (spawned maintenance task)
== 5 eager connect in smol::block_on, no tokio ctx, min_connections=1
  [5] after_connect ... on thread Some("main")
  [5] after_connect ... on thread Some("smol-1")        (smol's global executor)
== 6 eager connect in smol::block_on inside rt.enter()
  [6] after_connect ... on thread Some("main")
  [6] after_connect ... on thread Some("tok-worker")
```

`p1-sqlx` main: `connect_lazy` called on a plain thread with **no runtime at all** returned `Ok` (both features on), meaning the pool silently used
smol's global executor. A pool built inside tokio and queried from a `smol::block_on` thread also worked (the socket layer picks tokio's net when a
tokio context exists, async-io otherwise; the query ran), so mixed use is not caught either.

Verdict: the claim "SQLx, with more than one runtime feature, picks tokio when `Handle::try_current()` finds a context" **holds**. Two consequences the design does not state:

1. The test is made at every `rt::spawn` call, not once. A `PoolConnection` dropped on a thread outside the runtime context spawns its return-to-pool task
   (`pool/connection.rs:202,208`) on smol's global executor, which creates threads the application did not ask for. An app with `runtime-smol` also
   enabled anywhere in its tree (feature unification) gets this behaviour whether or not the module wanted it.
2. Wrapping creation in `enter()` is sufficient for the maintenance task, but not for later spawns made from other threads.

Means for the design: `ulo-db-sqlx` should enable `runtime-tokio` only, never `runtime-smol`. Then a missing runtime panics rather than falling back to a
global executor, and the `Spawn::name() == "tokio"` refusal at the factory is the whole story. The conformance suite's "unsupported runtime refused" scenario should
include a test that no `smol-*` thread exists after a pool is created and dropped in a tokio-only app.

### 1.3 What `connect_lazy` does, and what pool creation spawns

**Probed** (`out_p1.txt`):

```
== A: connect_lazy inside tokio runtime, unreachable host (127.0.0.1:1)
  connect_lazy returned Ok? true in 1.585166ms
  size=0 idle=0
```

Nothing connects until the first `acquire`. **Read** `pool/inner.rs:69`: `PoolInner::new_arc` calls `spawn_maintenance_tasks` unconditionally; defaults are
`idle_timeout = 10 min`, `max_lifetime = 30 min` (`pool/options.rs:161-162`), so a periodic reaper task is spawned at creation, lazy or not.
With `min_connections > 0` a lazy pool still stays at size 0 after 800 ms (spawn probe cases 1-3: `size=0`); the maintenance task does not connect a lazy pool.
So creation spawns one reaper task (and a `min_connections` task when that is set), on the runtime chosen by 1.2.

Verdict: holds. `connect_lazy` and the creation-time spawn are consistent with the design's `.lazy()` and `spawn_with`.

### 1.4 `libsqlite3-sys` range and one SQLx in a build

**Read** `sqlx-sqlite-0.9.0/Cargo.toml`: `libsqlite3-sys = ">=0.30.1, <0.38.0"` (so the latest 0.38.2 is excluded; Cargo resolved 0.37.0).

**Probed** `p2-seaorm`: `sea-orm 2.0.4` + `sea-orm-migration 2.0.4` + a direct `sqlx 0.9` dependency produce one `sqlx` and one `sqlx-core` (0.9.0);
`cargo tree -d | grep -E '^(sqlx|libsqlite)'` prints nothing. SeaORM's `Cargo.toml` pins `sqlx = "0.9.0"` as an optional dependency.

**Probed** `p2b-twosqlx`: the design says "an application that holds a second SQLx of its own is refused at compile time: Cargo refuses two copies that link
the same native SQLite library if their ranges don't overlap". The ranges **do** overlap for 0.8.6:

```
sea-orm 2.0.4 (sqlx-sqlite)  +  sqlx 0.8.6 (sqlite)   -> cargo generate-lockfile OK, cargo check OK
cargo tree -d:  sqlx v0.8.6, sqlx v0.9.0, sqlx-core v0.8.6, sqlx-core v0.9.0, sqlx-sqlite v0.8.6, sqlx-sqlite v0.9.0
libsqlite3-sys resolved to 0.30.1 (sqlx 0.8.6 requires ^0.30.1; 0.9's range includes it)
```

and with Postgres only, two SQLx copies also resolve. Verdict: **does not hold**. Nothing refuses a second SQLx at compile time; the second copy's types simply
do not match `Db<_>` (a type error at the use site) and the app carries two SQLx builds, with SQLite forced down to libsqlite3-sys 0.30.1.
Means for the design: the guard against two SQLx copies is the CI `cargo tree -d` job plus, if wanted, a runtime check; the compile-time refusal sentence must go.
`ulo-db-sqlx` re-exporting `sqlx` is the right mitigation, and the module docs should tell users to use `ulo_db_sqlx::sqlx`.

### 1.5 Parse errors echoing the URL

**Probed** (`out_p1.txt`, section G; `PgConnectOptions::from_str`, `MySqlConnectOptions::from_str`, `SqliteConnectOptions::from_str`):

```
postgres://user:hunter2@host:notaport/db  -> "error with configuration: invalid port number" | Configuration(InvalidPort)
postgres://user:hunter2@/db?sslmode=bogus -> "error with configuration: empty host"
postgresql://user:hunter2@[::1/db         -> "error with configuration: invalid IPv6 address"
sqlite://user:hunter2@weird?mode=bogus    -> "error with configuration: unknown value \"bogus\" for `mode`"
contains hunter2: false in every case, Display and Debug of the error
```

The error text does not contain the URL or the password. Verdict: holds for SQLx's own parser.

Three further findings from the same probe:

- `PgConnectOptions::from_str("notascheme://u:hunter2@h/db")` returns `Ok`. SQLx ignores the scheme when you parse with a concrete driver's options type. `validate` must check the scheme itself.
- `PgConnectOptions` and `MySqlConnectOptions` **Debug output contains the password** (`p1-sqlx/src/bin/dbg.rs`: `contains hunter2: true` for both). Neither type may reach `Debug` or a log line unredacted.
- `SqliteConnectOptions` rejects unknown query parameters (`unknown query parameter sslmode`), which is a refusal at `wire` for free.

---

## 2. SeaORM

**Probed / Read.** Latest 2.x is **2.0.4**, `rust-version = 1.94.0`, depends on `sqlx = "0.9.0"` (Read, `Cargo.toml`). Its runtime features are
`runtime-tokio*` and `runtime-async-std*` only; there is no smol feature (Read). Its `ConnectOptions` has `connect_lazy(bool)` (Read, `database/mod.rs:553`; the
Postgres driver branches on it at `driver/sqlx_postgres.rs:142`), and it can wrap an existing pool: `DatabaseConnection::from_sqlx_postgres_pool(pool)` (Read).

### 2.1 Lazy connection

**Probed** (`p2-seaorm`, `out` below):

```
== lazy ConnectOptions to unreachable host
  Ok? true in 1.649041ms
  ping -> Err(ConnectionAcquire(Timeout)) in 2.002097625s
== non-lazy to unreachable host
  Some("Connection Error: pool timed out while waiting for an open connection") in 2.002481959s
```

Verdict: holds. `connect_lazy(true)` returns without connecting. This answers premise 5: the design's `.lazy()` for SeaORM is available.
Alternative, equally supported: build the sqlx lazy pool in `ulo-db-sqlx` and wrap it with `from_sqlx_postgres_pool`, so one pool policy covers both.

### 2.2 Parse errors echo the URL and the password

**Probed**:

```
Database::connect("postgres://user:hunter2@host:notaport/db")
  -> Connection Error: The connection string 'postgres://user:hunter2@host:notaport/db' cannot be parsed.   contains hunter2: true
Database::connect("notascheme://u:hunter2@h/db")
  -> Connection Error: The connection string 'notascheme://u:hunter2@h/db' has no supporting driver.        contains hunter2: true
Database::connect("u:hunter2@h")  -> same shape                                                              contains hunter2: true
```

(`sea-orm-2.0.4/src/database/mod.rs:185,212` format the full string into the error.) `ConnectOptions` Debug also contains the password (`contains hunter2: true`).
Verdict: **does not hold** for SeaORM. The design's rule "the refusal is written from the parse failure's category with userinfo replaced" is correct and necessary, but it
means `validate` must parse with SQLx's parser (which does not echo) or with the `url` crate, and must never pass a malformed URL to `Database::connect`
or surface its error unfiltered. Any error out of `Database::connect` or `ConnectOptions` must be scrubbed through the core's redaction before display.

### 2.3 Does `Migrator::up` lock

**Read:** a case-insensitive search of `sea-orm-migration-2.0.4/src` for `lock` and `advisory` finds nothing. **Probed**
(`p2-seaorm`, two `Mig::up` calls via `tokio::join!` on separate connections, one migration that inserts a row and runs `pg_sleep(2)`, `applied_log` and
`seaql_migrations` pre-created so only the migration body races):

```
run 1  A: Ok(())   B: Err("Query Error: ... duplicate key value violates unique constraint \"seaql_migrations_pkey\"")   total 2.06s
run 2  A: Err(same)  B: Ok(())                                                                                           total 2.10s
applied_log rows: 1    seaql_migrations rows: 1
```

Both runners executed the migration body concurrently (total 2 s, not 4 s: neither waited). On Postgres the migration runs in a transaction, so the loser's
insert into `seaql_migrations` hit the primary key and its transaction rolled back: the row count stayed 1. Without pre-creating `seaql_migrations`, the loser instead fails
on the table-creation race (`pg_type_typname_nsp_index`). So: no lock; on Postgres the second replica **fails its start** rather than applying twice; on an engine without
transactional DDL (MySQL) a body with side effects could run twice (Unverified, not run).

Verdict: holds (the design says no lock). The failure is more specific than "can apply the same pending list twice": one runner errors. Means for the design: the shared guard is
required for SeaORM, and with the guard the loser waits and then sees nothing pending.

---

## 3. diesel-async

### 3.1 Versions and features

**Read** (`diesel-async-0.9.2/Cargo.toml`): latest 0.9.2, MSRV 1.84.0, no default features. Features: `postgres`, `mysql`, `sqlite` (= `diesel/sqlite` + `sync-connection-wrapper`),
`sync-connection-wrapper`, `async-connection-wrapper`, `migrations`, pools `deadpool`, `bb8`, `mobc`, `r2d2`.

### 3.2 `SpawnBlocking` and whether a handle can be supplied

**Read** `src/sync_connection_wrapper/mod.rs`: the trait is

```rust
pub trait SpawnBlocking {
    fn spawn_blocking<'a, R>(&mut self, task: impl FnOnce() -> R + Send + 'static) -> BoxFuture<'a, Result<R, Box<dyn Error + Send + Sync>>> ...;
    fn get_runtime() -> Self;     // no arguments
}
```

`SyncConnectionWrapper<C, S = Tokio>` is generic over `S`. `AsyncConnection::establish` calls `S::get_runtime()`; the default `Tokio::get_runtime()` takes
`Handle::try_current()` and otherwise **builds a hidden current-thread runtime owned by the connection**. But `SyncConnectionWrapper::with_runtime(conn, runtime: S)` exists
(line 333), and `S` can be a user type. The built-in `Tokio` enum is not re-exported (only `SyncConnectionWrapper` and `SyncTransactionManagerWrapper` are public), so a handle is supplied
through a user-defined `S`.

**Probed** (`p3-diesel/src/bin/sqlite.rs`, `Given(Handle)` implementing `SpawnBlocking` with `get_runtime()` that panics; the closure reports its thread):

```
== 1 with_runtime(conn, Given(handle)) built outside any runtime context   -> closure ran on thread "given-rt"
== 2 AsyncConnection::establish driven from runtime `other`                -> closure ran on thread "other-rt"   (ambient lookup)
== 3 establish driven by a runtime-free executor (futures block_on)        -> closure ran on thread "tokio-rt-worker"  (hidden runtime created inside the wrapper)
```

Verdict: the design's sentence "the trait's constructor takes no arguments, so a handle can't be passed in" is **partly wrong**. `get_runtime()` takes none, but
`with_runtime` takes a value, so a handle can be supplied through a user-defined `SpawnBlocking` type and a pool manager's `ManagerConfig::custom_setup` closure (which replaces
`C::establish(url)`, Read `pooled_connection/mod.rs:98,108`). The default `establish` path is an ambient lookup with a hidden-runtime fallback, which the brief forbids. Means for the design:
for SQLite the module should use `custom_setup` + `with_runtime(conn, Given(handle))`; "the default spawns on the runtime the call is running on" holds only if every pool refill happens inside the given runtime, and deadpool's
`Runtime::Tokio1` setting makes its own pool-side tasks tokio as well, so the given-runtime check still has to come first.

### 3.3 `AsyncConnectionWrapper` blocking rule

**Read** `async_connection_wrapper.rs:113-121`: `From<C>` calls `B::get_runtime()`; there is no `with_runtime`, so a handle cannot be supplied (the `Tokio` blocker is private, ambient lookup, hidden current-thread
runtime fallback). **Probed** (`p3-diesel`): running `run_pending_migrations` on an `AsyncConnectionWrapper` directly inside a tokio task panics:

```
thread 'tokio-rt-worker' panicked at diesel-async-0.9.2/src/async_connection_wrapper.rs:359:24:
Cannot start a runtime from within a runtime. ...
```

and inside `tokio::task::spawn_blocking` it works (`Handle::current().block_on(establish)` then `.into()`). The design's "must not be called inside a tokio runtime, so the module runs it through the given handle's `spawn_blocking`" is right.
Note that `AsyncMigrationHarness` (Read, `migrations.rs`) uses `block_in_place`, which also panics on a current-thread runtime.

### 3.4 Pool crates

| Crate | Last release | Last commit (GitHub API) | Downloads (recent 90 d) |
| --- | --- | --- | --- |
| deadpool 0.13.1 | 2026-08-26 | 2026-09-04 | 41.5 M |
| bb8 0.9.1 | 2025-11-24 | 2026-10-13 | 9.3 M |
| mobc 0.9.0 | 2025-06-25 | 2025-06-25 | 0.2 M |

Verdict: deadpool is maintained, so the design's fallback to bb8 is not needed. mobc has had no change in 16 months. **Probed**: `deadpool::Pool::builder(manager).build()` returns in 4 microseconds without connecting
(`out_p3.txt`), and `pool.get()` against an unreachable host fails at once with `Error occurred while creating a new object: error connecting to server`. Diesel therefore has no lazy mode to offer: `build()` is lazy but the module's `.ready(ping)` is the only connect.

### 3.5 `diesel_migrations` lock

**Read:** no `lock`/`advisory` in `diesel_migrations-2.3.2/src` or `diesel-2.3.14/src`. Each migration runs in its own transaction by default (`run_in_transaction`). **Probed** (two `run_pending_migrations` calls in two
`spawn_blocking` tasks through `AsyncConnectionWrapper`, `__diesel_schema_migrations` pre-created, body = insert + `pg_sleep(2)`):

```
("A", Ok(1), 2.07s)   ("B", Err("duplicate key value violates unique constraint \"__diesel_schema_migrations_pkey\""), 2.07s)
applied_log rows = 1
```

Same shape as SeaORM: both ran the body concurrently, the loser errored on the primary key. Without the pre-created table the loser failed earlier on the table-creation race. Verdict: holds (no lock).

### 3.6 Errors echoing the URL, and what survives of the error code

**Probed** (`out_p3.txt`): diesel-async's own errors never contain `hunter2` (Display or Debug), for malformed URLs, wrong password, unknown database, refused and DNS failure. But diesel-async flattens the
database error:

```
wrong password   -> CouldntSetupConfiguration(DatabaseError(Unknown, "password authentication failed for user \"postgres\""))
unknown database -> CouldntSetupConfiguration(DatabaseError(Unknown, "database \"nosuchdb\" does not exist"))
refused / DNS    -> CouldntSetupConfiguration(DatabaseError(UnableToSendCommand, "error connecting to server"))   (same for both)
malformed URL    -> CouldntSetupConfiguration(DatabaseError(UnableToSendCommand, "invalid connection string"))
mysql refused    -> BadConnection("Input/output error: Connection refused (os error 61)")
mysql bad port   -> InvalidConnectionUrl("URL parse error: invalid port number")
```

The SQLSTATE is gone; wrong password and unknown database are both `DatabaseErrorKind::Unknown` with a message string. Raw `tokio_postgres` keeps it (`e.code()` gives `28P01` and `3D000`, and `source()` gives `io::ErrorKind::ConnectionRefused`). Means for the design: the Diesel module cannot classify by
variant. Either match on message text (fragile), or have the `custom_setup`/ping path connect with `tokio_postgres` (and `mysql_async`) directly for the probe so the code is available, or classify only on the string
patterns `password authentication failed`, `does not exist`, `Access denied`, `Unknown database`. Recommended: probe through the driver's own connect in `Store::ping`, and keep message matching as a documented fallback.

---

## 4. MongoDB

**Read / Probed.** Latest 3.x: **3.9.1**, MSRV 1.88. Runtime: **tokio only**. `runtime/join_handle.rs` wraps `tokio::task::JoinHandle`; spawning is `tokio::runtime::Handle::current()` (panics outside a runtime), or with the `sync` feature
`try_current()` falling back to the crate's own hidden runtime. There is no smol or async-std feature in 3.x.

**Probed** (`p4-stores`, `out_p4.txt`):

```
== MongoDB client creation: lazy? tasks spawned?
  with_options returned in 937.5µs; alive tasks before=0 after=2
  alive tasks after 200ms: 4
  unreachable ping after 1.5s: ServerSelection { message: "Server selection timeout: No available servers. Topology: ... Error: Kind: I/O error: Connection refused (os error 61) ..." }
```

`Client::with_options` does not connect and returns in under 1 ms; it spawns the topology monitor tasks on the current tokio runtime (2 at creation, 4 after 200 ms; the numbers are `RuntimeMetrics::num_alive_tasks`
deltas). The first operation pays the server-selection timeout (30 s default; the probe set 1.5 s).

URL parse errors (`ClientOptions::parse`, seven malformed URLs including bad port, empty host, bad scheme, bad `authMechanism`, `maxPoolSize=abc`, bad SRV host, unencoded `@`/`:` in the password): every `Display` and `Debug` is free of
`hunter2`. The `mongodb+srv` case does DNS inside `parse`, so `parse` is async and does I/O; `wire` cannot call it without doing network work. `ClientOptions` Debug is also free of the password. Verdict: holds for echo; note the SRV exception for a "parse only" `validate`.

Client creation without connecting: holds (premise "Mongo's client is lazy"). Spawn target: the current tokio runtime, so creation must run under `spawn_with`.

---

## 5. redis-rs

**Read** `redis-1.7.1`: `smol-comp` exists (plus `smol-native-tls-comp`, `smol-rustls-comp`). `aio/runtime.rs`: with only `tokio-comp` it is `Runtime::Tokio`; with only `smol-comp`, `Smol`. With both, `Runtime::locate()` returns
the process-global preference set by `prefer_smol()` / `prefer_tokio()` (a `OnceLock`, set once) if any, otherwise tokio when `Handle::try_current()` succeeds and smol otherwise. `Smol::spawn` is `smol::spawn(f)`, smol's global executor
(`aio/smol.rs:234`). The repo's workspace entry is `redis = { version = "1", features = ["aio", "tokio-comp"] }` (`Cargo.toml:141`); `ulo-db-redis` would add `connection-manager`.

Verdict for premise 4: holds, with the caveat that the choice for redis is a process-global `OnceLock`, not a per-call check like SQLx's. `prefer_tokio()` can be called once by the module before it builds anything; a second call from anywhere returns an error.

**Probed** (`p4-stores`; `smol-comp` and `tokio-comp` both enabled):

- `ConnectionManager::new` is **eager**: it connects and (default config) retries. Default config, refused host: `Io: Connection refused (os error 61)` after **9.46 s**; wrong password: `AuthenticationFailed` after **9.48 s** (`out_p4_rdef.txt`). So the library retries a permanent error for nearly ten seconds. The module must pass `ConnectionManagerConfig::set_number_of_retries(0)` and own the retry policy.
  With `set_number_of_retries(1).set_connection_timeout(Some(2 s))`: wrong password 197 ms; refused 151 ms; DNS 171 ms; blackhole 4.16 s (`timed out`).
- Creation spawns one task on the current runtime (`alive tasks before=11 after=12`; the baseline of 11 is left over from the MongoDB section in the same process). After all handles drop, the count returns to 11.
- In `smol::block_on` with no tokio context, with both features enabled: connect and `PING` work, spawned onto smol's global executor.
- Error text with `redis://:hunter2@...`: `Client::open` errors read `Redis URL did not parse - InvalidClientConfig` and `Invalid database number - InvalidClientConfig`; connect errors read `Password authentication failed - AuthenticationFailed`,
  `Connection refused (os error 61)`, `failed to lookup address information ...`, `timed out`. None contains `hunter2`. `Client` Debug prints `password: Some("<redacted>")`, so redis-rs redacts on its own; `ConnectionInfo` Debug likewise.

Means for the design: `ulo-db-redis` has no `.lazy()` (confirmed: the manager connects at creation), needs `number_of_retries(0)`, and can classify by `ErrorKind::AuthenticationFailed` (permanent) versus `Io` / `is_timeout()` (transient).

---

## 6. Pool close under an abandoned execution (SQLx, Postgres)

**Probed** (`p1-sqlx/src/bin/close.rs`, `out_p1_close.txt`):

```
== 1 checked-out PoolConnection held 3s; pool.close() awaited
  is_closed after 300ms: true  closer finished: false
  acquire during close: Some("attempted to acquire a connection on a closed pool")
  close() returned after 3.004793042s (holder released at ~3s)
  server-side backends with our app name: 0
== 2 query in flight (pg_sleep(3)) when close() called
  close() took 2.705454084s; query result (true, 3.007524583s)
== 3 holder never returns within a bound: close() under timeout(1s), then holder dropped at 2s
  close under 1s bound -> false after 1.002236916s; pool.is_closed()=true
  backends while holder still alive: 1
  backends 0.5s after holder dropped: 0
== 4 (pool handle dropped too) backends 1s later: 0
```

Verdict: **holds**: `Pool::close().await` waits for every checked-out connection to come back; it marks the pool closed at once (new `acquire` fails with `PoolClosed`); an in-flight query completes. If the close is cancelled by a timeout, the pool stays closed and
the held connection is closed by its drop when the holder finally releases it (backend count goes to 0), so a bound around `close` leaks nothing past the holder's life.

Means for the design: premise 7 resolves. The destroy hook's close bound contains the wait; on expiry the pool is already closed, a late-returning holder cleans up itself, and the log line should say which pool and how many connections were still out
(`pool.size() - pool.num_idle()` is available). Hung holders keep a server connection until they drop; only the server's idle timeout bounds that.

---

## 7. Migration locks

### 7.1 Measured, two concurrent runners on one server

| Migrator | Postgres result | Lock |
| --- | --- | --- |
| sqlx `Migrator::run` | **Probed**: A Ok 2.03 s, B Ok 2.04 s, `applied_log` rows = 1 (B waited for A, then found nothing pending) | yes: `pg_advisory_lock` / `GET_LOCK` (Read `sqlx-postgres/src/migrate.rs:186-215`, `sqlx-mysql/src/migrate.rs:154-180`); SQLite `lock()` is a no-op (Read `sqlx-sqlite/src/migrate.rs:149-153`) |
| sea-orm-migration `Migrator::up` | **Probed**: one Ok, one Err (primary-key conflict or `pg_type_typname_nsp_index`); both ran the body concurrently; rows = 1 | none (Read) |
| diesel_migrations via `AsyncConnectionWrapper` | **Probed**: one Ok, one Err (`__diesel_schema_migrations_pkey`); both ran the body concurrently; rows = 1 | none (Read) |

sqlx on MySQL, same test: both Ok at 2.07 s, rows = 1 (**Probed**, `out_p1_mig.txt`).

No migrator applied a migration twice on Postgres, because Postgres DDL and the version-table insert are transactional. Without a lock, though, the second replica's start **fails** with a database error, and the failure is indistinguishable from a real migration failure unless the module wraps it.
This matches the design's choice to provide the guard once; it strengthens the argument, since "not applied twice" is not what the guard buys on Postgres. The guard converts a failed start into a wait.

Hazard record: the RESPONSE's golang-migrate note (advisory lock plus `CREATE INDEX CONCURRENTLY`) was **not tested**; the suite should use ordinary DDL as the design says. **Unverified.**

### 7.2 Guard primitives per engine, from client libraries

**Probed** (sqlx, dedicated `PoolConnection`, `mig.rs`):

```
Postgres  select pg_advisory_lock($1)  on c1
  a second connection's pg_advisory_lock blocked (still blocked after 1 s); pg_try_advisory_lock from a third connection -> false
  after c1 pg_advisory_unlock, the waiter acquired after 1.05 s
  lock 7 taken on a connection that was then closed (close_on_drop): afterwards pg_try_advisory_lock(7) -> true   (session lock dies with the connection)
MySQL     select GET_LOCK(concat('ulo-migrate:', database()), 10)
  c1 -> 1;   c2 GET_LOCK(..., 1 s) -> 0 (timed out);   c1 RELEASE_LOCK -> 1;   c2 GET_LOCK -> 1
  a lock with a different name is independent; scoping to a database is by the name we build (server-wide namespace)
```

So `pg_advisory_lock` and `GET_LOCK` work from SQLx on a dedicated connection, and both are released if the connection dies. They are plain SQL, so they work from `tokio_postgres` / `mysql_async` / diesel-async connections as well (the SQL is client-independent; I ran them only through SQLx. **Read** for the others).
Postgres side: the lock id must fit an `i64`; SQLx derives it from the database name, and the guard should use its own fixed key distinct from SQLx's so the nesting the design describes stays harmless.

SQLite: **Probed** (`flock.rs`): `std::fs::File::lock` / `try_lock` on a sidecar file blocks a second handle (`try_lock -> WouldBlock`), and releases on drop. **Read** (doc.rust-lang.org): `File::lock` is stable since **1.89.0**, above the workspace's 1.88 floor, but `ulo-db-sqlx` is already at 1.94 because of SQLx, so `std` works there. A 1.88 crate would need `fd-lock` or `fs4`.
In-memory SQLite needs no guard.

---

## 8. Permanent versus transient errors

"Permanent" below means retrying cannot help. All rows **Probed** (`out_p1_errs.txt`, `out_p3.txt`, `out_p4.txt`) against my own containers.

### 8.1 SQLx

| Case | Error | How to tell |
| --- | --- | --- |
| PG wrong password | `Error::Database`, `code() = "28P01"`, 42 ms | permanent: SQLSTATE class `28` (invalid authorization) |
| PG unknown user | `Error::Database`, `28P01` (same code as wrong password) | permanent |
| PG unknown database | `Error::Database`, `code() = "3D000"` | permanent |
| PG refused (port 1), through a pool | `Error::PoolTimedOut` after the full `acquire_timeout` (3.00 s); the underlying `Io` is hidden | transient; but cannot be told from other causes |
| PG refused, single `PgConnection::connect` | `Error::Io(ConnectionRefused)` in 0.3 ms | transient |
| PG DNS failure, through a pool | `Error::Io(Uncategorized)`, "failed to lookup address information", 45 ms | transient (a typo in the host is permanent but not distinguishable by kind) |
| PG blackhole (10.255.255.1) | `Error::PoolTimedOut`, 3.00 s | transient |
| MySQL wrong password | `Error::Database`, `code() = "28000"`, message `1045 (28000): Access denied ... (using password: YES)` | permanent |
| MySQL unknown database | `Error::Database`, `code() = "42000"`, `1049 (42000): Unknown database` | permanent |
| MySQL refused through a pool | `PoolTimedOut`, 3.00 s | transient |
| SQLite unwritable path | `Error::Database`, `code() = "14"` (`SQLITE_CANTOPEN`) | permanent |
| MySQL without TLS or `mysql-rsa` | `Error::Configuration` | permanent (misconfiguration) |
| URL parse failures | `Error::Configuration` | permanent, caught at `wire` |

Rule for SQLx: `Error::Configuration` and `Error::Database` with SQLSTATE class `28` or `3D000` (Postgres), `28000` or `42000` (MySQL) are permanent. `Io`, `PoolTimedOut`, `WorkerCrashed`, `Tls` are transient. Display never contained `hunter2` in any case above.
**A pool collapses connection-level I/O failures into `PoolTimedOut`** (the pool retries inside `acquire_timeout`), so the module's connect attempt should use a single `connect` (or a pool with a short `acquire_timeout` and a `min_connections(0)` ping) to see the `Io` kind.
The connect bound the design sets (30 s total) must be larger than `acquire_timeout` per attempt, or one attempt eats the bound.

### 8.2 diesel-async (tokio-postgres, mysql_async)

See 3.6. Through diesel-async: wrong password and unknown database both come back as `DatabaseError(Unknown, <message>)`; refused and DNS are identical (`DatabaseError(UnableToSendCommand, "error connecting to server")`). Through raw `tokio_postgres::connect`
(**Probed**): wrong password `code() = 28P01`, unknown database `code() = 3D000`, refused has no code and `io::ErrorKind::ConnectionRefused` as `source()`. Display is `db error` only; the message is in `as_db_error()`. MySQL through diesel-async: `Access denied` and
`Unknown database` arrive as `DatabaseError(Unknown, ...)`, refused as `BadConnection("Input/output error: ...")`.

### 8.3 MongoDB (3.9.1)

| Case | Error |
| --- | --- |
| wrong password (authSource admin) | `ErrorKind::Authentication { message: "SCRAM failure: Authentication failed." }`, `source` has `CommandError { code: 18, code_name: "AuthenticationFailed" }`, 267 ms. Permanent. |
| unreachable | `ErrorKind::ServerSelection { message: "... Error: Kind: I/O error: Connection refused ..." }` after `serverSelectionTimeoutMS`. Transient. |
| unknown database | **No error exists.** MongoDB creates databases on first write; a ping against `nosuchdb` with `authSource=admin` succeeds. With a database in the URL path and no `authSource`, that database becomes the auth source and the failure is `Authentication` (so it looks like wrong credentials). |
| malformed URL | `ErrorKind::InvalidArgument` (permanent); a `mongodb+srv` failure is `DnsResolve` and happens inside `parse`. |

None of the texts contains `hunter2`.

### 8.4 redis-rs

`e.kind()` gives `AuthenticationFailed` (wrong password, permanent), `Io` (refused: `is_connection_refusal() = true`; DNS: `is_io_error()` only; blackhole: `is_timeout() = true`), `InvalidClientConfig` (bad URL, permanent, from `Client::open`, so a `wire`-time check that does no I/O).
There is no unknown-database error at connect: `/notadb` fails URL parsing (`Invalid database number`); a numeric database beyond the server's count (default 16) is an error on `SELECT`, which was not probed (**Unverified**).
The default config retries ~9.5 s on every one of these (see 5), including `AuthenticationFailed`.

---

## Surprises

1. SQLx 0.9 and SeaORM 2.0.4 both need Rust 1.94. The 1.88 floor does not reach the database layer for SQLx or SeaORM.
2. Nothing refuses a second SQLx at compile time: sqlx 0.8.6 and 0.9.0 coexist, with SQLite pinned to libsqlite3-sys 0.30.1.
3. `Database::connect` (SeaORM) puts the full URL, password included, into its parse and no-driver errors; `PgConnectOptions` / `MySqlConnectOptions` / `ConnectOptions` Debug also print the password. SQLx's parser, mongodb and redis-rs do not.
4. redis-rs `ConnectionManager::new` retries about 9.5 s by default, wrong password included.
5. A SQLx pool created with no runtime at all succeeds when `runtime-smol` is on, starting smol's global executor silently. The runtime choice is made at each spawn, not at creation.
6. diesel-async flattens SQLSTATE away, so classification by variant is impossible through it.
7. A SQLx pool turns "connection refused" into `PoolTimedOut`.
8. diesel-async's wrapper can take a given handle (`with_runtime` + user `SpawnBlocking` type, or `custom_setup`); the design said it cannot.

## Summary table

| Premise | Verdict | Recommended choice | Confidence |
| --- | --- | --- | --- |
| SQLx 0.9 exists; runtime features `runtime-tokio` / `runtime-smol` | Holds (0.9.0, 2026-05-21; also async-std, async-global-executor) | Pin `=0.9.x` in one workspace entry; enable `runtime-tokio` only | High (Probed) |
| SQLx MSRV fits 1.88 | Does not hold: 1.94 (also sqlx-sqlite, SeaORM 2.0.4) | `ulo-db-sqlx` and `ulo-db-seaorm` state a 1.94 floor | High (Read) |
| Both runtime features pick tokio when a tokio context exists | Holds, per `spawn` call; else smol's global executor, silently, even with no runtime | Never enable `runtime-smol`; refuse non-tokio by `Spawn::name()` before building | High (Probed + Read) |
| `connect_lazy` connects nothing; creation spawns on the right runtime | Holds (1.6 ms to an unreachable host; one reaper task per pool at creation) | Build inside `spawn_with`, lazy = `connect_lazy_with` | High (Probed) |
| Second SQLx refused at compile time | Does not hold: sqlx 0.8.6 and 0.9.0 resolve and build together | CI `cargo tree -d` check; re-export `sqlx`; drop the compile-time claim | High (Probed) |
| SeaORM 2.x on one SQLx 0.9 | Holds (one sqlx in `cargo tree`) | Share the workspace entry; optionally wrap the module's pool with `from_sqlx_*_pool` | High (Probed) |
| SeaORM can create a connection without connecting | Holds (`ConnectOptions::connect_lazy(true)`, 1.6 ms) | Use it, or wrap the shared SQLx lazy pool | High (Probed) |
| Parse errors do not echo the URL | Holds for SQLx, diesel-async, mongodb, redis-rs; **does not hold for SeaORM** (full URL in error and Debug); SQLx `*ConnectOptions` Debug prints the password | Validate with SQLx's parser or `url`; never pass a bad URL to `Database::connect`; scrub every library error; never Debug an options value | High (Probed) |
| `Migrator::up` takes no lock | Holds; the loser errors (PK conflict), the body runs twice concurrently | Shared guard | High (Probed) |
| `diesel_migrations` has no lock | Holds; same loser-errors shape | Shared guard | High (Probed + Read) |
| sqlx `migrate` locks | Holds on Postgres and MySQL (B waited 2 s, rows = 1); no-op on SQLite | Keep the shared guard outside it | High (Probed) |
| Guard primitives work from client libraries | Holds: `pg_advisory_lock`, MySQL `GET_LOCK` (name built from `database()`), `File::lock` on a sidecar (stable 1.89) | Dedicated connection per guard; own fixed lock key; sidecar file for SQLite | High for SQLx and std (Probed); Read for the other clients |
| `SpawnBlocking::get_runtime()` takes no handle, so none can be given | Partly: the trait constructor takes none, but `with_runtime(conn, S)` and pool `custom_setup` let a user type carry a handle | SQLite via `custom_setup` + a `Given(Handle)` type; refuse on `Spawn::name()` first | High (Probed) |
| `AsyncConnectionWrapper` must not run inside a runtime | Holds (panics on a tokio worker); no way to supply a handle | Run migrations in the given handle's `spawn_blocking` | High (Probed) |
| deadpool maintained; bb8 fallback | Holds: deadpool released 2026-08-26, commit 2026-09-04; bb8 also active (commit 2026-10-13); mobc stale | deadpool; bb8 not needed | High (Probed) |
| Diesel is non-lazy | Holds in effect: `Pool::build` is lazy (4 microseconds) but there is no connect until `get()` | No `.lazy()` for Diesel as planned | High (Probed) |
| mongodb: tokio only; client creation lazy | Holds (3.9.1, `Handle::current()`; 2 tasks at creation, no connect in 1 ms); `parse` of `+srv` does DNS | Tokio check at the factory; `validate` avoids SRV resolution or runs it at `connect` | High (Probed + Read) |
| redis-rs `smol-comp` spawns on smol's global executor | Holds (`smol::spawn`); with both comps the choice is a process-global `OnceLock` or `try_current()` | Keep `tokio-comp` only; call `prefer_tokio()` if another crate enables smol | High (Read + Probed) |
| `ConnectionManager` creation is eager | Holds; default retries make a wrong password take ~9.5 s | `set_number_of_retries(0)`, own the backoff | High (Probed) |
| `Pool::close` waits for a held connection | Holds (3.00 s with a holder released at 3 s); in-flight query completes; a bounded close leaves the pool closed and the late holder closes its own connection | Bound the close; log leaked count; no extra mechanism | High (Probed) |
| Permanent vs transient classification | SQLx: SQLSTATE `28P01`, `3D000`, `28000`, `42000`, `Configuration` are permanent; pool turns refused into `PoolTimedOut`. diesel-async loses the code. Mongo: `Authentication` permanent; no "unknown database" error. redis: `AuthenticationFailed` permanent | Classify in each `Store::ping`/`create`; connect once without a pool retry loop; use the driver's own connect for Diesel | High (Probed) |

## Container and image state

Containers at start (all pre-existing, none touched): `backend-next-postgres-1`, `backend-next-s3-1`, `backend-next-redis-1`, `backend-next-mailpit-1`, `video-streaming-{caddy,worker,api,postgres,redis,minio,web}-1`, `sleepy_swanson`; all `Exited`.
Containers at end: the same twelve, all still `Exited` with the same status; my four `race3-probe-*` containers removed by name. No `prune` was run.
Images: 25 at start; 26 at end. The only change is `mongo:7`, pulled for the MongoDB probe (the machine had only `mongo:5.0.6`). It was left in place; it can be removed with `docker rmi mongo:7`.
