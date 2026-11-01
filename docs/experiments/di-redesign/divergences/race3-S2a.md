# Divergences: S2a, `ulo-db-seaorm`

S2a wrote `crates/ulo-db-seaorm/src/{lib,options,store,migrate}.rs` and left `Cargo.toml` as the
spine wrote it (floor 1.94 already stated). No cargo was run. Every sea-orm and sea-orm-migration
2.0.4 and sqlx 0.9.0 call below was read in `~/.cargo/registry/src`. All entries await sign-off.

## Requests to other owners

None. The plan's open question (do the lock primitives reuse D2's, which would need them public)
resolves without a request: `ulo_db_sqlx::{Postgres, MySql, Sqlite}` and their `MigrationLock`
impls are already public, and each takes the plain SQLx pool, which SeaORM exposes through
`DatabaseConnection::get_{postgres,mysql,sqlite}_connection_pool`.

## Points the area resolves

S2a-1. **Lock primitives are D2's, called over SeaORM's pool; none are written again.**
`MigrationLock for SeaOrm` matches `get_database_backend()` and calls
`<Postgres as MigrationLock>::lock(pool, bound)` (likewise MySQL, SQLite); `unlock` forwards the held
guard. The private field `SeaOrmLock::held` changed from `Box<dyn Any + Send>` to a private enum
`Held` of D2's guards, so `unlock` needs no downcast. Cost: S2a's lock behaves as D2's body does,
and the two share the pool-type identity (one SQLx in the build, PROBES_db.md §1.4). On Postgres and
MySQL the lock holds one pooled connection for the whole run, so `max_connections = 1` cannot
migrate; documented on `SeaOrmMigrations` and `SeaOrmOptions`, not refused (migrations are unknown to
`validate`).

S2a-2. **Engine detection uses SeaORM's own routing schemes, not `Engine::schemes`.**
`Database::connect` routes on `postgres`/`postgresql`, `mysql`, `sqlite` (`DbBackend::is_prefix_of`,
`sea-orm-2.0.4/src/database/db_connection.rs`). `validate` maps those to an `Engine`, then calls D2's
`validate_url` (shared scheme check and SQLx parser). If D2's `Engine::schemes` lacks `postgresql`
or reads a scheme case-sensitively, such a URL is refused here though SeaORM would accept it; the
refusal is conservative. If D2's list holds a scheme SeaORM lacks (for example `mariadb`), it is
still refused here by the first mapping. The scheme is lowercased for the mapping only.

S2a-3. **An engine whose feature is off is a `validate` problem.** SeaORM reports it as "no
supporting driver" with the URL in the message. Without any engine feature the crate accepts no URL.

S2a-4. **Pool-size checks in `validate`:** `max_connections` zero, `min_connections` above
`max_connections`, and `acquire_timeout_secs` zero are problems. The design is silent; a zero
maximum or timeout makes a pool that cannot hand out a connection.

S2a-5. **`secrets` registers several texts for the password.** The password as written in the
userinfo, its percent-decoded form, and, for a `password=` query parameter (which SQLx's Postgres
parser reads), the value as written, percent-decoded, and decoded with `+` as space. Each is a
`Secret<String>`; empties and duplicates are dropped. The userinfo is parsed by hand (no `url` crate
in the manifest, and `sqlx` does not re-export `Url`).

S2a-6. **`create` scrubs `Database::connect`'s error itself, as `Permanent`.** A lazy connect does
no I/O, so a failure is a URL it cannot route or parse, which `validate` should have caught.
The `DbErr` text is passed through `scrub` (URL, then each password text, replaced by `[redacted]`)
and wrapped in `ConnectRefused` inside `Permanent`. The registered secrets would scrub it again in
the core; this makes the crate safe on its own.

S2a-7. **Error classification is written again, not called.** D2's `permanent_or_not` is
`pub(crate)`, and takes an `sqlx::Error` while SeaORM hands a `DbErr`. `classify` reads
`DbErr::{Conn,Exec,Query}(RuntimeErr::SqlxError(e))` and applies D2's table: `Configuration`
permanent; `Database` by `code()` per `DbBackend` (Postgres class `28` or `3D000`, MySQL `28000` or
`42000`). SQLite: the code's primary byte equals 14 (`code & 0xff == 14`), which covers the extended
`CANTOPEN` codes; D2's text says "class 14" and may match only `"14"`. `ConnectionAcquire` (pool
timeout or closed), `Io`, `Tls` and the rest stay bare. The classification is `cfg`-gated on an
engine feature because `RuntimeErr::SqlxError` exists only with `sqlx-dep`; with none, nothing is
permanent.

S2a-8. **`ping` is SeaORM's `DatabaseConnection::ping`** (acquire from the pool, then SQLx's protocol
ping), not `select 1`. A refused connection arrives as `ConnectionAcquire(Timeout)` after the
pool's `acquire_timeout` (retryable); bad credentials arrive at once as a `Database` error.
`close` calls `DatabaseConnection::close` and logs a failure at `debug` (it fails only on
`Disconnected`).

S2a-9. **No `sqlx_logging` change.** SeaORM's default logs statements at `info` through `log`; no
URL is in those lines.

## Not verified (nothing compiles here)

- Match arms of different `Box::pin(async ..)` types coercing to `BoxFuture` in `MigrationLock::lock`
  rely on the function's return type as the expectation; a compile may want explicit casts.
- That `sea_orm::sqlx` and `ulo_db_sqlx::sqlx` are one crate (the `get_*_connection_pool` return
  types are passed to D2's `lock`); this holds only if the build resolves one SQLx (PROBES_db.md §1.4).
- D2's bodies (`validate_url`, the lock primitives) were not read; only the skeleton signatures.
- `sea-orm-migration` with `sea-orm` default features off: it declares its own `sea-orm` dependency
  with `schema-sync`, and the macros come from default-feature unification; read, not built.
