# Integration layer: designer responses

## First response: the integration layer from the capability brief

Received 2026-10-17, answering `CAPABILITIES.md`. The designer did not have `DESIGN.md` or `transports/DESIGN.md` (they were named in the brief but not pasted), so every dependency on the core is stated as a premise. The text below is the designer's design as received, trimmed of the conversational lead-in. It is a design awaiting review, not a signed-off decision.

### 1. Crates, levels and core extensions

Levels. The brief names two: runtime-free and runtime-neutral. These crates need a third, which the transport layer already has in its links: runtime-bound. A runtime-bound crate is built on one runtime's library. It holds the handle the application gives it, and refuses at `wire` an application on any other runtime. `runtime-levels.txt` needs this level named, with the runtime each such crate is bound to.

| Crate | Level | Why |
| --- | --- | --- |
| `ulo-config` | runtime-free | serde and parsers only. Files are read at `wire`, before anything is served. |
| `ulo-health` | runtime-free | Its bounds run on the core's `Timer`. |
| `ulo-health-http` | runtime-free | Its routes sit on `ulo-http`, which is runtime-free. |
| `ulo-health-grpc` | runtime-bound (tokio) | tonic and tonic-health. |
| `ulo-db` | runtime-free | The shared policy, over the core's `Runtime` and `Timer`. |
| `ulo-db-sqlx` | runtime-bound: `tokio` feature (default) or `smol` feature | SQLx 0.9 has a `runtime-smol` feature. Enabling both is a `compile_error!`. |
| `ulo-db-seaorm` | runtime-bound (tokio), MSRV 1.94 | SeaORM supports only tokio. The 2.0.1 manifest declares `rust-version` 1.94.0. |
| `ulo-db-diesel` | runtime-bound (tokio) | diesel-async is built on tokio-postgres and mysql_async. |
| `ulo-db-mongo` | runtime-bound (tokio) | The driver supports only tokio from 3.0. |
| `ulo-db-redis` | runtime-bound: `tokio` or `smol` feature | redis-rs has `tokio-comp` and `smol-comp`. |
| `ulo-db-conformance` | runtime-bound per run, `publish = false` | The suite. |
| `ulo-otel` | runtime-free (premise below) | Spans and sampling. The batch processor runs on a background thread of its own. |
| `ulo-otel-otlp` | a named allowance | Its default HTTP client is reqwest's blocking client, which runs its own runtime on its own thread and never touches the application's. |
| `ulo-cli` | a binary | Not a library, so it takes no level. |

Extensions to the core's SPI. Each one is needed by a requirement named next to it. Claude Code checks whether the core already provides it before building it.

1. Lifecycle as a value (requirements 6, 10, 15). The core exposes a `Lifecycle` binding with a watched state: `Wiring`, `Connecting`, `Listening`, `Draining`, `Closing`. Health reads it. The `Watch` primitive lives in `ulo-transport::__private`, and the core can't depend on that crate. So by §3.7, `Watch` moves into the core and `ulo-transport` re-exports it.
2. Collecting several bindings of one kind (requirement 8). `Contribute<dyn HealthCheck>` from any module, collected at `wire` as `Contributions<dyn HealthCheck>`. If the core has no multi-binding, this is the extension health needs.
3. Close order (requirement 15). Every transport drains before any module closes, and modules close in reverse dependency order. Premise to check: what the core does today.
4. The given runtime is injectable and can be downcast (requirement 18). The runtime is a binding, `GivenRuntime`, that can be downcast to `ulo_runtime_tokio::Tokio` or `ulo_runtime_smol::Smol`. A runtime-bound module takes the handle from it at `wire`, or refuses.
5. A drain delay (requirement 6). `App::builder().drain_delay(d)`, defaulting to zero. When the drain begins, readiness turns false at once, and the transports start refusing only after `d`. A zero here is meaningful (no delay), so it isn't refused. In Kubernetes, a load balancer keeps routing to a pod for a few seconds after its readiness turns false, and this absorbs that.
6. Health routes are served through the drain (requirements 6 and 9). A route marked `serves_during_drain` isn't refused by the drain. It's refused only at `close`. Without this, liveness fails during every drain and the orchestrator may kill a pod that is shutting down cleanly. This is an addition to the HTTP hub's route options.
7. Exit codes for startup failures. A misconfiguration exits with 78 and a retryable start failure with 75, following the BSD `sysexits.h` convention (`EX_CONFIG` and `EX_TEMPFAIL`). That's a convention, not a standard.
8. Trace context in the core (requirement 26). A `TraceContext` value on `Execution`, forwarded by `.within`, and a `Tracer` SPI. Section 6 covers it.

### 2. Configuration: `ulo-config`

What decided it. figment records which provider produced each value, and its error's `Display` names the key path through that metadata. Its documentation shows a source name and path, but no line. Line numbers are the point of this crate, so it isn't built on figment. Claude Code can show otherwise with a probe: a type error in a TOML file, checking whether figment reports a line.

The crate merges the sources into its own value tree, where every leaf carries its origin: a file with its line and column, an environment variable's name, an explicit value, or a default. Parsers per format:
- TOML: the `toml` crate's spanned parsing.
- JSON: `serde_json` exposes positions only for syntax errors, not for values. Probe a JSON parser that gives value spans. If none is maintained, JSON reports the file and the key path but no line, and the design says so.
- YAML: `serde_yaml` was archived in March 2024. `serde_yml` carries an advisory as unsound and unmaintained, which recommends `serde_norway` as a maintained fork. `serde-saphyr` is maintained and contains no `unsafe` code. YAML goes behind a `yaml` feature on `serde-saphyr`, provided a probe shows its errors give locations. Otherwise, `serde_norway`.
- `.env`: no specification exists for the format, so the design takes the common subset: `KEY=VALUE`, quotes, `#` comments, and no variable expansion. It's read into the layer and never into the process environment. Premise to check: `dotenvy`'s iterator API reads without setting the environment.

API.
```rust
#[derive(Deserialize)]
struct AppConfig { http: HttpOptions, database: SqlxOptions, api_key: Secret<String> }

let config = ConfigModule::<AppConfig>::new()
    .source(Defaults)                                   // from `Default`
    .source(File::toml("config/app.toml"))              // required unless `.optional()`
    .source(DotEnv::file(".env").optional())
    .source(Env::process().prefix("APP_").separator("__"))  // or Env::from_map(map) in tests
    .source(Values::new().set("http.port", 8080))
    .validate(|c: &AppConfig, v: &mut Problems| {
        if c.http.port == 1 { v.at("http.port", "must not be a privileged port") }
    });
```

Sections without the application's type. `ConfigModule` also provides an untyped `ConfigTree`, with origins attached. An integration deserialises its own section from it: `SqlxModule::<Postgres>::new().options_from("database")`. Premise to check: a module's options can be resolved from a binding at `wire`. The core needs that for this, and for every integration that reads configuration.

Refused at `wire`, all problems reported together:
- missing, ill-typed or invalid values, each with its path, its origin and what was expected;
- a required file that is missing or unreadable;
- a key nobody reads. After every integration has taken its section, any leaf no deserialiser consumed is refused, with its origin. That includes an `APP_` variable that matches nothing. A typo in configuration is otherwise silent (§3.1). `.allow_unread()` is the opt-out.

Secrets. `Secret<T>` deserialises transparently. Its `Debug` and `Display` go through the core's redaction, and `.expose()` is the only way to read the value. A refusal involving a secret names its path and what was expected, never the value.

Deferred: reloading while running, because every binding resolved at `wire` would need a second, live way to be read. Remote sources (a secrets manager, a configuration service), because they're network reads at `wire`, and that phase's rules don't cover a network dependency yet.

### 3. Health: `ulo-health`, `-http`, `-grpc`

The check. No macros:
```rust
pub trait HealthCheck: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn check(&self) -> BoxFuture<'_, Health>;      // Health::Up | Health::Down(Reason)
    fn bound(&self) -> Bound { Bound::Default }    // Default = 2 s; After(ZERO) refused as ZeroTimeout; Unbounded opts out
    fn probe(&self) -> Probe { Probe::Readiness }  // Liveness only when set explicitly
}
```
A check that doesn't answer within its bound is down. Reasons go through redaction. A contributed check reaches readiness through extension 2, so it can't be left out by a forgotten registration.

The probes:
- Readiness is true only when the lifecycle is `Listening` and every readiness check is up. It turns false at the moment the drain begins, before the drain delay, and so before any refusal.
- Liveness is true whenever the process answers, plus any liveness checks the user has added. Integrations never contribute to it.
- Startup is true once the lifecycle reaches `Listening`.

Checks run concurrently. Probe requests that arrive while a run is in flight share that run. Keeping results between runs is deferred, because a stale "up" is exactly what readiness exists to avoid.

