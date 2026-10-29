# 35 db-sqlx

A database as a module. `SqlxModule::<Postgres>::from_config(&config, "database")` reads its options from the configuration, applies `migrations/` at `connect` under a lock, binds SQLx's own `PgPool`, and contributes a health check named `postgres@database`. The application injects `Dep<PgPool>` and calls SQLx.

```
docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=pw postgres:18
APP_DATABASE__URL=postgres://postgres:pw@127.0.0.1/postgres APP_DATABASE__MAX_CONNECTIONS=5 PORT=7000 cargo run -p ex-35-db-sqlx
curl -d '{"title":"first"}' -H 'content-type: application/json' http://127.0.0.1:7000/notes
```

`connect` waits for the database, up to 30 s, and fails with the last error when it is not reached. `.lazy()` (see `lazy_builder`) returns at once instead: readiness is 503, naming the store, until a ping gets through. A lazy store runs no migrations, since they need the database. `wire()` refuses a URL of another engine or a missing one before any connection is tried, and prints no part of it. `max_connections` must be at least 2 when migrations run: the lock holds one connection.

```
cargo test -p ex-35-db-sqlx                        # wiring refusals, the binary without a URL
cargo test -p ex-35-db-sqlx --features integration # starts Postgres containers; needs Docker
```

Read first:

- `src/lib.rs`: the module, the repository over `Dep<PgPool>`, eager and lazy builders.
- `tests/store.rs`: the migration and a note through the pool; a lazy store's readiness across an outage (a relay cut and restored).
- `tests/wiring.rs`: the refusals at `wire()`.
- `tests/binary.rs`: the binary without a URL stops with the report; with a database it serves and exits clean.

`ulo_db_sqlx::sqlx` is the SQLx the pool comes from; the example's own `sqlx` dependency only switches on `macros` for `sqlx::migrate!`. The example needs Rust 1.94, SQLx's floor.

`ulo new --db sqlx-postgres` writes a project with this store imported (`crates/ulo-cli/src/templates/new`).

Next: [36 db-two-instances](../36-db-two-instances).
