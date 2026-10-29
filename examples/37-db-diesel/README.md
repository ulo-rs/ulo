# 37 db-diesel

A synchronous driver beside an async runtime. Diesel's SQLite connection blocks its thread for as long as a query runs, so `DieselModule::<Sqlite>` runs each query on the blocking pool of the runtime the app was given. The application injects `Dep<SqlitePool>` and calls diesel-async's `RunQueryDsl`.

```
cargo test -p ex-37-db-diesel
PORT=7000 cargo run -p ex-37-db-diesel
curl http://127.0.0.1:7000/count/1000000
```

`/count/{n}` counts the rows of a recursive SQLite query, which takes longer as `n` grows. `APP_DATABASE__URL` names a file; the default is `:memory:`. The Postgres and MySQL stores of the same crate are asynchronous end to end and need no blocking pool; the SQLite one is the case where the pool matters. It links the system `libsqlite3`.

Read first:

- `src/lib.rs`: the module and the repository over the pool.
- `tests/off_the_worker.rs`: the app and a ticker share one thread; during a query of a few hundred milliseconds the ticker's longest gap stays under a quarter of the query's duration. `EX_REPORT_TICKS=1` prints the measurement.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port and ends it with SIGTERM.

The example needs Rust 1.89, `ulo-db-diesel`'s floor.

`ulo new --db diesel-sqlite` writes a project with this store imported (`crates/ulo-cli/src/templates/new`).

Next: [38 tracing](../38-tracing).