HTTP. `HealthHttp::new()` serves `/health/live`, `/health/ready` and `/health/started` (each path configurable), all marked `serves_during_drain`.
- Status codes: 200 when the probe passes, 503 when it doesn't. Kubernetes counts any status from 200 to 399 as success.
- The body takes its shape from the IETF draft, which expired in April 2022: the media type `application/health+json`, with `status` as `pass` or `fail`. The draft is a shape we follow, not a contract anyone holds us to.
- Detail is off by default. `.detail(true)` adds the draft's optional `checks` object, with each check's redacted reason.

gRPC. tonic-health pairs a `HealthReporter` with a `HealthServer`. Setting a service's status notifies anyone watching. tonic-health describes itself as following the official protocol closely, but not necessarily every feature, so a probe checks that `Watch` behaves as `grpc.health.v1` specifies.
- The empty service name, and each registered service name, mirror readiness.
- Changes in the lifecycle are pushed at once, so `NOT_SERVING` reaches a watcher as the drain begins.
- Checks are re-evaluated on an interval (5 s by default; a zero is refused), because gRPC's `Check` reads a stored status, not a fresh evaluation.

Scenarios, on every host that serves health:
1. Readiness turns false at the drain, before the first refusal.
2. Readiness is false before `listen` finishes.
3. A check that hangs is down at its bound.
4. The body has no detail by default.
5. Liveness still answers during the drain.
6. Over gRPC, `Watch` receives `NOT_SERVING` at the drain.

### 4. Databases and stores

Excluded: Prisma Client Rust, archived with its "no longer maintained" notice.

The shared policy (`ulo-db`). A new store plugs in by implementing two traits, with no macros:
```rust
pub trait Store: Send + Sync + 'static {
    type Options: DeserializeOwned + Send;     // its config section
    type Client: Clone + Send + Sync + 'static;// the library's own pool or client
    const RUNTIME: RuntimeKind;                // Tokio or Smol
    fn validate(o: &Self::Options) -> Result<(), Problems>;            // wire: parses only, never connects
    fn connect<'a>(o: &'a Self::Options, rt: &'a Given) -> BoxFuture<'a, Result<Self::Client, ConnectError>>;
    fn ping(c: &Self::Client) -> BoxFuture<'_, Result<(), Reason>>;
    fn close(c: Self::Client) -> BoxFuture<'static, ()>;
}
pub enum ConnectError { Unreachable(Reason), Misconfigured(Reason) } // only Unreachable is retried

pub trait Migrations<S: Store> {             // optional; a store without it has no `.migrate`
    fn lock<'a>(c: &'a S::Client) -> BoxFuture<'a, Result<LockGuard, Reason>>;
    fn run<'a>(&'a self, c: &'a S::Client) -> BoxFuture<'a, Result<Applied, Reason>>;
}
```
`StoreModule<S, Name = Primary>` implements the module, written once. It provides `Db<S, Name>`, which derefs to `S::Client`, contributes the health check, and runs the phases:

- `wire`: the runtime check (`GivenRuntime` must downcast to `S::RUNTIME`, otherwise the start is refused, naming the module, the runtime it needs and the runtime it was given); `validate`, which parses the URL with the library's own parser. The refusal is written from the parse failure's category, with any userinfo in the URL replaced by the redaction marker, because a library's own error message may echo the password. Premise to check, per library: whether its parse error contains the URL.
- `connect`: the client is created inside the given handle's `enter()`. SQLx, with more than one runtime feature, picks tokio when `Handle::try_current()` finds a context, so whatever a library spawns at creation lands on the application's runtime. Retries use exponential backoff with jitter (100 ms doubling, capped at 5 s), under a total `connect_within` bound: 30 s by default, a zero refused. `Misconfigured` (bad credentials, unknown database) stops retrying at once and fails the start as a misconfiguration. Each store's mapping from library errors to the two variants is a premise for that store. At the end of the bound, the start fails retryable.
- `.lazy()` is offered only where the library can create a client without connecting: SQLx's `connect_lazy_with`, and Mongo's client. On other stores it doesn't exist, so the refusal happens at compile time. Readiness stays false until the first ping succeeds.
- Migrations, if requested, run after connecting and under `lock`. A failed migration fails the start as a misconfiguration.
- `close` runs after the transports have drained, under the close bound.

Migration locks. Each migrator is checked for whether it guards against several replicas at once:
- SQLx takes its own lock: one connection holds it through the lock, the migration and the unlock.
- SeaORM Migration 2.0.1's `Migrator::up` takes no advisory lock, so two runners can apply the same pending list. A crate built on it found this and added its own lock.
- `diesel_migrations` has no lock.

So `ulo-db` provides the guard itself, written once, and doesn't depend on whether the migrator has one: Postgres `pg_advisory_lock` on a dedicated connection; MySQL `GET_LOCK`, with the name scoped to the database, because MySQL's locks are server-wide; a SQLite file an OS lock on a sidecar file (in-memory SQLite needs no guard). SQLx's own lock sits inside ours, which is harmless. Mongo and Redis have no migrator, so they have no `.migrate`. A migration that uses `CREATE INDEX CONCURRENTLY` can deadlock against a session waiting on an advisory lock, as golang-migrate's issue tracker records. The design documents that hazard, and the suite's lock scenario uses ordinary DDL.

The stores:

