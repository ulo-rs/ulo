# Race 3: divergences from the design

Where race 3's code departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and
fifth responses, or fills what they leave open, gathered from the logs under `divergences/`:
`race3-S.md` (the spine's core and transport extensions), `race3-S-move.md`, `race3-S-skel.md`, one
per area (C `ulo-config`, H1 `ulo-health` and `ulo-health-http`, H2 `ulo-health-grpc`, D1 `ulo-db`,
D2 `ulo-db-sqlx`, D3 `ulo-db-conformance`, S2a SeaORM, S2b Diesel, S2c MongoDB, S2d Redis, L
`ulo-cli`, O1 the hubs' trace carriers, O2 `ulo-otel`, O3 `ulo-otel-otlp`), and `race3-step5.md`
(the `close` claim and the owners' drops). Also merged: what the builds after `QUESTIONS_4.md`
established, cited by commit, and the premise checks in `PROBES_config.md` and `PROBES_db.md` §9.

## How to read it

- **Citations.** `[S C1]`, `[S-skel D2]`, `[D1 DV5]`, `[step5 D3]` name the log and its entry;
  `[0051f07c]` a commit. A point several logs cover appears once, with every citation.
- **Design numbers.** "Decision n" is `integrations/DESIGN.md` §11, "transports n" `transports/DESIGN.md`
  §13, "§15" the integration design's points open for the designer.
- **Resolutions.**
  - *Ruled*: a response decided it, named with its item; the design states the outcome.
  - *Superseded*: a later entry or build replaced what this one describes.
  - *Open*: built as described, not put to the designer by `QUESTIONS_4.md`, and standing until it is
    signed off. Where the design now states the behaviour, the entry says so.
  - *Open for the designer*: raised by a build and unanswered, listed in integrations §15 or in a
    transports decision.

---

## 1. Rulings applied after the race

### R1. `Store::create_connects(o)` replaces the constant [0051f07c, S-skel D2, D1 DV11, QUESTIONS_4 Q5]

- **Design:** `const CREATE_CONNECTS: bool` per store; MongoDB set it for the whole store (fifth
  response, S2), and `.lazy()` was absent at compile time where it was true.
- **Built:** `fn create_connects(o: &Self::Options) -> bool` and a defaulted `connects_because(o)`.
  MongoDB answers by scheme; every other store false. `.lazy()` on options answering true is
  `LazyRefused` through `try_value`, at `wire()`. D1's inline `const { assert!(..) }` in `lazy()` is
  gone with the constant.
- **Resolution:** Ruled, seventh response (Mongo option (b)). Decisions 62, 63, 64.

### R2. Redis is lazy, through `ConnectionManager::new_lazy_with_config`, with `set_number_of_retries(0)` [0051f07c, 74314d34, S2d, QUESTIONS_4 Q5]

- **Design:** `ConnectionManager::new` connects, so Redis set the constant and had no `.lazy()`;
  `set_number_of_retries(0)` kept the library from retrying inside `ulo-db`'s loop.
- **Built:** the lazy constructor, `create_connects` false, `.ready(ping)` the retry loop, `.lazy()`
  offered, `AuthenticationFailed` `Permanent` in `ping`. The retries stay at 0. The seventh response
  made the setting depend on what it governs, nonzero if it also governs reconnects while serving, and
  left the answer to the probe. `PROBES_db.md` §9 found that it governs every reconnect; §9.1, the
  check, found that a nonzero setting shortens no recovery and only lengthens a failing command (a
  wrong password 2 ms at 0, 1.2 s at 3, 9.5 s at 6). A fake store whose `create` connects now drives the create-loop scenario, which no shipped host
  reaches (`crates/ulo-db-conformance/tests/create_loop.rs`).
- **Resolution:** Ruled, seventh response, decided by the probe as that response asked. Decision 65.

### R3. SeaORM on SQLite through `ulo-db-sqlx`'s lazy pool [0051f07c, QUESTIONS_4 Q2]

- **Design:** `ConnectOptions::connect_lazy(true)` on every engine.
- **Built:** on SQLite, `sea-orm-migration`'s default features turn on `sqlite-use-returning-for-3_35`,
  under which SeaORM's connector runs `SELECT sqlite_version()` while connecting, so the pool is
  `ulo-db-sqlx`'s, wrapped by `SqlxSqliteConnector::from_sqlx_sqlite_pool`; the three pool options map
  onto SQLx's (`max_connections` 1 unset, `acquire_timeout_secs` 5 s unset); `ping` runs
  `sqlite_version()` and refuses below 3.35 as `Permanent`. The lazy scenario runs on SeaORM's SQLite.
- **Resolution:** Ruled, seventh response, withdrawing the sixth's declared exception. Decision 66.

### R4. A migration-lock wait reaching the bound is retryable [6f6a7a7d, e719f88b, D1 DV5 and its Q1, QUESTIONS_4 Q3]

- **Design:** every `MigrationFailed` inside `Permanent`.
- **Built:** `MigrationFailed::waited_for_lock()`; `LockTimedOut`, public, which a store's `lock`
  returns when its own wait ran out; the init hook raises the lock wait bare. `tests/lock_wait.rs` in
  `ulo-db-conformance` (a fake lock) and in `ulo-db-sqlx` (a SQLite sidecar held by another handle)
  hold it; the second reaches the store's own timeout path, which the first fake answering at the
  bound did not.
- **Resolution:** Ruled, sixth response, Q3. Decision 68. A lock obtained late and then timed out is
  `Permanent`: open for the designer (§15, F443).

### R5. `ModuleDef::qualifier` and `S::NAME @ Q` [6f6a7a7d, D1 PD3 and R1, D3 R1, H1 V10, QUESTIONS_4 Q4]

- **Design:** silent on naming two keyed checks apart.
- **Built:** D1 named checks `S::NAME@path`, which gave two `from_options` instances one name. The core
  gains `ModuleDef::qualifier(&self) -> Option<&'static str>`, and a keyed module appends ` @ Q`, `Q`
  written short. The keyed scenario requires distinct names.
- **Resolution:** Ruled, sixth response, Q4. I17, decision 69. Two limits, equal last segments and a
  store inside a keyed application module, are open for the designer (§15).

### R6. `Defaults::of::<T>()` [c46c932d, C P-C3, QUESTIONS_4 Q1]

- **Design:** `.source(Defaults)` reads the type's `Default`.
- **Built:** C found the frozen signatures name no type and left `Defaults` inert, defaults coming
  through `#[serde(default)]`. `Defaults::of::<T: Default + Serialize>()` with `.at(path)` now reads
  `T::default()` into the tree with the origin `default`; the unit `Defaults` stays, inert; a section
  holding a `Secret` cannot use it (a `compile_fail` doctest).
- **Resolution:** Ruled, sixth response, Q1. Decision 73.

### R7. `ulo_grpc::outgoing` refuses the reserved trace headers [c46c932d, O1 O1-D1, QUESTIONS_4 Q6]

- **Design:** §6.3 states the reservation for the RPC client.
- **Built:** O1 had `outgoing` replace a caller's `traceparent` and, on a `Received` context, its
  `tracestate`. It now refuses both as `INVALID_ARGUMENT` with `reserved_header`, through
  `trace::reserved` called before `inject`, whose signature is unchanged; four unit tests.
- **Resolution:** Ruled, sixth response, Q6. Decision 76, I8.

### R8. The process environment under the OpenTelemetry crates [c46c932d, O2 O2-8, O3 2 and 4, QUESTIONS_4 Q7]

- **Design:** options from a given `Env`.
- **Built:** the SDK and `opentelemetry-otlp` read `OTEL_*` names from the process; both crates'
  documentation states it. An explicit value wins, except headers, where the environment's entries are
  merged over the options'. The default `ureq` client reads the proxy variables.
- **Resolution:** Ruled, sixth response, Q7, for the specification's variables; the header exception
  and the proxy read were found by the build. Decision 77.

### R9. Redis's runtime preference [6f6a7a7d, bba88873, S2d 1, S-skel D19, QUESTIONS_4 Q8]

- **Design:** the fifth response's S3: call `prefer_tokio()` once, read the preference where possible,
  refuse a non-tokio one, `warn` otherwise.
- **Built:** S2d built the guard with no call, since `prefer_tokio()` exists only with both runtime
  features. A `smol-comp` feature now compiles redis-rs's in and makes the call, a refusal a `warn`;
  with it off nothing is called. The feature also enables redis-rs's `smol-rustls-comp` and
  `tokio-rustls-comp`, without which `--all-features` does not compile.
- **Resolution:** Ruled, sixth response, Q8. Decisions 60, 70.

### R10. `OtelModule` with no `Timer` [O2 P1, QUESTIONS_4 Q9]

- **Built:** the flush hook bounded `After(flush_timeout + 1 s)`, refused at `wire()` with
  `BoundWithoutTimer` in an app with no `Timer`.
- **Resolution:** Ruled, sixth response, Q9: confirmed. Decision 78.

### R11. MySQL over TLS, and a refused certificate is `Permanent` [6f6a7a7d, e719f88b, QUESTIONS_4 Q10]

- **Design:** TLS on by default; the suite ran without it.
- **Built:** `ulo-db-sqlx`'s `tests/tls.rs` against MySQL 8 with a test CA's certificate. SQLx returns a
  failed handshake as `Io` around `rustls::Error`, which the classifier retried to the bound;
  `InvalidCertificate` is now `Permanent` except `Expired` and `NotValidYet`. Each store's
  documentation marks an unexercised TLS path.
- **Resolution:** Ruled, sixth response, Q10, MySQL first; the classification was the build's.
  Decision 72. Other certificate failures and SeaORM's classifier are open for the designer (§15, F442);
  MongoDB's recipe is deferred.

### R12. SQLx's `PoolConnection` dropped off the runtime [6f6a7a7d, 8aaa40a3, QUESTIONS_4 Q11]

- **Design:** the stage-2 test decides whether a smol feature in the tree is harmless.
- **Built:** the test measured a panic with `runtime-tokio` alone and smol's global executor with
  `runtime-smol`; both crates document it. The test moved to `ulo-db-sqlx-smol-probe` so
  `ulo-db-sqlx`'s own tests build the production SQLx.
- **Resolution:** Ruled, sixth response, Q11: documented. Decision 71.

### R13. A zero `roots_timeout` on a NATS client [c46c932d, QUESTIONS_4 Q12, Q13]

- **Built:** refused in `Link::usable`, so `RpcClient::new` answers `UnusableLink`; `Bound::Default` is
  the link's own 30 s.
- **Resolution:** Ruled, sixth response, Q12 and Q13. Decision 79, transports 150.

### R14. The MongoDB stall measured in release [QUESTIONS_4, for the record]

- **Built:** the scenario runs without authentication; release, authenticated: 7.8 to 8.0 ms against
  50 ms, in the store's documentation.
- **Resolution:** Ruled, sixth response, on the record. Decision 74.

### R15. The store suite's readings, made able to fail [abdabea2, 61c396b4, D3 V4, V6 and its checks]

- **Design:** §4.9's scenarios and their planted breaks.
- **Built:** the migration count is read once after both replicas finish, and the replicas meet at a
  rendezvous before migrating, where a count per replica counted one application twice; each host
  bounds an attempt below the connect bound, where SQLx's 5 s acquire and MongoDB's 30 s selection
  outlasted the 3 s bound; the keyed scenario closes one pool and pings the other; a known defect is
  declared `pending`. With the readings fixed, the lock-removal and ordering breaks are both green on
  SQLx, whose own lock makes `ulo-db`'s redundant, and SeaORM and Diesel on Postgres fail without it on
  a real double application.
- **Resolution:** the SQLx result is Ruled, sixth response: accepted. The readings and `pending` are
  Open. Decisions 81, 82. The scenario comment still names the ordering break as SQLx's evidence
  (F448).

### R16. The `scrub` pass for a secret under `flatten` [9331527c, C after the fill]

- **Design:** R3 (fifth response): a `Secret`'s inner `Error::custom` under `flatten` is printed as
  written, no second pass being able to help.
- **Built:** the premise check found the message leaking the secret and the collector holding its
  text; `scrub` rewrites a problem containing a collected text. Limits in integrations §2.3.
- **Resolution:** Open: built on the premise check, not put to the designer. Decision 75. The doc on
  `Secret` in `crates/ulo/src/redact.rs` still states the earlier limit (F441).

### R17. CI: the template matrix, the store suites, `ran_a_test.py` [8aaa40a3, L request 2]

- **Built:** `template matrix` over 22 projects (every transport bare, every database on `http`,
  `generate`), not the full cross of 120; `store conformance` on push; `scripts/ran_a_test.py` shared
  with `unsafe_allowed.py miri`. The two-SQLx `cargo tree -d` job decision 26 names is not among them
  (F447).
- **Resolution:** Open. Decision 83.

---

## 2. The spine: core and transport extensions [S]

- **S C1. `is_permanent` is true for every `StartupError::Wiring` and for `Bind` carrying
  `RuntimeMissing`,** beyond the walk for `Permanent`, so a configuration that failed to load reads as
  a misconfiguration. *Open.*
- **S C2. The walk enters a `Redacted`'s original and follows its `source()` chain,** through a
  `pub(crate)` `Redacted::original`, depth 32, so a user's error carrying a `Permanent` is found.
  *Open.*
- **S C3. A readiness attempt stops on a `Permanent` under `ConstructError::Dependency` too,** not only
  `Failed`. *Open.*
- **S C4. The collector records the text of every scalar `T`'s `Deserialize` visits,** so `Secret<T>`
  is collected whatever `T` is; nothing inside a sequence, map or enum, nothing under an `Option`
  request, no `bool` or unit; a value is recorded before `T` accepts it. *Open*; integrations §2.3
  rests the `scrub` pass on the last property.
- **S C5. `CollectedSecrets` gains `Clone`, `Default`, `append`, `texts`, `len`, `is_empty` and a
  count-only `Debug`,** answering S-skel's R1. *Open.*
- **S C6. The collector is outside the `serde` feature; the `Deserialize` impl is behind it.** *Open.*
- **S C7. I12's help text** ("the same type from two versions of its crate; `cargo tree -d` lists
  them") and `near_spelling` preferring an equal full name. *Open.*
- **S C8. `TraceContext`'s methods beyond `origin`:** every field value read, the received text kept
  trimmed and joined, a failed `tracestate` forwarded as received, `new` refusing zero IDs,
  `TraceState::prepend` for the merge. *Open.*
- **S C9. `TraceMode::Off` is enforced in the core, and `AppHandle::trace_mode()` is added.** *Open.*
- **S C10. `set_trace` changes nothing once `dispatch` has set the handler,** enforced where `route_to`
  states it. *Open.*
- **S C11. `execute` runs the closure inside the span `ExecOptions::span` hands it,** source or not.
  *Open*; answers integrations §14's observability point.
- **S C12. A `dyn TraceSource` that cannot be read at `connect` fails the connect** as
  `ConnectError::Construct`, beyond `NotFound`. *Open.*
- **S C13. `span::call` takes a `Kind` and records `otel.kind` at creation.** `tracing-opentelemetry`
  fixes a span's kind when it starts, inside `call`, so a kind recorded later never reaches the
  exporter. *Open.*
- **S C14. `span::call(app, exec, kind, transport, name, handler)`;** a second span for one execution
  starts on its own context. *Open.*
- **S C15. `GrpcHealth`'s copy is a `tokio::sync::Mutex` held across the reporter's await;** server
  writes skip `""` after `delegate_overall`. *Open.*
- **S C16. The RPC reservation matches the trace headers' names case-insensitively;** `deadline-ms`
  stays exact. *Open.*
- **S C17, O1 O3. `crates/ulo-ws/src/gateway.rs` is not edited:** line 576 is a per-gateway resolve
  execution with no connection; a hand-written gateway's execution is `connection.rs:143`. *Open.*
- **S C18, O1 P3. The WebSocket connection keeps its context in `ConnInner::trace`,** and
  `Connection::exec_options` opens every later execution with it as `Received`. *Open.*
- **S C19. A hub extracts only when a `traceparent` field is present, and a client injects only when
  the execution carries a context;** `written` gains `within`. *Open.*
- **S C20. `TestApp::trace(TraceMode)`.** *Open*; answers §14's point yes.
- **S C21. `ExecOptions` keeps `Clone`, `Debug` and `Default`, and each setter keeps the other
  fields.** *Open.*

## 3. The shared suite crate [S-move]

- **S-move 1. The `not_applicable` stamping rules stay in `conformance_suite!`,** since they recurse
  through the RPC macro's own arms; `ulo-db-conformance` writes its own stamper of the same shape,
  with a `pending` arm (R15). *Open.*
- **S-move 2. Docs naming `Broker` name "the suite" instead.** *Open.*
- **S-move 3. The two `bound` unit tests moved with `__private`.** *Open.*
- **S-move 4. `ulo_rpc_conformance::__private` re-exports the support crate's module,** so the macro's
  paths resolve unchanged. *Open.*
- **S-move 5. `ulo-rpc-conformance` drops its runtime dependencies,** its features forwarding to the
  support crate's. *Open.*
- **S-move 6. `failures_dir` finds the workspace root by depth,** unchanged. *Open.*

## 4. The skeletons [S-skel]

### `ulo-db`

- **S-skel D1, I2. `Store::secrets(o)`, defaulted, added to the frozen trait,** so a store registers a
  URL's password component, which `StoreModule` cannot parse. *Open*; integrations §4.1 lists it.
- **S-skel D2. `.lazy()` gated by a `LazyStore` marker.** *Superseded in part* by R1: the options now
  decide, refused at `wire()`; the marker stays, implemented by every shipped store. Whether it earns
  its place is open for the designer (§15, F444).
- **S-skel D3. `MigrationLock` (`Guard`, `lock`, `unlock`), `Migrations<S>`, `MIGRATION_LOCK_KEY`.**
  *Open*; integrations §4.1.
- **S-skel D4, D1 DV15. `StoreModule<S>: Module` under `S::Options: 'static`; the identity is the
  options, or the path and error of a failed read;** equal options with different builder settings are
  one module, the first registered winning. *Open.*
- **S-skel D5. `StoreModule::from_options` and `Section::from_value`.** *Open.*
- **S-skel D6. `.keyed::<Q>()` is `Module::keyed`.** *Open.*
- **S-skel D7. Each store crate's module is a type alias of `StoreModule<S>`.** *Open*; answers §14.
- **S-skel D8. `UnsupportedRuntime` and `MigrationFailed` are `#[non_exhaustive]` with public fields.**
  *Open*; `MigrationFailed` gained `waited_for_lock` (R4).
- **S-skel D9. `Policy` is `pub(crate)`.** Its defaults are *superseded* by D1 PD1.

### `ulo-config`

- **S-skel C1. `Source` is sealed.** *Open.*
- **S-skel C2, I3. JSON is not behind a feature; `File::toml`, `File::json`, `File::yaml`.** *Open*;
  answers §14.
- **S-skel C3. `DotEnv` gains `.prefix` and `.separator`.** *Open.*
- **S-skel C4. `Env::var(name)`,** for `ulo-otel`. *Open.*
- **S-skel C5. `Values::set(path, impl Into<Scalar>)`.** *Open.*
- **S-skel C6. `Problems::at` public, `at_origin` crate-private; `Problem`'s accessors.** *Open.*
- **S-skel C7. `Section<T>`, `ConfigError`, `Origin` (`#[non_exhaustive]`).** *Open.*
- **S-skel C8. `ConfigModule<T>`: `new`, `.validate`, `.deny_unread`, identity by type.** *Open.*
- **S-skel C9. `Config`'s and `Tree`'s `Debug` print no value.** *Open.*

### Health

- **S-skel H1. `HealthModule::new().lag(..)`; `Readiness`'s methods; `ZeroBound`.** *Open*; answers
  §14.
- **S-skel H2. `HealthHttp` imported as a module.** *Open*, refined by H1 V9.
- **S-skel H3. `HealthGrpc::new().interval(Bound)`, a zero refused at `connect`.** *Superseded* by
  H2 2: at `wire()`.

### Stores

- **S-skel D10. `permanent_or_not(Engine, sqlx::Error)`.** *Open*; integrations §4.1.
- **S-skel D11. `Engine`, `check_scheme`, `validate_url` public,** the shared scheme check. *Open.*
- **S-skel D12. `ulo-db-sqlx`: `default = ["tls"]`, engines off; SQLx's `macros` left to the
  application.** *Open.*
- **S-skel D13. SQLx option types carry `max_connections`, `min_connections`, `acquire_timeout_secs`.**
  *Open.*
- **S-skel D14. `SeaOrmMigrations<M>` wraps a `MigratorTrait`.** *Open*; the lock's field refined by
  S2a 1.
- **S-skel D15. `Given` is `pub` and `#[doc(hidden)]`,** being in the SQLite pool's type. *Open*,
  refined by S2b.
- **S-skel D16. One `DieselOptions`, `DieselLock`, `DieselMigrations<M>`.** *Open.*
- **S-skel D17. Diesel's features; a `deadpool` 0.13 workspace entry.** *Open.*
- **S-skel D18. MongoDB enables the driver's three default features explicitly.** *Open.*
- **S-skel D19. `rediss://` behind an opt-in `tls` feature.** *Open*; the preference stub is
  *superseded* by R9.

### Suite and OpenTelemetry

- **S-skel D20. The `Host` trait and `MigrationProbe`; `PASSWORD = "hunter2"`.** *Open*, refined by D3
  and R15.
- **S-skel D21, D3 V5. Twelve stamped tests, the migrations row split in two.** *Open.*
- **S-skel O1. `Otel::new(options, exporter)`, `Exporter` with an associated span type,** where the
  design writes `Otel::new(options)`. *Open.*
- **S-skel O2. `OtelOptions`' fields, `headers` a `Secret`.** *Open.*
- **S-skel O3. `ureq` with `rustls-no-provider` and `rustls-webpki-roots`.** *Superseded* by O3: the
  platform verifier.
- **S-skel I1. `ulo-cli` is not in the skeletons.** *Open.*

## 5. `ulo-config` [C]

- **C P-C1. A section's problems carry absolute paths,** the deserializer starting at the section;
  `ulo-db` prefixes a store's own. *Open.*
- **C P-C2. Unread leaves are tracked in one shared set per `Config`,** compared with the tree at
  `connect`. *Open.*
- **C P-C3. `Defaults` read as a layer with no values.** *Superseded* by R6.
- **C P-C4, C 15. Merge keys and the alias count come from one scan of `saphyr-parser`'s events,**
  the count reset per document; the refuse-every-alias fallback is unwritten, the events carrying what
  the count needs. The bound holds as the fifth response's R2 ruled (decision 80).
- **C 1. A variable or `.env` key becomes a lowercase dotted path** after the prefix, split at the
  separator. *Open.*
- **C 2. Environment values are untyped text,** parsed as the field asks; under `deserialize_any`
  (`flatten`, `untagged`, `Value`) a number cannot come from the environment. *Open.*
- **C 3. Source failures and typed problems are both reported,** a failed file's keys then missing.
  *Open.*
- **C 4. Reader rules:** a duplicate JSON key, a multi-document or non-mapping YAML file, a TOML date as
  text, integers as `i128`, a BOM skipped, a parser's text not reused when it quotes the source.
  *Open.*
- **C 5. `.env` fills:** spaces around `=`, a key charset, unknown escapes and unclosed quotes refused,
  `KEY=a#b` kept. *Open.*
- **C 6. `Origin`'s `Display` omits a zero line or column.** *Open.*
- **C 7, C 8. The tree's `Integer(i128)` and `Text`; `NodeDeserializer` and `DeError` restructured.**
  Internal. *Open.*
- **C 9. R3's rendering text,** an out-of-range integer caught before the visitor; a custom message
  under a secret's path replaced. *Open.*
- **C 10. Missing fields rerun the pass, at most 2048 times;** a placeholder-less enum ends discovery.
  *Open.*
- **C 11. `Section` keeps both records' texts and registers them on success and failure.** *Open.*
- **C 12, C 13. `ConfigModule` validates only a loaded value; the report's headers and line format.**
  *Open.*
- **C 14. `Env::process()` skips a variable that is not UTF-8.** *Open.*
- **C after the fill: a type mismatch under `flatten` is attributed to a sibling field's origin with an
  empty path.** Not fixed (F440).

## 6. Health [H1, H2]

- **H1 P1. `Readiness` answers from its own state,** flipped by its before-shutdown hook, and does not
  observe `listen`; "false before `listen`" holds by construction. *Open.*
- **H1 P2. `Readiness::subscribe() -> Changes`, resolving at `Stopping`.** *Open*; H2 does not use it.
- **H1 P3. The `.lag` guidance is in the documentation.** As designed.
- **H1 V1. The before-shutdown hook is a closure hook bounded `lag + 1 s`,** the trait's `TIMEOUT`
  being a constant. *Open.*
- **H1 V2, H2 request. `HealthModule` is global,** so sibling modules read one `Readiness`. *Open.*
- **H1 V3. A zero `Check` bound is the `Readiness` binding's construction error,** naming the first
  such check. *Open.*
- **H1 V4. After `Stopping`, readiness answers one down entry, `lifecycle`.** *Open.*
- **H1 V5. A panicking check is down for that run.** *Open.*
- **H1 V6. The shared run is per probe, and a run every caller abandoned is dropped.** *Open.*
- **H1 V7. `startup()` is always up and `liveness()` ignores `Stopping`.** *Open.*
- **H1 V8. A down reason is redacted where `ulo-health-http` shows it,** through the route's
  `AppHandle`; elsewhere it is stored unredacted. *Open.*
- **H1 V9. Each probe is a controller of its own, placed by `ControllerHandle::at`.** *Open.*
- **H1 V10. `checks` maps a name to an array.** *Open*; the duplicate name is resolved by R5.
- **H1 V11. `ulo-health-http` depends on `http`.** *Open.*
- **H1 V12. The health module requires a `Timer`.** *Open.*
- **H2 1. The bridge imports `GrpcHealth::module()` itself.** *Open*; answers §14.
- **H2 2. A zero interval is refused at `wire()`, through `try_value`.** *Open*; integrations §10
  states it.
- **H2 3. `""` turns `NOT_SERVING` in the bridge's own before-shutdown hook,** after aborting and
  awaiting the re-evaluation task. *Open.*
- **H2 4. The first evaluation runs on the task, not in the init hook.** *Open.*
- **H2 5, 6. `Unbounded` evaluates at start and at `Stopping`; a pass that reads `Stopping` writes down
  and ends.** *Open.*
- **H2 7, 8, 9. The task is aborted with the bridge's state; the state is a private singleton with
  module hooks; a panicked task is logged.** *Open.*

## 7. `ulo-db` [D1]

- **D1 PD1. `.retries(..)` unbounded and `.backoff(..)` 1 s by default,** so `connect_within` ends a
  loop against a database that refuses at once. *Open*; integrations §4.2 states it.
- **D1 PD2. `K` checked against the SHA-256.** As designed.
- **D1 PD3. Checks named `S::NAME@path`.** *Superseded* by R5.
- **D1 DV1, DV2. `Migrate<S>` holds the migrations beside `run_locked::<S>`; crate-private signatures
  take the runtime.** *Open.*
- **D1 DV3. The health ping runs through `spawn_with`.** *Open*; integrations §4.2.
- **D1 DV4. Every task `ulo-db` spawns is aborted when its awaiting future drops.** *Open.*
- **D1 DV5. The migration bound is `connect_within` over wait, migrations and release; the hook is
  unbounded.** *Open*; its permanence question is *Ruled* by R4.
- **D1 DV6. A zero `connect_within` refused through `try_value`.** *Open.*
- **D1 DV7. `validate`'s problems are rewritten under the section's path as `Problems`.** *Open.*
- **D1 DV8. A `create` task that panics ends the loop as `TaskFailed`.** *Open.*
- **D1 DV9. `from_options` registers only what `Store::secrets` answers.** *Open.*
- **D1 DV10. An app with no runtime fails `wire()` on a missing `Dep<dyn Runtime>`.** *Open.*
- **D1 DV11. `lazy()`'s inline `const` assert.** *Superseded* by R1.
- **D1 DV12, DV13. The errors' `module` is `S::NAME`; the retry log names no error.** *Open.*
- **D1 DV14. `.lazy()` with `.migrate(..)` still waits at start.** *Open.*

## 8. `ulo-db-sqlx` [D2]

- **D2 1. A parse failure never prints the URL,** SQLx's detail kept only while it holds none of it.
  *Open.*
- **D2 2. `acquire_timeout` is 5 s by default.** *Open*; decision 67.
- **D2 3. `validate` refuses a zero maximum, a minimum above the maximum, a zero timeout, and an engine
  whose feature is off.** *Open.*
- **D2 4. The Postgres and MySQL lock refuses a pool of one.** *Open*; integrations §4.4.
- **D2 5. Lock connections are closed on drop, never pooled.** *Open.*
- **D2 6. Postgres bounds the wait with `lock_timeout`.** *Open*; not probed.
- **D2 7. MySQL's `GET_LOCK` timeout is whole seconds, rounded up; a name past 64 characters is
  FNV-1a.** *Open*; answers §14.
- **D2 8, 9. SQLite's lock path comes from `pragma_database_list`; the lock polls `try_lock`.** *Open*;
  answers §14.
- **D2 10. In-memory SQLite pools disable the idle and lifetime reapers.** *Open.*
- **D2 11. SQLite's permanent code is a low byte of 14.** *Open*; integrations §4.6.
- **D2 12. `futures-channel` added.** *Open.*
- **D2 requests.** The lock steps run as one task through `spawn_with`, and `unlock` is awaited on
  every path: met by D1's `run_locked`.

## 9. The store suite [D3]

- **D3 P1–P6. The drain query in a standalone execution; two apps in one process for the replicas; no
  transport bound; checks found by `S::NAME`; the unknown runtime is the harness's renamed; the
  bounds.** *Open.*
- **D3 V1. `futures-util` added.** *Open.*
- **D3 V2. `bad_credentials` must refuse `PASSWORD`.** *Open.*
- **D3 V3. `cut` and `restore` stated precisely.** *Open.*
- **D3 V4. `MigrationProbe` counts applications.** *Superseded* by R15.
- **D3 V6. Breaks some stores cannot show.** The SQLx entry is *superseded* by R15; Redis's close
  stands; no scenario shows the close on SeaORM's SQLite or MongoDB (F439).

## 10. The stores [S2a, S2b, S2c, S2d]

- **S2a 1. SeaORM's lock is `ulo-db-sqlx`'s primitives over SeaORM's SQLx pool.** *Open.*
- **S2a 2, 3. Engine detection by SeaORM's own schemes; an engine whose feature is off is a problem.**
  *Open.*
- **S2a 4. Pool-size checks in `validate`.** *Open*; a minimum above the default maximum is not refused
  (F438).
- **S2a 5. Several texts registered for the password.** *Open.*
- **S2a 6. `create` scrubs `Database::connect`'s error itself, as `Permanent`.** *Open.*
- **S2a 7. Classification written again over `DbErr`.** *Open*; it lacks R11's certificate case
  (§15, F442).
- **S2a 8. `ping` is `DatabaseConnection::ping`.** *Open*; on SQLite *superseded* by R3.
- **S2a 9. SeaORM's statement logging unchanged.** *Open*; on SQLite SQLx's defaults apply.
- **S2b, classification. diesel-async's errors are classified by matched English messages,** the URL
  being unreachable from the pool. *Open*; integrations §4.6; answers §14.
- **S2b, SQLite lock. On the blocking pool, polling `try_lock`.** *Open.*
- **S2b, `Given::get_runtime` panics, naming why.** *Open*; answers §14.
- **S2b, decisions. The lock's session from the pool's manager; `lock_timeout` on Postgres; the MySQL
  lock name equal to SQLx's; migrations on a pooled connection in `Given::run`; `ping`, `close`,
  `validate`'s parsers, `secrets`, deadpool's runtime for a timeout.** *Open.*
- **S2b, divergences. `tokio-postgres` and `mysql_async` as optional dependencies; `rt-multi-thread`; a
  SQLite query dropped mid-task waits through `block_in_place`; `Given` compiled for every engine; the
  migrations' `Given` read from the task's runtime; `try_current` in `create`.** *Open.*
- **S2b, hazards. A migration thread outliving the bound; `pragma_database_list`'s SQLite version;
  MySQL's `caching_sha2_password` without TLS, unprobed.** *Open.* The one-thread scenario fails on
  Diesel's Postgres about one run in three (F437).
- **S2c. No SRV lookup failure is permanent; the permanent set adds `InvalidTlsConfig`,
  `IncompatibleServer` and command code 18; options override the URI; `validate`'s host checks;
  `close` is the driver's graceful `shutdown`; `ping` on `admin`; `tracing` removed.** *Open*;
  integrations §4.6. Laziness *superseded* by R1.
- **S2d 2. The guard is called from `create`.** *Open.*
- **S2d 3, 4. `secrets` from `Client::open`; `validate` refuses zero timeouts, `rediss://` without
  `tls`, and `valkey://` and socket schemes.** *Open.*
- **S2d, points. One attempt bounded by a 5 s connection timeout; `AuthenticationFailed` and
  `InvalidClientConfig` permanent; `close` drops the handle.** *Open*; laziness *superseded* by R2.

## 11. The command line [L]

- **L, points. The `--transport` and `--db` names; the no-tests count in a shared function (built as
  `ran_a_test.py`, R17); the `resolver` template on `async-graphql` alone; every `ulo-*` crate pinned
  exactly.** *Open*; integrations §5 states the pins; answers §14.
- **L, decisions. A flat layout; one spine per project; `main` reads the address after `connect`;
  health per transport with a probe protocol; tests that need no network; `generate`'s paths and
  printed lines; `generate controller --transport <http|rpc>`; an `allow(dead_code)` on the database
  service; cleanup on a failed write; name refusals; no gRPC client.** *Open.*
- **L, divergences. `ulo new` writes the `http` template; `regex` removed; `generate resource` gone;
  no `--dir`; the `rpc-udp` template is TCP's with another link; `mongodb` and `redis` take no backend;
  no emoji; the CLI carries no tests of its own.** *Open.*

## 12. Trace propagation and OpenTelemetry [O1, O2, O3]

- **O1 P1. What marks a server span an error:** HTTP 5xx, gRPC's six server codes, and on RPC and
  WebSocket the kinds HTTP renders 5xx; WebSocket close codes 1011 to 1014. *Open.*
- **O1 P2. `otel.kind`: gRPC and WebSocket `Server`, RPC `Server` for a request and `Consumer` for an
  event.** *Open.*
- **O1 O4, O5. A header value outside visible ASCII is off the grammar; RPC extraction is
  case-insensitive.** *Open.*
- **O1 O6. RPC injection removes every case of both headers, merges by `prepend`, drops a caller's
  off-grammar `tracestate` with a `warn`.** *Open.*
- **O1 O1-D1.** *Superseded* by R7.
- **O1 O1-D2. A `traceparent` that cannot travel as gRPC metadata is not written.** *Open.*
- **O1 O7. Failures recorded on `Span::current()`.** *Open.*
- **O2 P2. The `OTEL_*` mapping, `always_on` and its kin refused rather than read as parent-based.**
  *Open.*
- **O2 O2-1. `Otel::new` refuses a ratio outside 0 to 1, a zero flush timeout and malformed headers.**
  *Open.*
- **O2 O2-2, O2-3, O2-5. An absent inbound context leaves the parent to `tracing`; `start` without an
  OpenTelemetry span answers the inbound context or an unsampled new trace; a `Received` parent is
  remote.** *Open.*
- **O2 O2-4, O2-6, O2-7. A shared `flushing` flag; the module is global; the headers registered whole
  and per value.** *Open.*
- **O2 O2-9 to O2-13. The sampler variant; the instrumentation scope `ulo-otel`; the flush thread's
  outcome logged; the `trace` feature named; field docs.** *Open.*
- **O3, provider and roots. ring handed through `unversioned_rustls_crypto_provider`; the platform
  verifier in place of webpki roots.** *Open.*
- **O3, S7. The refusal for two linked providers is unreachable on this path; the one remaining
  refusal has its own message.** *Open.*
- **O3 1. `reqwest-blocking` compiles the `ureq` client out, `ureq` staying a dependency.** *Open.*
- **O3 2, 3. `/v1/traces` appended to the endpoint; a fixed 10 s request timeout.** *Open.*
- **O3 4. Headers parsed and refused by position.** *Open*; the environment's precedence is *Ruled* by
  R8.
- **O3 5, 6. The protocol set to `HttpBinary`; a 4 MiB response cap; proxy from the environment.**
  *Open*; integrations §6.4 states the protocol and the proxy read.

## 13. The `close` claim and the owners' drops [step5]

- **step5 D1, F1. `AppHandle::close` claims the servers at the call and triggers at the first poll,
  returning `impl Future + Send + use<>`.** The ruling's signature, edition 2024, would capture the
  borrow of `self` and refuse `tokio::spawn(handle.close(..))`; `use<>` is the build's.
  *Ruled*, sixty-seventh response, Q14; `use<>` *Open*. Transports 153.
- **step5 D2. A `close` made before the drop and polled after it runs its hooks;** the ruling's
  fallback is not built. *Ruled*, Q14.
- **step5 D3, F3, 9d913953. `ShutdownFailure::Dropped` for a claim that finds the servers released by
  a drop, and for every later one, read from a `released` flag after a second `close` beside the
  first answered `Ok`.** *Ruled*, Q14; the `Display` text departs from the ruling's wording, which is
  false for an abandoned shutdown whose hooks ran, *Open*. That such a close leaves `phase()` at
  `Running` and `draining()` unfired is open for the designer (transports 153).
- **step5 D4. The embedding's drop releases an installed host future; a `host` after the drop is
  refused.** *Ruled*, Q15 (F433). Transports 157.
- **step5 D5, F2. The hub's delivery task holds a `Weak`, the hub's drop aborts it, the Redis
  subscription races its stream's drop, and `UpgradeHandler` gains a defaulted `release` the HTTP
  servers' drops call.** *Ruled*, Q15 (F434); `release`, a public trait addition the ruling did not
  name, *Open*. Two HTTP servers preparing one hand-off is open for the designer (transports 157).
- **step5 D6, F4. No shared owned-task type;** the hub, `fw-rpc-tcp-core` and `fw-rpc-udp` each abort
  a handle on drop. Open for the designer (transports 157).
- **step5 F5. The in-memory adapter needs only the `Weak`;** the abort is held by the Redis test.
  *Open.*
- **Batch 45 S3, extended by the claim at the call. A `close` holding its claim and neither polled to
  its end nor dropped holds the servers, which now includes one made and never polled; one stored in
  the app's own state is a cycle.** Open for the designer (transports 153).
