# Questions after the probes (for the designer)

Written for: the designer. The probe batch (`PROBES_config.md`, `PROBES_db.md`, `PROBES_obs.md`) and the consolidated `DESIGN.md` (42 decisions, §14 lists what each area resolves in its log) are in the repository. Most premises held. The items below are the ones the probes overturned or the design leaves open and a signature depends on. The spine freezes signatures, so these come before it.

Paste alongside: `DESIGN.md` §1, §2, §3.4, §4.1, §4.5, §4.6, §6, §14, and `PROBES_*.md` tables.

## What the probes overturned

**P1. SQLx 0.9, sqlx-sqlite and SeaORM 2.0.4 need Rust 1.94.** The workspace floor is 1.88. `ulo-db-sqlx` and `ulo-db-seaorm` therefore state a 1.94 floor, as `ulo-graphql-async-graphql` states 1.89 and `ulo-http-salvo` 1.92. Confirm, and say whether `ulo-db` (shared policy, runtime-free) stays at 1.88 with the SQLite sidecar lock (`File::lock`, 1.89) kept out of it.

**P2. A second SQLx in an application is not refused at compile time.** SQLx 0.8.6 and 0.9.0 resolve and build together, with SQLite pinned to libsqlite3-sys 0.30.1. The design now says a second SQLx fails at `wire()` as a missing binding, with a CI `cargo tree -d` job as the guard. Confirm.

**P3. Passwords leak beyond parse errors.** SeaORM's `Database::connect` puts the full URL with the password in its errors. `Debug` of SQLx's `PgConnectOptions` and `MySqlConnectOptions` and of SeaORM's `ConnectOptions` prints the password. mongodb and redis-rs do not. SQLx's parser accepts `notascheme://`. Rule proposed: `validate` parses with SQLx's parser or the `url` crate and checks the scheme itself, no library error reaches a user unscrubbed, and `Debug` of an options value is never logged. Which parser for the SeaORM store?

**P4. JSON and YAML.** `jsonc-parser` 0.34.0 is maintained and gives a range per value, so JSON files report line and column like TOML. For YAML, `serde-saphyr` needs Rust 1.89 and `serde_norway` has 64 `unsafe` hits; the probe recommends `saphyr`'s `MarkedYaml` into the same origin tree (MSRV 1.85, no `unsafe` in src), with anchors, merge keys and YAML 1.1 scalars unprobed. `serde_yml` carries RUSTSEC-2025-0068 and is never used. Which YAML route?

**P5. `.env`.** `dotenvy`'s iterator expands `$VAR` from earlier keys and from the process environment with no switch, and gives no line numbers. The crate writes its own subset parser (about 40 lines). Confirm.

**P6. `Secret` under `#[serde(flatten)]` or `#[serde(untagged)]` goes unregistered with no error**, and a problem message can print a secret's found value. Proposed: both attributes documented as unsupported around a `Secret`, and a problem under a secret path rendered from its path and kind alone, never the value. Or a collector written by `Secret::deserialize` itself. Which?

**P7. "All errors together" covers type, range and missing-field problems only.** An error raised inside a leaf type's own `Deserialize` (`IpAddr`, `Url`) has no placeholder and stops the pass at that point. Accept as a stated limit?

**P8. `ulo-otel-otlp` can be tokio-free.** With a supplied HTTP client over `ureq` (about 20 lines), `cargo tree -i tokio` is empty and the span still arrives; only the default reqwest blocking client brings tokio. Ship ureq as default and `reqwest-blocking` as an opt-in feature (level per feature in `runtime-levels.txt`), or keep reqwest and `tokio-bound`?

**P9. gRPC health.** tonic-health ends a `Watch` on an unregistered service with `NOT_FOUND` instead of streaming `SERVICE_UNKNOWN`, and sends a duplicate `Watch` message on every write of an unchanged value. The bridge writes only on change. Document the deviation, or answer with a `Health` service of the bridge's own over the same reporter?

**P10. Kubernetes.** Endpoints are marked terminating at the pod's deletion timestamp, before `preStop` and SIGTERM. `.lag` equals the `preStop` sleep only if no `preStop` exists. Reword the guidance; the default stays zero.

## Open design holes (DESIGN §14)

**H1. `Store::create` "without connecting".** Redis's `ConnectionManager::new` connects, so a construction failure sits outside `.ready`'s retries, and with default settings a wrong password takes 9.5 s because it retries. The probe recommends `set_number_of_retries(0)` and owning the backoff. Does Redis fit `create` as "builds the manager, which connects once with no retry", with `.ready` doing the retrying? And why does Diesel offer no `.lazy()` when deadpool's `build` connects nothing?

**H2. Diesel, SQLite and a handle.** After the third response no store holds a handle; the given runtime is a `dyn Runtime`. diesel-async's `SyncConnectionWrapper` takes a `SpawnBlocking` type through `custom_setup`, and `AsyncConnectionWrapper` panics on a tokio worker. Where does Diesel's migration run off the worker, and what `SpawnBlocking` does the SQLite wrapper take? Does `Runtime` gain a blocking-work method now (transports decision 133 said it gains one when a runtime-free or runtime-neutral crate needs it), or does `ulo-db-diesel`, a tokio-bound crate, own a blocking path of its own?

**H3. The lock.** `ulo-db` is runtime-free and names no driver, yet "writes the lock once". Proposed split: `ulo-db` owns the protocol (key, order, bound, error), each store supplies the dedicated connection and the primitive (`pg_advisory_lock`, `GET_LOCK` scoped by database name, a sidecar file). Confirm, and the Postgres key.

**H4. Trace pass-through.**
- An invalid incoming `traceparent` in pass-through (no exporter): no context, as a standalone execution has, or a new trace, which only a bound `TraceSource` can give IDs to?
- Merging a caller's `tracestate` in pass-through conflicts with W3C §3.4: a `traceparent` forwarded unchanged must carry its `tracestate` unchanged. Merge only when a context is originated or modified here?
- W3C Level 1 or Level 2 (Level 2 adds the `random` flag 0x02, which Level 1 says to zero)?
- A correction: the `tracestate` mutation rules are §3.5, not §3.3.

**H5. The `TraceSource` hook.** A tracing layer cannot parent a span from a recorded field (`OtelData` is `pub(crate)`). The parent has to be set by code holding the `Span` between creation and first enter, which `span::call` and `.instrument` already allow. So `TraceSource` is a core hook called there, and `ulo-otel` implements it with `OpenTelemetrySpanExt::set_parent`. Confirm; `Execution::open` consumes `ExecOptions`, so each transport extracts the trace context before opening the execution.

**H6. Smaller.** The Mongo crate is `ulo-db-mongo` in the responses and `crates/ulo-db-mongodb` on disk: rename the directory? Which TLS feature or `mysql-rsa` the MySQL driver enables (SQLx 0.9 against MySQL 8 fails at connect with neither)? `mongodb+srv` resolves DNS inside `parse`, so `validate` skips it. Does anything catch an application whose tree enables SQLx's `runtime-smol` or redis-rs's `smol-comp`; does `ulo-db-redis` call `prefer_tokio()`?

Answers that the area logs can settle by themselves (names of constructors, defaults, module paths) are not asked; §14 lists them.