| Module | Client provided | URL schemes | Migrator | Lazy |
| --- | --- | --- | --- | --- |
| `ulo-db-sqlx` | `PgPool`, `MySqlPool`, `SqlitePool` | `postgres://` (libpq's connection URI), `mysql://` (convention), `sqlite:` | `sqlx::migrate::Migrator` | yes |
| `ulo-db-seaorm` | `DatabaseConnection` | as SQLx | `sea-orm-migration` | premise to check |
| `ulo-db-diesel` | a deadpool `Pool` of diesel-async connections (bb8 if deadpool proves unmaintained) | as SQLx | `diesel_migrations` through `AsyncConnectionWrapper` | no |
| `ulo-db-mongo` | `mongodb::Client` | `mongodb://`, `mongodb+srv://` | none | yes |
| `ulo-db-redis` | redis-rs `ConnectionManager` | `redis://`, `rediss://` | none | no |

Example:
```rust
App::builder(AppModule)
    .runtime(Tokio::new(handle))
    .import(config)
    .import(SqlxModule::<Postgres>::new().options_from("database").migrate(sqlx::migrate!("./migrations")))
    .import(SqlxModule::<Postgres>::named::<Analytics>().options_from("analytics"));

struct Users { db: Db<Postgres>, stats: Db<Postgres, Analytics> }
// sqlx::query("select 1").execute(&*self.db).await?
```
Two instances of the same store are told apart by the marker type (requirement 16). Premise to check: how the DI core distinguishes two bindings of one type (F311), because this depends on it.

Diesel (requirement 19): diesel-async. Its Postgres and MySQL connections are fully async, so no query blocks. SQLite goes through `SyncConnectionWrapper`, whose `SpawnBlocking` trait defaults to tokio's `spawn_blocking`. The trait's constructor, `get_runtime()`, takes no arguments, so a handle can't be passed in. The default spawns on the runtime the call is running on, which is the given runtime, because the module refuses any other at `wire` and queries run on its workers. Diesel's migrations run through `AsyncConnectionWrapper`, which blocks on futures and must not be called inside a tokio runtime, so the module runs it through the given handle's `spawn_blocking`, which is link-local. The alternatives: a blocking pool owned by the module would duplicate what diesel-async already does; a blocking-work method on the core's `Runtime` still has no runtime-free consumer.

Library versions kept in step (requirement 18). SeaORM 2.0 moves onto SQLx 0.9, and SQLx 0.9 now declares `libsqlite3-sys` as a version range. `ulo-db-sqlx` and `ulo-db-seaorm` share one workspace entry for SQLx. `ulo-db-sqlx` re-exports `sqlx`. A CI job builds an application with both modules and fails if `cargo tree -d` shows two SQLx. An application that holds a second SQLx of its own is refused at compile time: Cargo refuses two copies that link the same native SQLite library if their ranges don't overlap; otherwise the second copy's types don't match `Db<_>`.

The suite. `ulo-db-conformance` runs every scenario requirement 20 lists, against real containers, with a file for SQLite. Capabilities are named positively (`lazy`, `migrations`) and default to true by §3.9, so a store that lacks one declares `not_applicable` with its reason. Each scenario is shown failing against a planted break: a password echoed in a refusal; `Unreachable` treated as `Misconfigured`; readiness set true before the first ping; a pool closed before the drain; a client created on the worker without `enter`; the runtime check removed; two markers that resolve to one instance; the migration lock removed.

Deferred, with reasons: transactions scoped to an execution (they need request-scoped providers); request-scoped providers (a core design of their own); per-test databases (they depend on the suite's container harness settling first).

### 5. The command line

- `ulo new <name> --transport <http|grpc|rpc-tcp|…> [--db <module>-<backend>]` generates a project from templates compiled into the binary, with no network fetch. The generated project depends on `ulo = "=<the CLI's version>"`, an exact pin.
- CI builds every combination of transport and database, runs each generated project's tests, and reuses the existing check that fails a job running no tests (§3.7). Generated code passes the generated-code lints.
- `ulo generate <module|provider|controller|gateway|resolver> <name>` writes new files and prints the line that registers each one. It doesn't edit existing source: a parser round trip loses formatting and comments. It refuses to overwrite an existing file, and refuses a name that isn't a valid identifier.
- `ulo dev` is unchanged. Its Windows gap (F305) stays recorded.

### 6. Observability: W3C Trace Context and OpenTelemetry

The core. `TraceContext` is a value in the core, runtime-free, with no OpenTelemetry dependency: a trace ID, a span ID, flags and the trace state, which the core parses and formats per W3C Trace Context. Each `Execution` carries its own context, and `.within(&exec)` forwards it, as it forwards the deadline. The parent of an outgoing call is whatever `.within` names, never a span looked up from the thread. A child span opened inside a handler, and passed on explicitly, is a refinement: deferred.

The `Tracer` SPI:
```rust
pub trait Tracer: Send + Sync + 'static {
    fn start(&self, name: &str, kind: SpanKind, parent: Option<&TraceContext>, attrs: &[Attr]) -> Span;
}
```
A `Span` has `context()` and `end(outcome)`.

On by default: propagation. The default tracer passes incoming context through unchanged, so a ulo service doesn't break a trace that crosses it, even with nothing exported. Export is on only once `ulo-otel` is added.

Turning it off: `App::builder().trace(TraceMode::Off)`. Nothing is read or written, and `traceparent` is the ordinary header F400 made it.

`traceparent` and `tracestate` become reserved while tracing is on. A caller setting either with `.header` is refused at the await as `reserved_header`, as `deadline-ms` is. While tracing is off, they're ordinary headers.

What the standard decides. An incoming context that can't be parsed is ignored, and a new trace starts. `tracestate` is discarded when `traceparent` is invalid. Unknown higher versions are parsed by the spec's forward-compatibility rules. `tracestate`'s limits on list members are as the spec sets them. Claude Code checks each rule against the spec text. This is deliberately the opposite of `deadline-ms`: the W3C specification tells a receiver to restart the trace, not to fail the request, and a trace context is advisory where a deadline is a bound.

Carriers. The context travels in HTTP headers; gRPC metadata, written by `outgoing`; the RPC frame's `h` map, on every link (premise to check: every frame kind carries `h`, events included, after F412); the WebSocket handshake, each message's execution having the connection's span as its parent. Events carry the context even though they don't carry a deadline: an event's handler is a consumer of the producer's span, as OpenTelemetry's messaging conventions model it. Writing the context into a broker's own native headers, for consumers that aren't ulo, is deferred.

Spans. Kinds and attributes follow OpenTelemetry's semantic conventions: HTTP `http.request.method`, `http.route`, the response status; RPC `rpc.system`, `rpc.service`, `rpc.method`; brokers the messaging conventions. A server span counts as an error on a 5xx, and not on a 4xx. Database spans are left to each library's own instrumentation.

`ulo-otel` and `ulo-otel-otlp`:
```rust
.import(OtelModule::new()
    .options_from("otel")                 // or OtelOptions::from_env(&env): the standard OTEL_* names, read from a given Env
    .exporter(OtlpExporter::http()))      // http/protobuf, the exporter's default protocol
```
- The exporter. The OTLP exporter's default features are HTTP with protobuf and reqwest's blocking client, and its gRPC transport requires a tokio runtime. So HTTP is the default here, and gRPC export is deferred until it can run on the given runtime.
- The batch processor exports from a dedicated background thread and needs no external runtime. Its `shutdown` blocks, and can deadlock if called on the main thread of a current-thread tokio runtime. So `close` flushes it from a thread of its own, joined under the close bound, after the transports have drained, so their last spans are included.
- Sampling is parent-based by default.
- Premise to check: `opentelemetry_sdk` without its `rt-*` features pulls no tokio into `ulo-otel`'s tree.

Scenarios, on every transport suite, with a recording tracer: (1) one hop links parent to child; (2) an invalid `traceparent` starts a new trace and the request still succeeds; (3) with tracing off, the header passes through untouched; (4) a caller-set `traceparent` is refused while tracing is on; (5) an event's handler span has the emitter's span as its parent.

### 7. Where standards decide

| Standard | What the design takes from it |
| --- | --- |
| Kubernetes probes | The meanings of liveness, readiness and startup; 200–399 as success. |
| `grpc.health.v1` | `Check`, `Watch`, the empty service name, `SERVING` and `NOT_SERVING`. |
| IETF health-check draft, expired | The body's shape and media type only. |
| W3C Trace Context | Header formats, the invalid-header rule, `tracestate`'s limits. |
| OpenTelemetry | Semantic conventions, OTLP over HTTP/protobuf, the `OTEL_*` variable names. |
| Database URLs | libpq's URI, MongoDB's connection-string specification, `redis`/`rediss`. Parsed by each library's own parser. |
| TOML 1.0, JSON (RFC 8259), YAML 1.2 | The file formats. |
| `.env` | No standard exists. The design takes the common subset and says so. |
| `sysexits.h` | Exit codes 78 and 75. A convention, and named as one. |

### 8. Build order

1. The core extensions 1–7. 2. `ulo-config`. 3. `ulo-health` and `ulo-health-http`. 4. `ulo-db`, `ulo-db-sqlx` and `ulo-db-conformance`, together. 5. SeaORM, after SQLx; Diesel, Mongo and Redis after that. 6. `ulo-health-grpc`, any time after step 3. 7. `ulo-cli`'s templates. 8. Observability, last: core extension 8, the carriers in every hub and link, then `ulo-otel` and `ulo-otel-otlp`.

### Premises Claude Code checks first, each returned as an S-item if it fails

1. The core: multi-binding, close order, options resolved from a binding at `wire`, how two bindings of one type are distinguished (F311), and whether the core uses `tracing` today.
2. figment: reports no line (the probe in section 2).
3. JSON: a maintained parser with value spans.
4. YAML: `serde-saphyr`'s errors give locations.
5. `dotenvy`: reads a file without setting the process environment.
6. The HTTP hub: can mark a route `serves_during_drain`.
7. tonic-health: `Watch` conforms to `grpc.health.v1`.
8. Each store: whether its parse errors echo the URL; how its errors divide into unreachable and misconfigured; what it spawns when its client is created; whether SeaORM supports a lazy connection.
9. deadpool: whether it's maintained.
10. Redis: which client `ulo-rpc-redis` and `ulo-ws-redis` use.
11. W3C Trace Context: each rule above, read against the specification's text.
12. OpenTelemetry: `opentelemetry_sdk` pulls no tokio without its `rt-*` features.
13. RPC frames: every frame kind carries `h`.

## Second response: the integration design corrected against the core and transport designs

Received 2026-10-17. The designer was then given `DESIGN.md` and `transports/DESIGN.md`. This response corrects the first against both; anything it does not mention stands as the first response has it. It is a design awaiting review, not a signed-off decision.

### 1. Premises the two designs answer

| My premise or extension | What the designs say | Result |
| --- | --- | --- |
| Collecting several contributions of one kind (requirement 8) | Core §3.2 and §4: `contribute::<T>()` and `Many<T>` exist, collections are app-wide, and the core's own `DbModule` example contributes a health indicator. | No extension. Health reads `Many<dyn Check>`, and each database module contributes. |
| Close order: pools after the transports drain (requirement 15) | Core §9.5: before-shutdown hooks, stop accepting, the drain, then `OnModuleDestroy` in reverse connect order, then sockets. A singleton is built before whatever depends on it, so it is destroyed after it. | No extension. A pool closes in its binding's `on_destroy`. |
| Connect with retries and backoff under a total bound | Core §9.3: `.ready(..).retries(..).backoff(..).attempt_timeout(..).timeout(..)` on a singleton handle, failing as `ConnectError::Readiness`. | The shared policy uses it. `ulo-db` writes no retry loop of its own. |
| Two databases of one kind, separately named (requirement 16) | Core §8.3: `Keyed<Q, M>` requalifies a module's exports as `T @ Q`, so consumers read `Dep<PgPool, Primary>`, and each keyed instance contributes its own health check. | `Db<S, Name>` is dropped. A module provides `Dep<sqlx::PgPool>`, and `.keyed::<Primary>()` names it. |
| Secrets | Core §8.3, §9.3 and §13: `Secret<T>` exists, with `m.secret(&s)` registering it with the graph's redaction. | Used as is, plus one addition (extension E1). |
| An injectable runtime that can be downcast (my extension 4) | Core decision 25: the injected runtime is the core's wrapper, `Runtime` has no `Any` supertrait, and a downcast reaches nothing. | Withdrawn. It contradicts a settled decision. Section 2.4 gives the replacement. |
| Lifecycle as a watched value (my extension 1) | X13 `AppHandle::phase()`, the core's before-shutdown stage, and `fw-transport`'s doc-hidden `Watch`. | Withdrawn. Health turns readiness false in a before-shutdown hook of its own (section 2.2). |
| Drain delay (my extension 5) | Core §9.5: the before-shutdown stage exists for exactly this, while traffic is still normal. | Withdrawn. It becomes health's own `lag` (section 2.2). |
| Health routes served through the drain (my extension 6) | Transports §3.6: once `Execution::open` is refused, the HTTP server answers 503 before routing. | Withdrawn (section 2.2). |
| Exit codes 78 and 75 (my extension 7) | The core maps no exit code, and `main` returns the error. | Deferred. The distinction is carried by type (section 2.4). |
| Execution-scoped providers (requirement 21) | Core §3.3 and §6: `#[injectable(execution)]` and `.execution(..)` exist. | My "deferred" was wrong. What is deferred is narrower (section 2.6). |
| A gRPC health service | Transports §6.2: `fw-grpc` already serves `grpc.health.v1` through tonic-health, with `GrpcHealth::module()`, writing SERVING after bind and NOT_SERVING at the drain. | `ulo-health-grpc` is dropped. `fw-grpc` follows readiness instead (section 2.2). |
| `traceparent` | Transports §5.2: F400 made it an ordinary header, and F404 reserves it "as `deadline-ms` is" once propagation is built in, in this race. | Section 2.5 does that. |
| Scaffolding and `dev` | Transports §9: `ulo new`, `ulo generate` and `ulo dev` exist in `ulo-cli`. | Section 4 of my brief is a change to that crate, not a new crate. |

### 2. Changes, section by section

#### 2.1 Configuration: loaded in `main`, refused at `wire`

Core principle 4 and decision 4 say registration is synchronous, and that `wire()` does no I/O. So configuration cannot load inside `wire`, and the brief's "refused at `wire`" has to be reached another way.

- Loading never fails. `let config = Config::builder().source(..)…load();` runs in `main` and reads every source. A source that cannot be read, or that does not parse, is recorded inside the `Config` value rather than returned.
- Every problem is reported by `wire()`, in one report. `ConfigModule::new(&config)` registers through `m.try_value(..)`, which reports each recorded source failure and every problem in the typed `AppConfig`. An integration reads its section at its own registration: `SqlxModule::<Postgres>::from_config(&config, "database")` deserialises the section there and records a failure through `try_value`. So `wire()` reports source failures, deserialisation failures and the validation hook's findings together with every other wiring error, as `WiringErrors` entries naming the module.
- Blocking file reads happen in `main`, before anything serves. That satisfies the rule against blocking a worker.
- Unread keys: a warning by default, refusal opt-in. At `connect`, `ConfigModule`'s init hook logs at `warn` every key nothing read, with its origin. `.deny_unread()` refuses instead, at `connect`. It can't be a `wire()` refusal: the core has no hook that runs after every module has registered, and a lazily loaded module (core §8.6) may legitimately read a key later.
- Secrets inside a typed configuration need extension E1, because `#[module]` registers only the `Secret` fields of the module's own type.

#### 2.2 Health

- Readiness turns false at `Stopping`. The health module implements `BeforeApplicationShutdown`: it turns readiness false, then waits `.lag(Duration)`, zero by default. This is a `Duration` and not a `Bound`, because zero means no lag rather than a refused value. Health reads `Many<dyn Check>`, so it is constructed after every check and its before-shutdown hook runs among the first. Every refusal comes later, at `Draining`, so requirement 10 holds without any core change.
- "False until `listen` has finished" holds by construction. Nothing answers a request before `serve`, and every server has bound by then. An embedding answers 503 in its `Unbound` state (transports §3.8). The scenario stays, but as a unit test of health's own state, since nothing outside can observe a probe before `serve`.
- During `Draining`, the HTTP server's own 503 answers every probe, liveness included. A pod that is already terminating loses nothing when its liveness probe fails, so serving probes through the drain isn't worth a change to the HTTP transport. That trade-off goes in the design.
- gRPC: `fw-grpc` follows readiness when it is bound. By §3.7's one-writer rule, one component writes the health statuses. With `ulo_health::Readiness` bound, `fw-grpc` stops writing its own statuses at bind and at the drain, and `""` plus every known service mirror readiness: NOT_SERVING from `Stopping` on, and whenever a check is down. Without the health module, §6.2's behaviour is unchanged. `fw-grpc` gains a dependency on `ulo-health`, which is runtime-free and small. Because gRPC's `Check` reads a stored status, readiness is re-evaluated every 5 s by a task on the app's runtime, with a zero refused.
- The trait is `ulo_health::Check`, with a `check` method, following core §0.3's naming rule.

#### 2.3 The connect policy in `ulo-db`

The core's readiness API becomes the connect policy:
- `try_singleton(create the pool without connecting)` registers the pool;
- `.ready(ping).retries(..).backoff(..).timeout(30 s)` is the bounded connect;
- `.on_destroy(close)` is the close;
- the health check is a contribution.

`.lazy()` omits `.ready(..)`, so the start does not wait for the database, and the health check stays down until the first ping succeeds.

Two gaps remain:
- Backoff is constant, since that is what `.backoff(Duration)` offers. Exponential backoff is a deferred refinement.
- Wrong credentials are retried until the bound, because a readiness check cannot stop its own retries. Extension E4 adds a way.

Migrations run in the module's `on_init`, under `ulo-db`'s lock (unchanged from the design). A failed migration carries `ulo_db::MigrationFailed`.

#### 2.4 Runtimes: run the work on the given runtime, and refuse at `connect`

This replaces the withdrawn downcast.

`connect()` is awaited in `main`, and `main` need not run on the app's runtime. So `ulo-db` runs every library call that might look up a runtime as a task on the given runtime, through the core's own `fw::spawn_with(&Dep<dyn Runtime>, ..)`: creating the pool, the readiness ping, the migrations and the close.

The first of these tasks also checks whether the runtime it is running on is tokio's, with `tokio::runtime::Handle::try_current()` inside the task. That finds no runtime by ambient lookup: it reads the runtime the task was given to run on. Running the work on the application's runtime is exactly the "entered through the application's runtime" the brief allows.
- A smol app is refused at `connect` with `ulo_db::UnsupportedRuntime`, naming the module.
- A handler's own queries run on the app's workers, so they have tokio's context.

The refusal is at `connect`, not `wire`, because `wire` builds nothing and cannot run a task. The error is permanent and says so by its type. The same holds for the other permanent failures discovered at `connect` (bad credentials under E4, a failed migration): a `StartupError::Connect` whose cause downcasts to a `ulo-db` type marked permanent.

Smol variants of every store are deferred, with this reason: by reading, SQLx's and redis-rs's smol support spawns on smol's global executor, which is not the app's runtime. That is a premise to confirm.

#### 2.5 Observability: built on the spans that already exist

`fw_transport::span::call` already opens a `tracing` span per call, following OpenTelemetry's semantic conventions (transports §2.9). So `ulo-otel` exports those spans, and the design adds only the propagation:

- The core (E2):
  - a `TraceContext` value, W3C-parsed, runtime-free and with no OpenTelemetry dependency;
  - `Execution::trace()`;
  - forwarding through `.within`'s `Spec`;
  - `AppBuilder::trace(TraceMode)`, where `On` is the default and `Off` turns propagation off.
- Each transport extracts the trace context into the execution it opens, from:
  - an HTTP request's headers;
  - gRPC metadata;
  - an RPC frame's `h`, which brokers already carry as their native headers (§5.2), so a consumer that isn't ulo sees the context too;
  - a WebSocket connection's handshake headers, each message's execution taking the connection's as its parent.

  It injects the context on calls made `.within` an execution: `outgoing` writes it into gRPC metadata, and `RpcClient` writes it into the frame's headers, events included, since an event carries no deadline but does carry its trace (F408).
- `traceparent` and `tracestate` are reserved while tracing is on. A caller setting either with `.header` is refused `reserved_header` at the await, as `deadline-ms` is (F404, closing F400's deferral). With tracing off they are ordinary headers.
- What is on by default: pass-through. With no exporter, an incoming context is forwarded unchanged. An invalid incoming context starts a new trace, as the W3C specification says.
- With `ulo-otel` (E3): it binds a `dyn TraceSource`, which a transport reads to give the execution its own span's IDs, and it sets the transport span's parent from the execution's context.
- Installation: `ulo_otel::Otel::new(options)` is built in `main`. Its `.layer()` goes into the `tracing` subscriber the application installs; the subscriber is the application's to set up. Its `.module()` flushes the batch processor in `OnApplicationShutdown`, step 7, after the sockets have closed. The flush runs on a thread of its own, joined under the hook's bound.
- Levels: `ulo-otel-otlp` goes in `runtime-levels.txt` as tokio-bound. reqwest's blocking client brings tokio, on a thread of its own, and never touches the app's runtime.

#### 2.6 Deferred: corrected list

- Transactions scoped to an execution. The core has no asynchronous hook at an execution's end, so a commit or rollback cannot run there. That is the reason, and it is a core design of its own.
- Per-test databases. They need the suite's container harness first.
- Execution-scoped providers are not deferred: they exist.

#### 2.7 The database suite reuses what exists

`ulo-db-conformance` takes `Budget`, its `stall` field, `block_on_one_thread`, `startup_failed!`, `report`, `relay::unshadowed`, `reachable` and the `not_applicable` stamping rules from the RPC suite. Where it needs a piece that lives in that suite, §3.7's one-copy rule applies: the shared pieces move into a `publish = false` support crate both suites depend on, rather than being copied.

### 3. Extensions, revised

To the core:
- E1: deserialising a `Secret` registers it. `Secret<T>: Deserialize` behind a `serde` feature. A deserialisation run under `fw::secret::collect(..)` hands back every `Secret` it produced, and `ConfigModule` and each section read register them with `m.secret(..)`. The collector is scoped to one synchronous deserialisation, so a thread-local is sound there.
- E2: trace context on the execution. `TraceContext`, `Execution::trace()`, forwarding through `.within`, and `AppBuilder::trace(TraceMode)`.
- E3: the trace source. `TraceSource`, an SPI a transport reads as `Option<Dep<dyn TraceSource>>`, and a parent field accepted by `span::call`.
- E4: a readiness check can stop retrying. A check returning an error that downcasts to `fw::Permanent` ends at once, `Errored` with that error. Additive.

To the transport layer:
- `fw-grpc`'s health follows `ulo_health::Readiness` when it is bound;
- trace extraction and injection in every hub and in `outgoing`;
- `RpcClient` reserves the two trace headers while tracing is on.

Withdrawn: extensions 1, 4, 5, 6 and 7 from the first response's design.

### 4. Premises still open, for Claude Code to check

1. Whether figment reports a line, whether a maintained JSON parser gives value spans, whether `serde-saphyr`'s errors give locations, and whether `dotenvy` reads without setting the environment (unchanged).
2. Whether `try_value`'s `WiringErrors` entry can carry a configuration error's full list of problems with their origins, or whether redaction or truncation cuts it down.
3. Whether the core's `Secret` already implements `Deserialize`. If it does, E1 shrinks to the collector.
4. Whether SQLx 0.9's `runtime-smol` and redis-rs's `smol-comp` spawn on smol's global executor (the reason for section 2.4's deferral).
5. Whether SeaORM's `ConnectOptions` can create a connection without connecting, the precondition for its `.lazy()`.
6. Whether a tracing-opentelemetry layer can set a span's parent at creation from a recorded field (E3's mechanism). If not, `span::call` takes the parent directly from the source.
7. Whether `sqlx::Pool::close` waits for a connection still held by an execution the drain abandoned. If it does, the destroy hook's bound contains the wait, and the design says so.
8. The remaining store premises (lock behaviour per migrator, parse errors echoing the URL, deadpool's maintenance, the Redis client the existing crates use), unchanged.

## Third response: answers to QUESTIONS_1

Received 2026-10-17, answering `QUESTIONS_1.md`. Answers Q1 to Q10 and the review's corrections. The user signed it off the same day; the build waits until the transport batches are done. Questions answered separately by the designer are recorded when they arrive.

### Q1: drop the probe. `Spawn` gains `fn name(&self) -> &'static str`, answering `"unknown"` by default; the core's wrapper forwards it, and a store refuses any name but `"tokio"`

Capturing `Tokio::try_current()` where the module is built, the link pattern, doesn't answer the question. A link runs all its own I/O on its own runtime through `Tokio::run`, so the runtime the link holds is the only one its futures touch. A database client is called directly by the user's handler code, on whatever runtime the app was given. A store module holding a tokio handle of its own changes nothing about where those queries are polled: on a smol app they still panic on smol's workers. The question to answer is therefore "what runtime is this app on?", and only the given runtime can answer it.

The name is a method, not `Link::NAME`'s associated constant, because `Runtime` has to stay dyn-compatible. It is a string rather than a closed enum, so a third-party runtime can declare itself. It sits on `Spawn` because `Runtime` is a blanket impl. Its default reads `"unknown"`, and a store refuses `"unknown"` too, so a runtime that declares nothing fails the check rather than loosening it (§3.9).
- `fw_runtime_tokio::Tokio` answers `"tokio"` and `Smol` answers `"smol"`.
- The core's `Clocked` forwards the spawner's name, so `.runtime(r).timer(t)` still answers `r`'s.

The check is the first thing the pool's factory does, reading `Dep<dyn Runtime>`, before anything is built: a runtime-free comparison, no ambient lookup, and a smol app driven from a thread with a tokio context still answers `"smol"`. `spawn_with` stays for creating the pool, the readiness ping, migrations and the close. That isn't a probe: it runs the work on the runtime the app was given, which is how a library that spawns at pool creation spawns on the app's runtime.

### Q2

- (a) Yes: a decision change, recorded as one. With the health module imported, the overall status turns NOT_SERVING at `Stopping`. §6.2's "consistent with `is_draining()`" predates readiness, and its own reason, a client polling health stops sending before the GOAWAY, is served better by the earlier flip. Without the health module, §6.2 is unchanged.
- (b) One writer per service name. `""` belongs to health when readiness is bound, and to the server otherwise. Named services keep today's writers, the server at bind and drain and the application through `GrpcHealth::set_named`. Kubernetes' gRPC probe reads `""` unless told otherwise. That named services have two writers today predates this race: a refinement, deferred, with that reason.
- (c) No dependency from `ulo-grpc` on `ulo-health`. `ulo-health-grpc` comes back as a bridge crate, tokio-bound, depending on both:
  - In its init hook, before `listen()`, it calls a new `GrpcHealth::delegate_overall()`. After that, the server's `bind` and `drain` stop writing `""`. That one method is the whole change to `ulo-grpc`.
  - It then writes `""` from readiness through `Dep<GrpcHealth>`.

  `Readiness` stays in `ulo-health`. Moving it into the core or `ulo-transport` would put an integration's concept below the transports, the same inversion in the other direction.

### Q3: yes, E1 shrinks to a `serde` feature on `ulo`

`Secret<T>: Deserialize` calls `deserialize_newtype_struct("Secret", ..)`. `ulo-config`'s deserializer recognises the name, records the leaf's value and path from its own origin-carrying tree, and deserialises the inner value. Any other deserializer sees a transparent newtype. `ConfigModule` and each section read call `m.secret(..)` for every leaf recorded. There is no thread-local and no `fw::secret::collect`.
- Another crate's newtype also named `Secret` would be recorded too. The cost of that is extra redaction, which is harmless.
- No `Serialize` impl: writing a secret out is the leak redaction exists to prevent.

### Q4: the `Store` trait, restated

```rust
pub trait Store: Send + Sync + 'static {
    const NAME: &'static str;                                          // "postgres", "mongodb": messages and check names
    type Options: DeserializeOwned + Clone + Eq + Hash + Send + Sync;  // the module's identity is by value (core §8.3)
    type Client: Clone + Send + Sync + 'static;
    fn validate(o: &Self::Options) -> Result<(), ulo_config::Problems>;      // at registration, through `try_value`
    fn create(o: &Self::Options) -> BoxFuture<'static, Result<Self::Client, BoxError>>;   // without connecting; run through `spawn_with`
    fn ping(c: &Self::Client) -> BoxFuture<'static, Result<(), BoxError>>;   // the `.ready` attempt and the health check
    fn close(c: Self::Client) -> BoxFuture<'static, ()>;
}
```

There is no `Given` parameter, and no error type of the store's own, so no clash with `ulo::ConnectError` or `ulo_rpc::Misconfigured`.

E4, in the core:
- `#[non_exhaustive] pub struct Permanent(BoxError)`, built with `Permanent::new(e)`. Its `Display` is the inner error's, and `source()` answers the inner error.
- A readiness attempt whose error downcasts to `Permanent` ends the check at once, as `ConnectError::Readiness { reason: Errored(..) }`.
- `fw::is_permanent(&StartupError) -> bool` walks the chain as `is_panic` does, so `main` can tell a restart that cannot help from one that might.

A store marks an error permanent by returning `Err(Permanent::new(e).into())` from `ping`. Which of each library's errors count as permanent, bad credentials or an unknown database, is a premise for each store.

In `ulo-db`, both wrapped in `Permanent` where they are raised:
- `UnsupportedRuntime { module: &'static str, required: &'static str, given: &'static str }`, from the factory;
- `MigrationFailed { module: &'static str, source: BoxError }`, from the init hook.

### Q5

- A standalone `execute`: `ExecOptions::trace(TraceContext)` seeds the inbound context. Without it, a standalone execution starts a new trace when an exporter is bound, and carries none in pass-through.
- A client outside an app has no mode of its own. It writes whatever trace context the execution named by `.within` carries. A call with no `.within` carries none, and so does an execution of an app with `TraceMode::Off`. The reservation follows the same rule, per call: `traceparent` is refused `reserved_header` exactly when the client writes it itself, and is an ordinary header otherwise. The client needs no mode, and a client outside any app behaves the same way.
- `tracestate` is not reserved. A caller's `tracestate` is merged as W3C §3.3 describes: the caller's entries first, then the execution's, duplicates by key dropped, and the list cut to the specification's limits. That's how a vendor adds its own entry.

### Q6

- `.lag(d)` bounds its own hook. Health's before-shutdown hook takes `TIMEOUT = After(d + 1 s)`, so a lag isn't cut by the 10 s default. A lag reaching past `shutdown_timeout` is cut by the cap and recorded `TimedOut { limit: ShutdownCap }`. The builder's cap is invisible at registration, so the documentation says this rather than a refusal.
- Ordering: corrected to "before every `Check`'s hooks", with lazily loaded modules' hooks running ahead of it, by the exact reverse of connect order.
- The kubelet facts confirm 2.2 and add one line.
  - They settle the earlier liveness concern: liveness and startup probes stop at termination, so a liveness probe failing during the drain can't kill a pod.
  - Since Kubernetes marks a pod's endpoints terminating at the same time as SIGTERM, the readiness flip matters for other load balancers and for readers of readiness. Inside Kubernetes, `lag` is the `preStop` sleep's equivalent: the time kube-proxy and the load balancers need to see the endpoint change before the app refuses work.
  - The default stays zero, and the documentation recommends a few seconds in Kubernetes.

### Q7: the interval is a `Bound`

`Default` is 5 s. `Unbounded` means statuses change only at lifecycle transitions, which is meaningful and so is the opt-out. A zero is refused, as `ping_interval`'s is. Under the vocabulary, a `Duration` is a time that is never off, and a zero `Duration` here would mean evaluating continuously.

### Q8: a stated limit, with an explicit opt-out. No core request

A lazily loaded store module contributing a `Check` isn't silently invisible to readiness: `load` refuses it with `LoadRefusal::Contribution`. Store modules gain `.without_health_check()`, which makes the module loadable lazily at the cost of a readiness that cannot see it, chosen by the application where it is written. Lifting the core's rule is a design for the core, not for this race.

### Q9: confirmed

The flush runs on a thread of its own. The thread signals its end through a `futures-channel` oneshot, and the hook awaits that oneshot under its bound. On expiry the thread is left running, detached, and the hook is recorded `TimedOut`. It's the same shape as Kafka's blocking helper, and no worker is ever blocked.

### Q10: confirmed, with the level names corrected

| Crate | Level |
| --- | --- |
| every `ulo-db-*` store crate | tokio-bound, smol deferred |
| `ulo-db-conformance` | shaped like `ulo-rpc-conformance`: runtime-free at default features, with a `tokio` harness feature, `publish = false` |
| `ulo-otel-otlp` | tokio-bound |
| `ulo-health-grpc` | tokio-bound |
| `ulo-config`, `ulo-health`, `ulo-health-http`, `ulo-db`, `ulo-otel` | runtime-free, `ulo-otel`'s subject to its premise |

"`runtime-levels.txt` needs this level named" is withdrawn: tokio-bound is already the fourth name.

### The review's corrections

All four facts, and the staging, are accepted:
- Destroy before sockets. `pool.close()` can run under an abandoned execution's query. The `sqlx::Pool::close` probe decides whether the destroy hook's bound is what contains the wait.
- F404, not F400, is the deferral this race closes.
- Only `Req`, `Evt` and `Open` carry `h`. That covers propagation on the request side, which is all propagation needs.
- Configuration errors' `Display` carries every problem, one per line with its path and origin, since `Redacted` flattens `source()` into one string. A failing source yields one `ValueFailed` per module that read a section from it, each listing that section's problems.
- Staging:
  1. configuration and `ulo-health` with HTTP;
  2. `ulo-db` over `.ready`, with SQLx and the suite;
  3. the other stores;
  4. the CLI's templates;
  5. trace propagation and `ulo-otel`.

  The probes come first, as proposed, with two added to the list: whether SeaORM can create a connection without connecting, and whether a tracing-opentelemetry layer can set a span's parent at creation from a recorded field.

## Fourth response: answers to QUESTIONS_2

Received 2026-10-18, answering `QUESTIONS_2.md` (the probe results and the design's open holes). The user signed it off 2026-10-19.

### What the probes overturned

**P1: confirmed.** `ulo-db-sqlx` and `ulo-db-seaorm` state 1.94, as `ulo-graphql-async-graphql` and `ulo-http-salvo` state theirs. `ulo-db` stays at 1.88 and names no lock primitive (H3). The SQLite sidecar lock, `File::lock`, stable since 1.89, belongs to the store crates that speak SQLite, which sit at 1.94 anyway, or at Diesel's own floor where higher.

**P2: confirmed, with one addition to the core's report.** A second SQLx is a different `TypeId`, so it fails at `wire()` as a missing binding. Both `PgPool`s have the same path, so the report as it stands would print two identical names and leave the reader guessing. The core's §10.1 step 3 already names both spellings when a missing key matches a bound one up to `+ Send + Sync`. Extend it: when a missing key's full type name equals a bound key's exactly and the `TypeId`s differ, the help reads "the same type from two versions of its crate; `cargo tree -d` lists them". The CI `cargo tree -d` job guards the workspace, and the report is what guards a user's application.

**P3: the rule, confirmed. The SeaORM store parses with SQLx's own option parser**, which SeaORM's SQL drivers use underneath, so no parser is added, and the scheme check is shared with `ulo-db-sqlx`. Three measures together:
1. Registration. The options hold the URL as a `Secret<String>`, and the module registers both the whole URL and its password component with `m.secret(..)`. A library error that prints the URL, or the password alone, is then scrubbed wherever the core stores it, so SeaORM's `connect` errors need no special case.
2. `Debug`. The options type's `Debug` writes the URL redacted, and no library's options value is ever formatted.
3. The scheme. `validate` checks it itself, so `notascheme://` is refused there.

**P4: `saphyr`'s `MarkedYaml` into the origin tree, behind the `yaml` feature, which ships after TOML and JSON.**
- Anchors and aliases are supported once a probe shows the expansion is bounded. Without a bound, an alias bomb is a memory exhaustion at startup. If `saphyr` has no bound, the conversion into the tree enforces one, and passing it is a problem naming the line.
- Merge keys (`<<`) are YAML 1.1, refused as a problem naming the line, not silently merged.
- Scalars read by the YAML 1.2 core schema. `yes` given to a `bool` is a type problem, which reports itself.

**P5: confirmed: our own parser.**

| Form | Rule |
| --- | --- |
| a line | `KEY=VALUE`, with an optional `export ` prefix |
| single quotes | literal |
| double quotes | `\n`, `\"` and `\\` as escapes |
| comments | `#` at the start of a line, or after whitespace in an unquoted value |
| expansion | none |
| multi-line values | none |

Every problem carries a line number. A key appearing twice in one file is a problem naming both lines, since within a single file that is almost always a mistake. Layering between files is unaffected.

**P6: both measures together.**
- The collector. `Secret::deserialize` records its value into a collector `ulo-config` installs around each deserialisation. The deserialisation is synchronous, so a scoped thread-local is sound for the length of the call, and the collector catches `flatten` and `untagged`, where serde's buffered content loses the newtype's name. The name-based path keeps giving origins where it works; the collector exists for completeness of registration.
- Problem text. A problem under a secret's path renders from its path and kind alone, never the value found.

With both, no secret goes unregistered, and none appears in a problem.

**P7: accepted as a stated limit.** serde stops a struct's pass at the first error raised inside a leaf type's own `Deserialize`, and nothing outside serde can resume it. Each section deserialises on its own, so other sections still report their problems. The design states it: at most one leaf-type error per section, reported with its path and origin.

**P8: ship `ureq` by default, with `reqwest-blocking` as an opt-in feature.** `runtime-levels.txt` then reads `ulo-otel-otlp` as runtime-free at default features, plus a tokio-bound entry with `--features reqwest-blocking`. A crate that can be tokio-free should be.

Premise to check: `ureq`'s rustls client is built with an explicit provider. If it falls back to rustls's own resolution, it hits F422's ambiguity when two providers are linked. If it can't be built with an explicit provider, the adapter refuses at construction with S7's message.

**P9: document the deviation and file upstream.** A `Health` service of the bridge's own would be a second implementation of the protocol beside tonic-health's, which §3.7's one-copy rule argues against for a case no shipped client hits: Kubernetes probes use `Check`, not `Watch`.
- The NOT_FOUND on an unregistered service's `Watch` is documented as tonic-health's behaviour.
- Duplicate messages are prevented on our side: the bridge writes only on a change, and so do `ulo-grpc`'s own writes at `bind` and `drain`, which compare with the stored status first.

**P10: reworded.** Kubernetes marks a pod's endpoints terminating at its deletion timestamp, before `preStop` and before SIGTERM. So the readiness flip is not what removes the endpoint there. What the application must allow for is the time kube-proxy and the load balancers take to see the change. A `preStop` sleep gives that time if the pod has one, and `.lag` gives it if not, so the guidance becomes: with a `preStop` sleep, leave `lag` at zero; without one, set it to a few seconds. The default stays zero.

### Open design holes

**H1: Redis builds once with no library retry, and `ulo-db` owns the retrying for stores that cannot build without connecting.** `ConnectionManager` cannot be built without connecting, so with no change, a Redis that is unreachable at startup fails at construction with no retry, which breaks requirement 14.
- `Store` gains `const CREATE_CONNECTS: bool`.
- For such a store, `ulo-db` runs `create` under the same retries, backoff and total bound it hands to `.ready(..)`. The loop is written once, in `ulo-db`, and stops on `Permanent` exactly as `.ready` does under E4.
- `.ready` then only pings.
- Redis's `create` sets `set_number_of_retries(0)`, so the library doesn't retry inside our loop. That is what made a wrong password take 9.5 s.

Diesel's `.lazy()`: the earlier table was wrong. deadpool's `build` connects nothing, so Diesel offers `.lazy()`.

**H2: no change to `Runtime`. `ulo-db-diesel`'s blocking path is tokio's own, reached from a task on the given runtime.**
- Migrations: the module has already checked that `Runtime::name()` is `"tokio"`. It then runs `spawn_with(&runtime, async { tokio::task::spawn_blocking(migrate).await })`. That's no ambient lookup: the task runs on the given runtime, which is verified as tokio's, and its `spawn_blocking` lands on that runtime's blocking pool.
- The SQLite wrapper keeps diesel-async's default `SpawnBlocking`, tokio's, through `custom_setup`. Its queries run in user handlers on the app's tokio workers, which the name check guarantees.

Decision 133's condition, a runtime-free or runtime-neutral crate needing blocking work, still isn't met: `ulo-db-diesel` is tokio-bound.

**H3: confirmed. `ulo-db` owns the protocol, and each store supplies the connection and the primitive.**
- `ulo-db`: the key, the order, the bound and the error.
- Each store: the dedicated connection and the lock primitive.

| Store | Primitive |
| --- | --- |
| Postgres | `pg_advisory_lock(K)` on a connection held for the migration. Advisory locks are scoped to the current database, so one constant serves every database. |
| MySQL | `GET_LOCK('ulo-db.migrate.' + database, timeout)`. A name longer than 64 characters takes a hash of the database name in place of the name. |
| SQLite | an OS lock on `<file>.ulo-migrate.lock` |

`K` is the first eight bytes of the SHA-256 of `"ulo-db/migrations"`, read as a big-endian `i64` and written into the design as the literal. It differs from SQLx's own migration key, so the two nest, ours outside, and never collide.

**H4:**
- An invalid incoming `traceparent` in pass-through gives no context. Only a participant can originate IDs, and in pass-through nothing does. The next hop that participates starts a trace.
- `tracestate` is merged only when the context is originated or modified here, that is, when a `TraceSource` is bound. In pass-through, both headers are forwarded exactly as received, as §3.4 requires, and a caller's `tracestate` set with `.header` on a call that forwards is refused `reserved_header`, its message naming that adding a vendor entry needs an exporter.
- Level 1. When a context originates here, only the sampled flag is written. In pass-through the flags are forwarded unchanged, `random` (0x02) included, since forwarding unchanged is the rule there. Level 2 waits until it is a Recommendation.
- The correction is accepted: §3.5 carries the mutation rules.

**H5: confirmed. `TraceSource` is a core hook, called inside `span::call` between creating the span and first entering it.** One addition the order needs: each transport extracts the incoming context before `Execution::open` and passes it in `ExecOptions::trace`. `span::call` then calls `TraceSource::start(&span, inbound)`, which answers the execution's own context, and that is written through `Execution::set_trace`. Like `route_to`, it can be called once, by the holder only, before `dispatch`. With no source bound, the inbound context stays as it is. `ulo-otel` implements the hook with `OpenTelemetrySpanExt::set_parent`.

**H6:**
- Mongo: `ulo-db-mongodb`, as on disk. The convention is `ulo-<role>-<library>` and the library is `mongodb`, so the design text is what changes.
- MySQL: `ulo-db-sqlx`'s TLS feature is on by default and uses aws-lc-rs, the provider the rest of the workspace uses, so a second provider isn't linked into the F422 ambiguity. `mysql-rsa` isn't enabled: with TLS, caching_sha2's full authentication works without RSA key exchange. Premise to check: which SQLx 0.9 feature gives aws-lc-rs with the platform's roots. If none does, use aws-lc-rs with webpki roots, and document it.
- `mongodb+srv`: `validate` checks the scheme and a host and does no lookup. The SRV lookup happens in `create`, on the app's runtime, through `spawn_with`.
- smol features in an application's tree:
  - SQLx with both runtime features uses tokio wherever a tokio context exists, and the name check guarantees one on every path that runs a query. So `runtime-smol` enabled elsewhere is harmless. A test in the workspace builds with it enabled to show that.
  - `ulo-db-redis` calls `prefer_tokio()` once, when the module is built. It is process-wide, but it only decides which runtime redis-rs uses when both of its runtime features are compiled in, and an app already verified to be on tokio needs tokio. The documentation says so, and a test builds with `smol-comp` enabled beside `tokio-comp`.

## Fifth response: answers to QUESTIONS_3

Received 2026-10-19, answering `QUESTIONS_3.md` (rulings that did not hold as applied, and the signatures the spine freezes). The user signed it off the same day.

### Rulings that do not hold as written

**R1: the ring feature, with the platform's roots.** The H6 premise was wrong: ring is the workspace's provider. ring with platform roots adds no second provider and keeps every TLS stack in the workspace on one trust source. aws-lc-rs with webpki roots would bring both the second provider F422 is about and a bundled trust store that diverges from every other link's.

One consequence to record with it: rumqttc already brings aws-lc-rs, so an application linking MQTT has both providers today, whichever SQLx feature is chosen. That is MQTT's limit, not a reason to add a third case.

**R2: pre-scan saphyr's event stream, the parser layer under its loader, and bound the expansion before anything loads. If the event API can't support the count, refuse aliases.** Anchors are common in hand-written YAML configuration, so refusing all aliases outright costs real users.

The scan counts the nodes inside each anchored node as its events arrive, adds that count at every alias, and refuses once the total passes 100,000 nodes, with a problem naming the alias's line. That's far beyond any configuration file and far below a memory problem. It's a fixed limit with no setting, as the 1 MiB deadline buffer is.

Premise to check: whether the events carry the anchor IDs and positions the count needs. If they don't, the fallback is refusing every alias, with a problem naming the line, the same shape as merge keys. YAML ships last of the formats either way.

**R3: `ulo-config`'s error type renders the kind of the value found, never the value, for every type and value problem. The one remaining case is stated at the type.** serde's `ContentDeserializer`, the buffered content behind `flatten` and `untagged`, builds its errors through the deserializer's own error type, through `invalid_type` and `invalid_value`. `ulo-config` owns that type, so it decides what those problems print: "expected an integer, found a string", never the string. That covers buffered content too, where no secret path is known.

What it cannot cover is a leaf type's own `Error::custom` message, which might quote its input. So the restriction is stated on `Secret`'s `Deserialize` documentation and in `ulo-config`'s: a `Secret` under `flatten` or `untagged` is registered, through the collector, but a leaf type's own error message inside it is printed as that type wrote it. A second pass over the collected values can't help: a value that failed was never collected.

**R4: changed. Supply a `SpawnBlocking` that enters the given runtime, as the probe recommends.** The name check proves the app's runtime is tokio's. It doesn't prove the thread polling a query has tokio's context, and a hidden fallback runtime is a runtime found rather than given.
- `create` has already checked the name. It captures the given runtime's handle once, from a task on that runtime, `spawn_with(&rt, async { Handle::current() })`, which reads the runtime the task was given and finds nothing ambiently.
- Each connection is built with `with_runtime(conn, Given(handle.clone()))`.
- `Given::spawn_blocking` calls `handle.spawn_blocking(..)`.

Diesel's migration uses the same `Given` (H2).

**R5: accepted. The stage-2 test decides.** If a `PoolConnection` dropped off tokio's context still spawns on smol's global executor, that comes back as an S-item with the measured behaviour, and H6's ruling is revised then. The test builds with `runtime-smol` enabled beside `runtime-tokio`, drops a connection on a thread with no tokio context, and asserts no task ran on smol's executor.

### Open signatures the spine freezes

**S1: one bound, and one retry loop per store, not two in series.**
- A store whose `create` doesn't connect: `.ready(..)` is the policy, as designed. Its retries and backoff, and `.timeout(connect_within)`.
- A store whose `create` connects: the `create` loop is the policy, under `connect_within`, stopping on `Permanent`. `.ready(..)` then runs one confirming ping, `.retries(0)` with the default attempt bound. The construction bound is set to `connect_within` through `.timeout(..)` on the binding handle, so `construct_timeout` can't cut the loop short.

Passing the remaining time from the loop to `.ready` isn't possible anyway: `.ready`'s bound is written at registration, as a fixed value. This shape needs none of that: each store has exactly one loop, so the one-copy rule holds per store, and the total bound is one number.

**S2: `CREATE_CONNECTS` is true for the whole Mongo store.** It stays a `const`. For a plain `mongodb://` URL, `create` can't fail transiently, so the loop's first attempt succeeds and it costs nothing. For `+srv`, a DNS failure is retried under `connect_within`, which is what requirement 14 asks.

**S3: call it once per process, under a guard of our own, and act on what redis-rs reports.**
- The guard. A process-wide `OnceLock` in `ulo-db-redis`, so two Redis modules never conflict with each other.
- If redis-rs refuses because another crate set a preference first: read the current preference, if redis-rs exposes a way to. Tokio means continue. Anything else is a refusal at `connect`, `Permanent`, naming the conflict.
- If there's no way to read it: log a `warn` naming the conflict and continue. A non-tokio preference then fails loudly at the first query, and the `warn` says why. Refusing a harmless double setting we can't distinguish from a harmful one would break working applications.

Premise to check: whether redis-rs exposes the preference.

**S4: confirmed.** `GrpcHealth` is the only path to the reporter. It doesn't expose `HealthReporter`, and every write goes through its stored copy: the server's, the bridge's and the application's `set_named`. The copy is the truth per name, and a write compares against it before reaching the reporter.

**S5:**
- Raw text: confirmed. `TraceContext` keeps both headers as received, with the parsed fields beside them, and a forward writes the raw text.
- Origin: accepted. A `TraceOrigin` marker, `Received` or `Originated`, inside `TraceContext`. `ExecOptions::trace` sets `Received`, `set_trace` from a `TraceSource` sets `Originated`, and the client's `tracestate` rule reads it.
- Resolution: accepted. `App` resolves `Option<Dep<dyn TraceSource>>` once at `connect`, after the singletons are built. `AppHandle::trace_source() -> Option<&Arc<dyn TraceSource>>` is a plain accessor, read as `timer()` is.
- A standalone execution: in `execute`, and only when the caller hands it a span. `ExecOptions::span(tracing::Span)` makes `execute` call `TraceSource::start(&span, inbound)` and record the result through `set_trace`, so a background job is traced when its author asks for it. Without a span, the execution carries its inbound context as `Received`, or none. `Execution::open` stays untouched, since transports already call the hook through `span::call`.

### For the record

- N1: accepted. 1.89 on `ulo-db-diesel` itself, for `File::lock`. The earlier "Diesel's own floor" was wrong, since Diesel's floors are lower.
- N2: accepted, including the exact pin. A setter outside ureq's semver guarantee is what an exact pin is for, and bumping it is a deliberate change with its own test.
- N3: accepted. `K = 3490428366386280370`. Not being a multiple of SQLx's multiplier is the right argument: no database name can make the two keys meet.

---

# Sixth response (to QUESTIONS_4, first part)

Q1: accepted. `Defaults::of::<T>()` takes `T: Default + Serialize` and reads `T::default()` into the tree through serde, so a default carries the origin "default". Limit: `Secret` has no `Serialize`, by decision, so a section type holding a `Secret` cannot use `Defaults::of`; it uses serde's `#[serde(default)]`. The documentation of `Defaults` says so.

Q2: accepted, but declared rather than ignored (later withdrawn, see the seventh response).

Q3: agreed. Running out of time waiting for another replica's lock is retryable: another replica is migrating, not a wrong configuration. `is_permanent` answers false; the wait is bounded by `connect_within`. A migration that itself fails stays `Permanent`. Correct the docs on `run_locked` and `MigrationFailed`.

Q4: accepted. Add `ModuleDef::qualifier`. The check name becomes `S::NAME @ Qualifier`, printed short as reports print types, and plain `S::NAME` when unkeyed.

Q5: drop `.lazy()` for Mongo (superseded by the seventh response).

Q6: refuse, for parity. A caller-set `traceparent`, or a caller-set `tracestate` on a `Received` context, is refused by `outgoing` as `INVALID_ARGUMENT` with the reason `reserved_header`, in RPC's wording. `trace::inject` keeps its frozen signature. An `Originated` context still merges the caller's `tracestate`. A test per refusal on the gRPC side.

Q7: accepted. Only the variables OpenTelemetry's specification defines, stated at the module. Where ulo passes a value explicitly, that value wins over the environment's.

Q8: accepted. A `warn` when the feature is on and the call returns its error, and nothing otherwise. Correct S3's ruling text.

Q9: the refusal is confirmed (a written wait needs a `Timer`, §3.9).

Q10: add the TLS recipe, MySQL first, Mongo after. Until it lands, each store's documentation marks its TLS path unexercised.

Q11: if the test fails, document it. Refusing is not possible: SQLx exposes no way to see `runtime-smol`. Document the case, the measured behaviour and the remedy (drop the connection on a tokio thread).

Q12: refused, not treated as the default. `Bound::After(ZERO)` on `roots_timeout` is refused by the zero rule: on a client in `usable`, on a server in `prepare`.

Q13: confirmed. `Bound::Default` there is the link's own 30 s.

Q14, Q15: see transports/RESPONSE.md, the sixty-seventh response.

On the record: measure the Mongo stall in release; record the number in the store's documentation. SQLx's lock-removal break staying green is acceptable, and the scenario's comment says so (the build later showed the ordering break is also green on sqlx; see the build log). `ulo-db-prisma` is not built.

# Seventh response (to QUESTIONS_4, the laziness correction)

Mongo: (b). `Store`'s `const CREATE_CONNECTS: bool` becomes `fn create_connects(o: &Self::Options) -> bool`, decided by the options. `mongodb://` is false: lazy again, offers `.lazy()`. `mongodb+srv://` is true: the retry loop under `connect_within`, no `.lazy()`. `.lazy()` on a `+srv` module is refused at registration through `try_value`, naming the scheme. S2's ruling is revised: it answered store-wide for a case only one scheme has. Other stores answer from their own nature.

Redis: `new_lazy_with_config`, after the probe. With `create_connects` false, `.ready(..)` is Redis's retry policy. Redis offers `.lazy()`. What decides `set_number_of_retries` is what it governs: first connection only, keep 0; also reconnects while serving, keep nonzero. The probe answers (first command connects, wrong password's time to fail at each setting, a server restarted mid-service recovering at each setting). Redis's authentication error is `Permanent` in `ping`.

SeaORM on SQLite: accepted. Build the pool through `ulo-db-sqlx`'s lazy pool, wrapped by `SqlxSqliteConnector::from_sqlx_sqlite_pool`; map the SeaORM pool options the store exposes onto SQLx's and document which. SeaORM's SQLite `ping` checks `sqlite_version()` against 3.35 and refuses an older version as `Permanent` naming `RETURNING`. The `not_applicable` declaration is withdrawn: `lazy_readiness_waits_for_the_first_ping` runs on SeaORM's SQLite, and a planted eager `create` must fail it.
