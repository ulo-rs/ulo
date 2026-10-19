# Questions after race 3 (for the designer)

Written for: the designer. Race 3 filled the integration layer (config, health, the five stores, the store suite, the CLI templates, trace propagation, OpenTelemetry) and compiled it. The workspace test run is 1319 passed, 1 failed (a timing margin under load, not a defect), 154 ignored. The store suite runs against real postgres, mysql, mongo and redis, and every scenario that could be checked went red under a planted break and green after the restore. Each item below fixes a public signature, changes a ruling's claim, or needs a choice the design does not make. Each carries a recommendation. Divergence logs are `divergences/race3-*.md`.

Paste alongside: `DESIGN.md` §2.3, §3.4, §4.2, §4.6, §6, §13, §14; `divergences/race3-C.md`, `race3-D1.md`, `race3-D3.md`, `race3-S2b.md`, `race3-O2.md`.

## Rulings the build showed to be wrong or incomplete

**Q1. `Defaults` carries no values.** The code in `ulo-config` reads a `Default` value into the tree once a type is in hand, but the signature the spine froze names no type, so a section cannot supply its defaults.
- Recommendation: `Defaults` takes the section's type and reads `Default::default()` through serde, the same path a file value takes, so a default carries the origin "default".

**Q2. `ulo-db-seaorm` is not lazy on SQLite.** `sea-orm-migration` enables sea-orm's default `sqlite-use-returning-for-3_35`, which makes its SQLite connector run `SELECT sqlite_version()` inside `connect_lazy`. `create` therefore connects, and a missing file is refused in about 2 ms. Postgres and MySQL are lazy. The scenario `lazy_readiness_waits_for_the_first_ping` is ignored for SeaORM SQLite, marked KNOWN DEFECT.
- Options: build the SQLite pool lazily in the store and wrap it with `SqlxSqliteConnector::from_sqlx_sqlite_pool` (re-implements SeaORM's pool options), or accept SQLite as an exception.
- Recommendation: accept and document. A file has no startup ordering to be lazy about.

**Q3. A replica whose wait for another replica's migration lock reaches the bound fails as `Permanent`.** The skeleton's docs on `run_locked` and `MigrationFailed` say so, yet a restart after the other replica finishes would succeed (`race3-D1.md`, design question Q1).
- Recommendation: make the lock-wait timeout retryable (no `Permanent`), bounded by `connect_within` like every other connect failure. A slow migration then costs a retry, not a failed start.

**Q4. Two keyed instances built with `StoreModule::from_options` share a check name.** The check name reads the section path, which is empty for `from_options`, so two keyed stores are two checks named `S::NAME`. The suite's keyed scenario counts them and passes; a health body listing checks by name shows the name twice.
- Options: the optional `ModuleDef::qualifier(&self) -> Option<&'static str>` request in `race3-S2b.md` R1, or refuse a second keyed `from_options` store without a section path.
- Recommendation: add `qualifier`; it is a read, not a new concept.

**Q5. Mongo has no `.lazy()`.** The store's `create` runs `Client::with_options` inside the factory's task and does not defer anything, and `CREATE_CONNECTS` is false (`race3-S2c.md`). `lazy_readiness_waits_for_the_first_ping` does not apply to it and is ignored in the suite.
- Recommendation: document that Mongo readiness waits on the first ping and drop `.lazy()` from the design's list for it.

## Observations that do not hold a ruling but cost a decision

**Q6. gRPC `outgoing` replaces a caller's `traceparent` and, for a `Received` context, a caller's `tracestate`.** §6.3 states the reservation (a refusal) for the RPC client alone. `ulo_grpc::outgoing` already writes `grpc-timeout` over a caller's value, and S-core froze `trace::inject` as returning nothing, so a refusal needs a changed helper and a new error from `outgoing`. For an `Originated` context a caller's `tracestate` entries are merged by `TraceState::prepend`, as the RPC client does (`race3-O1.md`, O1-D1).
- Options: leave as built, or make `outgoing` refuse both as the RPC client does.
- Recommendation: refuse, for parity. A transport that silently differs on a rule the design states once is the kind of gap the conformance suites exist to remove.

**Q7. Environment read outside `Env`.** `ulo-otel-otlp` and the SDK defaults it forwards to read `OTEL_*` variables from the process environment directly, bypassing the `Env` the application passed to `main`.
- Recommendation: accept for the OpenTelemetry-defined variables only, state it at the module, and do not extend it to any other.

**Q8. Redis S3 does not hold.** `prefer_tokio()` exists only with the `smol-comp` feature, and it is a `OnceLock`, so another crate's earlier preference stands. S3 asked for a `warn` or a refusal; the store has no call to make when the feature is off.
- Recommendation: `warn` when the feature is on and the call returns its error; nothing otherwise.

**Q9. `OtelModule` with no `Timer` is refused at `wire()` with `BoundWithoutTimer`.** The flush hook needs a written bound, and a written bound needs a `Timer` (`race3-O2.md`). Leaving the hook at `Bound::Default` would avoid the refusal but its 10 s `hook_timeout` would cut a flush timeout set above it.
- Confirm the refusal, or have `OtelModule` bring its own timer.

**Q10. Gap, not a question: TLS to MySQL and Mongo is not exercised.** The suite runs without TLS against containers. Add a TLS container recipe to `support/server.rs`, or mark the premise unexercised in the stores' docs?

**Q11. The R5 test is expected to fail.** The smol-drop premise (H6) says a `PoolConnection` dropped off the runtime thread is harmless. The probe says it spawns on smol's global executor. The test is written to decide it.
- If it fails as expected, the ruling is revised: which of "refuse the combination" or "document it" does the design want?

## Returns from batches 43 to 46

**Q12. Client zero `roots_timeout`.** A client with `roots_timeout` of zero never loads roots. Refuse at construction or treat as the default?

**Q13. `Bound::Default` as the link's own 30 s.** Confirm the link's own default, not the core's.

**Q14. The remaining shutdown window** needs `AppHandle::close` to change signature (it cannot report the drain it started).

**Q15. F428's widening** (one hub's claim count against the rest). Fix beside Q14 or leave filed.

## For the record

- The Mongo stall scenario runs against a container without `--auth`; the SCRAM key derivation takes 55 to 80 ms in a debug build against the 50 ms budget. The driver's cost, not the store's.
- For sqlx the "remove the lock" break stays green, because sqlx's migrator holds its own advisory lock. The ordering break goes red. SeaORM and Diesel go red under both.
- `ulo-db-prisma` is not built; Prisma Client Rust was archived on 2025-03-12.
