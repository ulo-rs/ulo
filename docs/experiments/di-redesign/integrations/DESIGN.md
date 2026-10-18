The integration-layer design, written to sit beside `DESIGN.md` and `transports/DESIGN.md`: the same numbering style, with brackets referring to the capability list in `integrations/CAPABILITIES.md`. Crate and path names are the repository's: where the two settled designs write `fw`, this one writes `ulo`, and every file named is a path under `crates/` at `58e7637b`. Eleven SPI extensions are named and numbered **I1–I11**, a series of their own beside the core's and the transports' X1–X39, each at the point where a requirement forces it. Section 9 collects them, section 10 is the refusal table, section 11 lists the decisions, section 12 what is deferred, section 13 the premises with the verdicts `PROBES_config.md`, `PROBES_db.md` and `PROBES_obs.md` return, and section 14 what each area resolves in its own log. A choice that rests on a probe carries its verdict where it is stated. "DESIGN §n" is the core design, "transports §n" the transport design, "handoff §n" `DESIGNER_HANDOFF.md`.

---

# Integration layer: design

## 0. Principles

1. **A runtime is given, never found.** An integration runs every library call that may look a runtime up as a task on the runtime the app was given, through `ulo::spawn_with` over `Dep<dyn Runtime>`, so whatever the library spawns at creation runs on that runtime. Whether the app's runtime is one a library supports is answered by the given runtime itself, through `Spawn::name` (I1), and never by an ambient lookup such as `tokio::runtime::Handle::try_current()`. A module holding a runtime of its own, the links' pattern (transports X33), does not apply: a database client is called by the user's handler code on the app's workers, so the runtime that matters is the app's.
2. **A misconfiguration is refused at the first phase that can see it, and says it is permanent.** What registration can see, a value that does not parse, a URL its library refuses, a source that failed to load, is refused by `wire()` through `try_value`, with every other wiring error. What only `connect` can see, an unsupported runtime, bad credentials, a failed migration, fails `connect` with an error wrapped in `ulo::Permanent` (I2), which `ulo::is_permanent` finds. An unreachable dependency is the one retryable start failure, and carries no `Permanent`.
3. **Thin over the library.** A store module provides the library's own pool or client as a binding, and the application calls the library's own API on it. No query layer, no wrapper type around the client, no re-implementation of a parser the library has: URLs are parsed by each library's own parser [12], except where that parser echoes a secret (§4.2).
4. **One copy of a policy.** Connecting, retrying, closing, the health check and the migration lock are written once in `ulo-db` over the core's readiness API (DESIGN §9.3); a store supplies how to create, ping and close its client (handoff §3.7). A library's own retry loop is turned off where it has one. The database suite reuses the RPC suite's harness pieces by moving them into one shared crate, not by copying them (I11).
5. **Standards decide.** Kubernetes decides the probes' meanings, `grpc.health.v1` the gRPC health service, W3C Trace Context the trace headers, OpenTelemetry the span attributes and the export protocol, each library's URL specification the URLs, and TOML 1.0, RFC 8259 and YAML 1.2 the files. Where a candidate standard never finished, the expired IETF health-check draft, it is a shape followed and named as one; where none exists, `.env`, the design takes the common subset and says so (§7).

---

## 1. Crate layout

The four level names are `runtime-levels.txt`'s: *runtime-free*, *runtime-neutral*, *runtime-neutral+http2* and *tokio-bound* (transports §8). No crate of this layer sits at either runtime-neutral level.

| Crate | Contents | Level | Meets the core and transports through |
| --- | --- | --- | --- |
| `ulo-config` | `Config` and its builder, the sources (`Defaults`, `File`, `DotEnv`, `Env`, `Values`), the origin-carrying value tree, `Problems`, `ConfigModule<T>`, the section read every integration's `from_config` calls, and the deserializer that records `Secret` leaves (§2) | runtime-free: serde and parsers only, every file read in `main` | `ModuleDef::try_value`, `ModuleDef::secret`, an init hook (unread keys) |
| `ulo-health` | `Check`, `Probe`, `Health`, `Readiness`, the bounded concurrent run, the health module with its before-shutdown hook and `.lag` (§3) | runtime-free: its bounds run on the core's `Timer` | `Many<dyn Check>`, `BeforeApplicationShutdown`, `Dep<dyn Timer>` |
| `ulo-health-http` | `HealthHttp`: the three probe routes and the response body | runtime-free, as `ulo-http` is | `ulo-http`'s routes |
| `ulo-health-grpc` | The bridge writing the overall `grpc.health.v1` status `""` from `Readiness`, and the re-evaluation task | tokio-bound, as `ulo-grpc` is | `ulo_grpc::GrpcHealth` (I4), `Dep<dyn Runtime>` |
| `ulo-db` | `Store`, `StoreModule<S>`, the connect, retry, close and health policy over `.ready`, the migration lock protocol, `UnsupportedRuntime`, `MigrationFailed` (§4) | runtime-free: the policy is written over the core's `Runtime` and `Timer`, and names no driver | `try_singleton`, `.ready`, `.on_destroy`, `contribute::<dyn Check>()`, `spawn_with`, `Keyed`, I1, I2 |
| `ulo-db-sqlx` | `Store` for Postgres, MySQL and SQLite over SQLx 0.9 with `runtime-tokio` alone; re-exports `sqlx` | tokio-bound | `Store` |
| `ulo-db-seaorm` | `Store` for SeaORM 2.0's `DatabaseConnection` | tokio-bound | `Store` |
| `ulo-db-diesel` | `Store` for a deadpool `Pool` of diesel-async connections | tokio-bound | `Store` |
| `ulo-db-mongo` | `Store` for `mongodb::Client` | tokio-bound: the driver supports tokio alone in 3.x | `Store` |
| `ulo-db-redis` | `Store` for redis-rs's `ConnectionManager`, `tokio-comp` alone | tokio-bound | `Store` |
| `ulo-db-conformance` | The database suite, shaped like `ulo-rpc-conformance`: a host trait per store, the scenarios of §4.9, one macro stamping them per store; `publish = false` | runtime-free at default features, with a `tokio` harness feature | `Store`, the shared suite crate (I11) |
| `ulo-otel` | The `tracing` layer exporting the transports' spans over `tracing-opentelemetry`, the `TraceSource` implementation (I7), sampling, the batch processor, the flush module (§6) | runtime-free: `opentelemetry_sdk` with `trace` alone and `tracing-opentelemetry` 0.34.0 bring no tokio (PROBES_obs.md §2, §3a: holds) | `TraceSource`, `OnApplicationShutdown` |
| `ulo-otel-otlp` | The OTLP exporter over HTTP with protobuf, on reqwest's blocking client | tokio-bound: reqwest's blocking client brings tokio, runs it on a thread of its own and never touches the app's (PROBES_obs.md §4a: holds; §4c finds a tree with no tokio when the HTTP client is supplied, §14) | `ulo-otel`'s exporter slot |
| `ulo-cli` (changed) | Templates per transport and database for `ulo new`, `ulo generate` for five kinds, printing registration lines and editing no source; `ulo dev` unchanged (§5) | tokio-bound, unchanged | — |

Every crate builds on Rust 1.88, edition 2024, the workspace's `rust-version`, except where a library needs more and the crate states its own floor: `ulo-db-sqlx` and `ulo-db-seaorm` at 1.94, the `rust-version` SQLx 0.9.0 and SeaORM 2.0.4 declare (PROBES_db.md §1.1, §2), and `ulo-config`'s `yaml` feature at 1.89, `serde-saphyr` 2.0.0's (PROBES_config.md §4). mongodb 3.9.1 and redis 1.7.1 declare 1.88, diesel 2.3.14 1.86 and diesel-async 0.9.2 1.84 (PROBES_db.md, versions). The excluded crates under `crates/` (`ulo-config`, `ulo-health`, `ulo-db-diesel`, `ulo-db-mongodb`, `ulo-db-redis`, `ulo-db-seaorm`, `ulo-db-sqlx`, listed in the workspace `Cargo.toml`'s `exclude`) are the master API, not ported, and each crate of this table replaces its namesake and moves to `members`. `crates/ulo-db-prisma` is not ported: Prisma Client Rust's repository was archived with a "no longer maintained" notice on 2025-03-12 [11].

The core and transport crates this layer changes, with the extension each change belongs to:

| Crate | Change | Extension |
| --- | --- | --- |
| `ulo` | `Spawn::name`; `Permanent`, `is_permanent` and the readiness loop's early end; a `serde` feature with `Secret<T>: Deserialize`; `TraceContext`, `Execution::trace()`, `ExecOptions::trace`, `AppBuilder::trace(TraceMode)`; `TraceSource` | I1, I2, I3, I5, I7 |
| `ulo-runtime-tokio`, `ulo-runtime-smol` | Each runtime's `name()` | I1 |
| `ulo-transport` | `span::call` declares `otel.kind` and `otel.status_code` | I7 |
| `ulo-grpc` | `GrpcHealth::delegate_overall`; extraction and the `TraceSource` call in the dispatcher; injection in `outgoing` | I4, I6, I9 |
| `ulo-rpc` | Injection in the client's `written`; `traceparent` reserved per call; `tracestate` merged; extraction and the `TraceSource` call in the dispatcher | I6, I8, I9 |
| `ulo-http`, `ulo-ws` | Extraction and the `TraceSource` call where each opens an execution | I9 |
| `ulo-rpc-conformance` | Its harness pieces move into a shared suite crate | I11 |
| `runtime-levels.txt`, `Cargo.toml` | Entries and members for the new crates | I10 |

---

## 2. Configuration [1]–[5]

### 2.1 Sources and the origin tree

Sources are given, never found, and layer in the order added, each overriding the ones before [2]:

```rust
#[derive(Deserialize)]
struct AppConfig { http: HttpOptions, database: PgOptions, api_key: Secret<String> }

let config = Config::builder()
    .source(Defaults)                                         // from `Default`
    .source(File::toml("config/app.toml"))                    // required unless `.optional()`
    .source(DotEnv::file(".env").optional())
    .source(Env::process().prefix("APP_").separator("__"))    // a test passes `Env::from_map(map)`
    .source(Values::new().set("http.port", 8080))
    .load();                                                  // never fails; runs in `main`
```

Reading the process environment is the application's choice, written as `Env::process()`; a test passes a map. A `.env` file is read into the layer and never into the process environment.

`load` merges the sources into a value tree of the crate's own, where every leaf carries its origin: a file with its line and column, an environment variable's name, an explicit value, or a default. The tree and the deserializer collecting every problem over it are the crate's own code (PROBES_config.md §2: holds, about 200 lines for TOML and 20 per further format). Per format:

- **TOML:** the `toml` crate's spanned parsing.
- **JSON:** `jsonc-parser` 0.34.0 in strict mode, maintained and giving a byte range per value, so JSON reports line and column as TOML does. The design's condition, a maintained parser with value spans, is met (PROBES_config.md §3: the premise that none exists does not hold); without it JSON would report the file and the key path.
- **YAML:** behind a `yaml` feature, on `serde-saphyr`, whose errors give line and column (PROBES_config.md §4: holds in part: the message carries no field path, and the crate needs Rust 1.89, the floor the feature states). `serde_yaml` is archived, and `serde_yml` carries RUSTSEC-2025-0068, unsound and unmaintained.
- **`.env`:** the common subset, `KEY=VALUE`, quotes, `#` comments, no variable expansion (§7), with a line for each value's origin. `dotenvy`'s iterator sets no variable, but expands `$VAR` from earlier keys and from the process environment with no way to turn it off, and gives no line numbers (PROBES_config.md §5), so it cannot be the reader.

The crate is not built on figment: its errors name a key path and a source, give no line for a value, and report the first error alone (PROBES_config.md §1: holds, figment 0.10.19). The JSON and YAML file sources take `File::toml`'s form.

### 2.2 Loaded in `main`, recorded, reported by `wire()`

Registration is synchronous and does no I/O (DESIGN §3.6, DESIGN decision 4), so configuration cannot load during `wire()`. It loads in `main`, before anything serves, which also keeps every blocking file read off a runtime worker. `load` returns a `Config` and never fails: a source that cannot be read or does not parse is recorded inside the value.

Every problem is reported by `wire()`, in one report:

```rust
struct AppModule { config: Config }

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity { ModuleIdentity::of_type::<Self>() }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(ConfigModule::<AppConfig>::new(&self.config)
            .validate(|c: &AppConfig, p: &mut Problems| {
                if c.http.port == 1 { p.at("http.port", "must not be a privileged port") }
            }));
        m.import(SqlxModule::<Postgres>::from_config(&self.config, "database"));
    }
}

let app = App::builder(AppModule { config })
    .runtime(ulo_runtime_tokio::Tokio::current())
    .wire()?;                                   // source, value and validation problems, with every other wiring error
```

- `ConfigModule::<T>::new(&config)` registers `T` through `m.try_value(..)`. The `Err` it records carries every source failure, every value missing, ill-typed or invalid in `T`, and every finding of the validation hook [3].
- An integration reads its own section by path at its own registration, without knowing the application's type [1]: `from_config(&config, "database")` deserialises the section there and records a failure through `try_value`.
- A failing source yields one `WiringError::ValueFailed` per module that read a section from it, each listing that section's problems.
- Each problem names the field's path, the origin that supplied the value (file, line and column, or the variable's name) and what was expected.
- The error's `Display` carries every problem, one per line, with its path and origin, under a header line of its own, because `Redacted` flattens `source()` into one string (DESIGN §10.2): a problem reachable only through `source()` would be lost. The structured list stays reachable through `Redacted::downcast_ref`. PROBES_config.md §7: holds: the newlines and every problem pass through `try_value` and the `WiringErrors` renderer uncut at 65 problems and a 400-character line, with secrets and userinfo scrubbed.
- Missing fields, wrong types and out-of-range numbers are all collected. A failure inside a leaf type's own parser, an `IpAddr` or a `Url` built from a valid string, has no placeholder value and stops its enclosing struct's pass, so that case reports one problem for the struct (PROBES_config.md §2, §14).

### 2.3 Secrets

`Secret<T>`, the core's (`crates/ulo/src/redact.rs`), implements `Deserialize` behind a `serde` feature on `ulo` (I3), calling `deserialize_newtype_struct("Secret", ..)`. `ulo-config`'s deserializer recognises the name, records the leaf's value and path from its tree, and deserialises the inner value; any other deserializer sees a transparent newtype. `ConfigModule` and each section read call `m.secret(..)` for every leaf recorded, so the graph's redaction replaces the value in every error, `Debug` and log it reports [4]. A deserialisation run more than once records a leaf more than once, and the registry skips a repeat. `Secret` has no `Serialize` impl. Another crate's newtype also named `Secret` is recorded too, and costs extra redaction only.

PROBES_config.md §6: the newtype decodes transparently on `serde_json`, `toml`, `serde-saphyr` and `serde_norway`, and the deserializer records exactly the secret leaves of plain structs, `Option` and `Vec`. Under `#[serde(flatten)]` or `#[serde(untagged)]`, serde buffers the field into its own content type and the name never reaches the deserializer: nothing is recorded and nothing says so (§14).

A refusal involving a secret names its path and what was expected, never its value. Serde's and the parsers' messages quote the value they found (`expected u8, found 300`), and on the `try_value` path the registry holds nothing yet to replace (DESIGN §9.3), so a problem under a recorded secret path is rendered from its path and kind alone (PROBES_config.md §6).

### 2.4 Unread keys

At `connect`, `ConfigModule`'s init hook logs at `warn` every leaf no section read consumed, with its origin, an `APP_` variable matching nothing among them. `.deny_unread()` refuses instead, at `connect`. Neither can be a `wire()` refusal: the core runs no hook after every module has registered, and a lazily loaded module (DESIGN §8.6) may read a key later.

---

## 3. Health [6]–[10]

### 3.1 The check

```rust
pub trait Check: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn check(&self) -> BoxFuture<'_, Health>;      // `Health::Up` or `Health::Down(reason)`, the reason redacted
    fn bound(&self) -> Bound { Bound::Default }    // `Default` is 2 s; `After(ZERO)` refused; `Unbounded` the opt-out
    fn probe(&self) -> Probe { Probe::Readiness }  // `Probe::Liveness` only when written
}
```

A check that does not answer within its bound is down, never up [7]. Checks run concurrently, and a probe arriving while a run is in flight shares that run. No result is kept between runs. A zero bound is refused where the health module reads the collection, at `connect`, the first point a `&self` method can be read.

A check reaches readiness through the core's collections (DESIGN §3.2, §4): a module contributes it with `m.contribute::<dyn Check>()`, and the health module reads `Many<dyn Check>`, so no registration by hand can be forgotten [8]. A keyed module's contribution keeps its key (DESIGN §8.3): two keyed databases contribute two checks. No core extension is needed.

A lazily loaded module cannot contribute to a collection a pre-existing binding reads as `Many` (DESIGN §8.6): `load` refuses it with `LoadRefusal::Contribution`. A store module therefore carries `.without_health_check()`, which makes it loadable lazily at the cost of a readiness that cannot see it, chosen by the application where it is written.

### 3.2 The probes

- **Liveness** is true whenever the process answers, plus any check the application declares `Probe::Liveness`. Integrations never contribute to it.
- **Readiness** is true while the app is `Running` and every readiness check is up. It is false until `listen` has finished by construction: nothing answers a request before `serve`, every server has bound by then, and an embedding answers 503 in its `Unbound` state (transports §3.8). That case is a unit test of health's state, since nothing outside observes a probe before `serve`.
- **Startup** is true once `listen` has finished, which holds whenever a probe is answered.

**Readiness turns false at `Stopping`.** The health module's binding reads `Many<dyn Check>`, so it is constructed after every check, and its `BeforeApplicationShutdown` hook runs before every `Check`'s hooks, lazily loaded modules' hooks running ahead of it by the exact reverse of connect order (DESIGN §9.2, §9.5). The hook turns readiness false, then waits `.lag(d)`, a `Duration`, zero by default. Every refusal comes later, at `Draining`, so readiness is false before the first refusal with no core change [10].

- The hook is bounded `After(d + 1 s)`, so a lag is not cut by the 10 s `hook_timeout` default. A lag reaching past `shutdown_timeout` is cut by the cap and recorded `TimedOut { limit: ShutdownCap }`; the builder's cap is invisible at registration, so the documentation says this rather than a refusal.
- kubelet stops the liveness and startup probes at termination and keeps probing readiness until the container stops (`prober_manager.go`, `StopLivenessAndStartup`; PROBES_obs.md §6: holds). Kubernetes marks a pod's endpoints terminating at its deletion timestamp, before the `preStop` hook and SIGTERM, and an endpoint's `serving` condition follows the pod's Ready condition, so the flip at `Stopping` reaches every reader of `serving` (PROBES_obs.md §6: marking at SIGTERM holds in part). `.lag` is the `preStop` sleep's equivalent for a deployment without a `preStop` hook, the time kube-proxy and the load balancers need to see the endpoint change before the app refuses work; with one, the change has had that long already. The default stays zero, and the documentation recommends a few seconds for a deployment without a `preStop` hook.
- During `Draining`, the HTTP server's own 503 answers every probe before routing (transports §3.6), liveness included. kubelet has stopped the liveness probe by then, so a liveness failure during the drain cannot kill the pod.

### 3.3 HTTP: `ulo-health-http`

`HealthHttp::new()` serves `/health/live`, `/health/ready` and `/health/started`, each path configurable [9].

- **Status:** 200 when the probe passes, 503 when it does not; Kubernetes counts 200 to 399 as success (PROBES_obs.md §6).
- **Body:** the shape of the IETF health-check draft, which expired in April 2022: media type `application/health+json`, `status` `pass` or `fail`. The draft is a shape followed, not a contract.
- **Detail:** off by default, because it tells an outsider about internals. `.detail(true)` adds the draft's optional `checks` object, each check with its redacted reason.

### 3.4 gRPC: `ulo-health-grpc`

`ulo-grpc` already serves `grpc.health.v1` through tonic-health (`crates/ulo-grpc/src/health.rs`): the reporter's overall status `""` starts `SERVING` from `HealthReporter::new`, `bind` writes `SERVING` for every service the handlers' paths name (`crates/ulo-grpc/src/server.rs:385`), and `drain` writes `NOT_SERVING` for `""` and each of them (`server.rs:406`). `ulo-grpc` takes no dependency on `ulo-health`; `ulo-health-grpc` is a bridge crate depending on both.

- **One writer per service name.** `""` belongs to health when the bridge is imported, and to the server otherwise. In its init hook, before `listen()`, the bridge calls `GrpcHealth::delegate_overall()` (I4), after which the server writes no status for `""`, and the bridge writes `""` from `Readiness` through `Dep<GrpcHealth>`. Named services keep their writers: the server at `bind` and `drain`, the application through `GrpcHealth::set_named`. tonic-health's reporter is a map of independent channels with no ownership check, so one writer per name is held by what each component calls, and the last write to a name wins (PROBES_obs.md §1: holds).
- **Pushed at lifecycle transitions.** `""` turns `NOT_SERVING` at `Stopping`, so the change reaches a `Watch` subscriber as readiness flips, and whenever a check is down.
- **Re-evaluated on an interval, written on change.** gRPC's `Check` reads a stored status, not a fresh evaluation, so a task on the app's runtime re-evaluates readiness every interval. The interval is a `Bound`: `Default` is 5 s, `Unbounded` means statuses change at lifecycle transitions alone, and `After(ZERO)` is refused, as `ping_interval`'s zero is (transports decision 11). tonic-health sends a `Watch` message on every write, an unchanged value included, where `grpc.health.v1` sends one when the status changes (PROBES_obs.md §1), so the bridge keeps the last value it wrote and writes only a change.
- Kubernetes' gRPC probe sends `Check` with `service` defaulting to `""` (PROBES_obs.md §6).
- tonic-health answers `Check`, the initial `Watch` message and each update as `grpc.health.v1` specifies, and answers a `Watch` on a name nobody registered with `NOT_FOUND` where the specification streams `SERVICE_UNKNOWN` and keeps the call open; its `ServingStatus` has no `SERVICE_UNKNOWN` (PROBES_obs.md §1: holds in part, §14).

`Readiness` stays in `ulo-health`: in the core or `ulo-transport` it would put an integration's concept below the transports.

### 3.5 Scenarios

On every host that serves health:

1. Readiness turns false at `Stopping`, before the first refusal.
2. A check that hangs is down at its bound.
3. A check that goes down turns readiness false, and readiness recovers with it.
4. The body carries no detail by default.
5. Over gRPC, `Watch` receives `NOT_SERVING` on `""` at `Stopping`, and no message while the value is unchanged.

Readiness false before `listen` is a unit test of health's state (§3.2).

---

## 4. Databases and stores [11]–[21]

### 4.1 The `Store` trait

A new store plugs into the shared policy by implementing one trait, with no macros [13]:

```rust
pub trait Store: Send + Sync + 'static {
    const NAME: &'static str;                                          // "postgres", "mongodb": messages and check names
    type Options: DeserializeOwned + Clone + Eq + Hash + Send + Sync;  // the module's identity is by value (DESIGN §8.3)
    type Client: Clone + Send + Sync + 'static;                        // the library's own pool or client
    fn validate(o: &Self::Options) -> Result<(), ulo_config::Problems>;                 // at registration, through `try_value`
    fn create(o: &Self::Options) -> BoxFuture<'static, Result<Self::Client, BoxError>>; // without connecting; run through `spawn_with`
    fn ping(c: &Self::Client) -> BoxFuture<'static, Result<(), BoxError>>;              // the `.ready` attempt and the health check
    fn close(c: Self::Client) -> BoxFuture<'static, ()>;
}
```

The store has no error type of its own, so nothing clashes with `ulo::ConnectError` (DESIGN §10.2) or `ulo_rpc::Misconfigured` (`crates/ulo-rpc/src/link.rs:439`). A store marks an error permanent by returning `Err(Permanent::new(e).into())` from `ping` (I2). Which of each library's errors are permanent is in §4.6.

A store written without macros, SQLx's Postgres:

```rust
pub struct Postgres;

impl Store for Postgres {
    const NAME: &'static str = "postgres";
    type Options = PgOptions;                // `url: Secret<String>`, pool sizes; `Eq + Hash`
    type Client = sqlx::PgPool;

    fn validate(o: &PgOptions) -> Result<(), Problems> {
        /* the scheme checked first, since `PgConnectOptions` accepts any; then
           `PgConnectOptions::from_str(o.url.expose())`, a failure becoming a problem at `url`
           written from the failure's category, with the userinfo replaced by the redaction marker */
    }

    fn create(o: &PgOptions) -> BoxFuture<'static, Result<PgPool, BoxError>> {
        let o = o.clone();
        Box::pin(async move { Ok(PgPoolOptions::new().connect_lazy_with(o.connect_options())) })
    }

    fn ping(c: &PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move { sqlx::query("select 1").execute(&c).await.map(drop).map_err(permanent_or_not) })
    }

    fn close(c: PgPool) -> BoxFuture<'static, ()> {
        Box::pin(async move { c.close().await })
    }
}
```

`permanent_or_not` wraps the library's permanent errors (§4.6) in `Permanent` and leaves the rest bare. `PgConnectOptions` and `MySqlConnectOptions` print the password under `Debug` (PROBES_db.md §1.5), so neither is formatted anywhere, and the options type holds the URL as a `Secret`.

### 4.2 `StoreModule<S>` and the phases

`StoreModule<S>` implements `Module` once for every store, its identity `ModuleIdentity::of_value` over `S::Options`, so the store modules are nominal types rather than string-named `DynamicModule`s (F286). Each store crate names it for its stores, `SqlxModule::<Postgres>` in `ulo-db-sqlx`. It registers through the core's value API (DESIGN §9.1, §9.3, §13):

- **Registration (`wire`).** `S::validate` runs on the options the section read produced; a failure is recorded through `try_value` with every configuration problem (§2.2) [14]. The secrets the section carried are registered with `m.secret(..)`. A library's own message may echo the password, so the refusal is written from the parse failure's category, with any userinfo replaced by the redaction marker. PROBES_db.md §1.5, §2.2, §3.6, §4, §5: the parsers of SQLx, diesel-async, mongodb and redis-rs echo neither the URL nor the password; SeaORM's `Database::connect` writes the whole URL, password included, into its parse and no-driver errors, so SeaORM's store validates with a parser that does not echo and never hands a malformed URL to `Database::connect` (§14). Every library error stored in a core error type passes the graph's redaction (DESIGN §9.3), which replaces the registered URL.
- **The pool.** `try_singleton` registers the client. Its factory first reads `Dep<dyn Runtime>` and compares `name()` with `"tokio"` (I1), before anything is built: a runtime-free comparison, no ambient lookup, and a smol app driven from a thread with a tokio context still answers `"smol"`. Any other name, `"unknown"` included, fails with `UnsupportedRuntime { module, required, given }` wrapped in `Permanent`, naming the module, the runtime it needs and the runtime it was given [18]. The factory then runs `S::create` through `spawn_with` on the given runtime.
- **The bounded connect (`connect`).** `.ready(ping).retries(..).backoff(..).timeout(30 s)`, the ping run through `spawn_with`, under a total bound of 30 s by default; a zero bound is refused at registration, a refusal the core does not make on its own bounds (F313). An attempt returning `Permanent` ends the check at once, as `ConnectError::Readiness { reason: Errored(..) }`, so bad credentials stop retrying (I2). At the bound the start fails as `ConnectError::Readiness`, carrying no `Permanent`: retryable, and told apart from misconfiguration by `is_permanent` [14]. Backoff is constant, what `.backoff(Duration)` offers. An SQLx pool reports a refused connection as `PoolTimedOut` once its own `acquire_timeout` passes (PROBES_db.md §8.1), so one attempt's bound exceeds that timeout, or one attempt spends the whole bound.
- **Lazy (`.lazy()`).** Omits `.ready(..)`, so the start does not wait for the database, and the health check stays down until the first ping succeeds [14]. Offered only where the library creates a client without connecting (§4.6); elsewhere the method does not exist, so the refusal is at compile time.
- **Migrations (`.migrate(..)`).** Off by default. When written, they run in the module's `on_init`, after connecting, under `ulo-db`'s lock (§4.4). A failure is `MigrationFailed { module, source }` wrapped in `Permanent`, a misconfiguration and not retryable [17].
- **The health check.** `contribute::<dyn Check>()` with a check named from `S::NAME` that runs `ping`; omitted under `.without_health_check()` (§3.1).
- **The close.** `.on_destroy(close)`, run through `spawn_with`, under the hook's bound (§4.7).
- **The export.** `S::Client`, bare, so a consumer writes `Dep<sqlx::PgPool>` [12].

```rust
fn register(&self, m: &mut ModuleDef<'_>) {      // the root module's, as in §2.2
    m.import(ConfigModule::<AppConfig>::new(&self.config));
    m.import(SqlxModule::<Postgres>::from_config(&self.config, "database")
        .migrate(sqlx::migrate!("./migrations"))
        .keyed::<Primary>());
    m.import(SqlxModule::<Postgres>::from_config(&self.config, "analytics").keyed::<Analytics>());
}

#[injectable]
struct Users { db: Dep<PgPool, Primary>, stats: Dep<PgPool, Analytics> }
// sqlx::query("select 1").execute(&*self.db).await?
```

### 4.3 Several instances of one kind [16]

`Keyed<Q, M>` (DESIGN §8.3) requalifies the module's exports as `T @ Q`: consumers read `Dep<PgPool, Primary>` and `Dep<PgPool, Analytics>`, the identity includes `Q`, and each keyed instance contributes its own health check. A bare `SqlxModule::<Postgres>::from_config(..)` beside a keyed one is two modules with two pools and two checks.

### 4.4 Migrations and the lock

Migrations never run implicitly [17]. PROBES_db.md §2.3, §3.5, §7.1, two runners started together against one server:

- SQLx's `Migrator::run` takes `pg_advisory_lock` on Postgres and `GET_LOCK` on MySQL, and the second runner waits and finds nothing pending; on SQLite its `lock()` does nothing.
- SeaORM Migration 2.0.4's `Migrator::up` and `diesel_migrations` take no lock. Both runners execute the migration body at once; on Postgres, whose DDL and version-table insert are transactional, the loser fails its start on the version table's primary key. On an engine without transactional DDL a body with side effects can run twice (not probed).

`ulo-db` holds the lock protocol, written once, whatever the migrator has; with it the second replica waits rather than failing its start:

- Postgres: `pg_advisory_lock` on a dedicated connection, under a key of `ulo-db`'s own, distinct from SQLx's, so the SQLx lock taken inside it on another connection never waits on it.
- MySQL: `GET_LOCK`, the name built from `database()`, because MySQL's locks are server-wide.
- SQLite: an OS lock on a sidecar file; in-memory SQLite needs none.

PROBES_db.md §7.2: each primitive works from a dedicated connection and is released when that connection dies; `std::fs::File::lock` on a sidecar blocks a second handle and releases on drop, and is stable from Rust 1.89, above `ulo-db`'s 1.88 floor (§14). Mongo and Redis have no migrator and no `.migrate`. A migration using `CREATE INDEX CONCURRENTLY` can deadlock against a session waiting on an advisory lock, as golang-migrate's issue tracker records (not probed); the documentation states the hazard, and the suite's lock scenario uses ordinary DDL.

### 4.5 Runtimes [18]

Every `ulo-db-*` store crate is tokio-bound; smol variants are deferred (§12). A store refuses any runtime but tokio at `connect`, by name (§4.2). The refusal is at `connect`, not `wire`, because `wire` builds nothing and cannot read the runtime binding, and it is permanent by type.

- **Spawning at creation.** `create`, the readiness ping, migrations and the close run as tasks on the given runtime through `spawn_with`, so whatever a library spawns at creation runs on the app's runtime. That is the brief's "entered through the application's runtime", not a probe: the work runs on the runtime the app was given. PROBES_db.md §1.3, §4, §5: an SQLx pool spawns its reaper task at creation, lazy or not; a MongoDB client its topology monitors on the current tokio runtime; a `ConnectionManager` one task.
- **Handler queries.** A handler's own queries run on the app's workers, which have tokio's context once the name check has passed.
- **One runtime feature.** `ulo-db-sqlx` enables SQLx's `runtime-tokio` alone and `ulo-db-redis` redis-rs's `tokio-comp` alone. With a smol feature also on, SQLx chooses at every spawn and falls back to smol's global executor outside a tokio context, a pool built with no runtime at all included, and redis-rs chooses through a process-global preference (PROBES_db.md §1.2, §5: holds). Feature unification in an application that enables `runtime-smol` or `smol-comp` elsewhere brings the fallback back (§14).
- **Libraries in step.** SeaORM 2.0.4 depends on `sqlx = "0.9.0"`, and SQLx 0.9 declares `libsqlite3-sys` as a version range. `ulo-db-sqlx` and `ulo-db-seaorm` share one workspace entry for SQLx, and `ulo-db-sqlx` re-exports `sqlx`; with both, a build holds one SQLx (PROBES_db.md §1.4: holds). A CI job builds an application with both modules and fails when `cargo tree -d` shows two SQLx. Nothing refuses a second SQLx at compile time: SQLx 0.8.6 beside 0.9.0 resolves and builds, SQLite dropping to `libsqlite3-sys` 0.30.1 (PROBES_db.md §1.4: the compile-time refusal does not hold). The second copy's `PgPool` is another type, so `Dep<PgPool>` written against it is a missing binding at `wire()`, and the documentation directs users to `ulo_db_sqlx::sqlx`.

### 4.6 The stores

| Module | Client provided | URL schemes | Migrator | `.lazy()` |
| --- | --- | --- | --- | --- |
| `ulo-db-sqlx` | `PgPool`, `MySqlPool`, `SqlitePool` | `postgres://` (libpq's connection URI), `mysql://` (convention), `sqlite:` | `sqlx::migrate::Migrator` | yes, `connect_lazy_with` (PROBES_db.md §1.3: holds) |
| `ulo-db-seaorm` | `DatabaseConnection` | as SQLx | `sea-orm-migration` | yes, `ConnectOptions::connect_lazy(true)` (PROBES_db.md §2.1: holds) |
| `ulo-db-diesel` | a deadpool `Pool` of diesel-async connections; deadpool 0.13 is maintained (PROBES_db.md §3.4: holds) | as SQLx | `diesel_migrations` through `AsyncConnectionWrapper` | no |
| `ulo-db-mongo` | `mongodb::Client` | `mongodb://`, `mongodb+srv://` (MongoDB's connection-string specification) | none | yes, `Client::with_options` connects nothing (PROBES_db.md §4: holds) |
| `ulo-db-redis` | redis-rs `ConnectionManager` | `redis://`, `rediss://` | none | no: `ConnectionManager::new` connects (PROBES_db.md §5) |

`ulo-db-redis` takes the workspace's `redis` entry, version 1, which `ulo-rpc-redis` and `ulo-ws-redis` already use (`Cargo.toml:141`, read). `ConnectionManager::new` retries for about 9.5 s by default, a wrong password included, so the module sets `ConnectionManagerConfig::set_number_of_retries(0)` and the core's readiness check is the one retry policy (PROBES_db.md §5). `mongodb+srv` resolves DNS inside `ClientOptions::parse`, so a registration-time `validate` cannot parse it without network I/O (PROBES_db.md §4, §14).

What each store's errors mean (PROBES_db.md §8, probed against real servers; no text contained the password):

| Store | Permanent, wrapped in `Permanent` | Retryable |
| --- | --- | --- |
| SQLx | `Error::Configuration`; `Error::Database` with SQLSTATE class `28` or `3D000` on Postgres, `28000` or `42000` on MySQL, `14` on SQLite | `Io`, `PoolTimedOut`, `WorkerCrashed`, `Tls` |
| SeaORM | as SQLx, through the SQLx error it carries | as SQLx |
| diesel-async | not told apart by variant: wrong credentials and an unknown database both arrive as `DatabaseError(Unknown, message)`, the SQLSTATE lost (§14) | `UnableToSendCommand`, `BadConnection` |
| MongoDB | `ErrorKind::Authentication`; `InvalidArgument` | `ServerSelection` |
| redis-rs | `AuthenticationFailed`; `InvalidClientConfig`, from `Client::open` | `Io`, a timeout |

MongoDB has no unknown-database error: it creates a database on first write, and a database named in the URL with no `authSource` becomes the authentication source, failing as `Authentication`. SQLx's MySQL driver without a TLS feature or `mysql-rsa` fails a stock `mysql:8` at connect with `Error::Configuration` (PROBES_db.md §1.1, §14).

**Diesel [19]: diesel-async.** Its Postgres and MySQL connections are async, so no query blocks a worker. SQLite goes through `SyncConnectionWrapper`, whose `SpawnBlocking` trait has a `get_runtime()` taking no arguments; its default looks the current tokio runtime up and builds a hidden current-thread runtime when it finds none, an ambient lookup the brief forbids, while `SyncConnectionWrapper::with_runtime(conn, s)` and a pool manager's `custom_setup` accept a value of a `SpawnBlocking` type of the module's own (PROBES_db.md §3.2: "no handle can be passed in" does not hold; §14). Diesel's migrations run through `AsyncConnectionWrapper`, which blocks on futures, takes no runtime value, and panics on a tokio worker; inside `spawn_blocking` it runs (PROBES_db.md §3.3: holds), so the migrations run off the worker. `AsyncMigrationHarness` uses `block_in_place`, which panics on a current-thread runtime. A blocking pool owned by the module would duplicate what diesel-async already does, and a blocking-work method on the core's `Runtime` has no runtime-free consumer.

### 4.7 Closing [15]

The core already closes in reverse dependency order, and no extension is needed (DESIGN §9.2, §9.5): before-shutdown hooks, stop accepting, the drain, then `OnModuleDestroy` in reverse connect order, then the transports' sockets, then `OnApplicationShutdown`. A singleton is built before whatever depends on it, so it is destroyed after it, and a pool closes in its binding's `on_destroy`, after every transport has drained. Destroy hooks run before the transports' socket `close`, so `pool.close()` can run while an execution the drain abandoned still holds a query.

PROBES_db.md §6: holds. `sqlx::Pool::close` marks the pool closed at once, a new `acquire` failing, waits for every checked-out connection to return, and lets a query in flight complete. When the hook's bound expires first, the pool stays closed and the late holder closes its own connection as it drops it. The destroy hook's bound, its own `TIMEOUT` or `hook_timeout`, is what contains the wait, and nothing leaks past the holder's life.

### 4.8 Phases at a glance

| Phase | What a store module does |
| --- | --- |
| `wire` | validates its options, records every problem, registers its secrets, contributes its check |
| `connect` | checks the runtime's name; creates the client through `spawn_with`; pings under `.ready` (skipped by `.lazy()`); runs migrations under the lock when asked |
| `listen` | nothing |
| before-shutdown | nothing; health's hook, which runs before it, has turned readiness false |
| drain | nothing; in-flight executions keep using the client |
| destroy | closes the client through `spawn_with` under the hook's bound |

### 4.9 `ulo-db-conformance` [20]

The suite every store module is held to, run against real containers, a file for SQLite. Shaped like `ulo-rpc-conformance`: runtime-free at default features, a `tokio` harness feature, `publish = false`. It reuses `Budget` with its `stall` field, `block_on_one_thread`, `startup_failed!`, `report`, `relay::unshadowed`, `reachable` and the `not_applicable` stamping rules (`crates/ulo-rpc-conformance/src/lib.rs`, `relay.rs`); those pieces move into one shared `publish = false` crate both suites depend on (I11). Capabilities are named positively (`lazy`, `migrations`) and default to true (handoff §3.9), so a store lacking one declares `not_applicable` with its reason.

| Scenario | Planted break it is shown failing against |
| --- | --- |
| A malformed URL is refused at `wire`, the password redacted | a password echoed in the refusal |
| An unreachable database fails `connect` at its bound, not `Permanent` | `Unreachable` treated as permanent |
| Bad credentials fail `connect` at once, `Permanent` | a permanent error retried to the bound |
| `.lazy()` keeps readiness false until the first ping | readiness set true before the first ping |
| The health check goes down in an outage and recovers | a check that reports a cached `Up` |
| The pool closes after the transports drain, a query in flight at the drain completing | a pool closed before the drain |
| Connecting does not stall a one-thread runtime | a client created on the worker outside `spawn_with` |
| An unsupported runtime is refused at `connect`, `Permanent` | the name check removed |
| Two keyed instances stay separate | two qualifiers resolving to one instance |
| Migrations run only on request, and once across concurrent replicas | the migration lock removed |

### 4.10 Deferred here [21]

Execution-scoped providers exist (`#[injectable(execution)]`, `.execution(..)`, DESIGN §3.3, §6). Transactions scoped to an execution and per-test databases are deferred (§12).

---

## 5. Command line [22]–[25]

- **`ulo new <name> --transport <http|grpc|rpc-tcp|…> [--db <module>-<backend>]`** generates a project from templates compiled into the binary (`include_str!`, as `crates/ulo-cli/src/templates/new` is today), with no network fetch: one template per transport, plus an optional database. The generated project depends on `ulo = "=<the CLI's version>"`, an exact pin, where the template at `crates/ulo-cli/src/templates/new/Cargo.txt` pins a caret requirement today.
- **CI** builds every combination of transport and database, runs each generated project's tests, and fails a template whose tests run nothing, counting the tests libtest reports run as `scripts/unsafe_allowed.py miri` counts a Miri entry's (handoff §3.1). Generated code passes the `generated code lints` job's clippy, `-D warnings` on a pinned toolchain (transports §1).
- **`ulo generate <module|provider|controller|gateway|resolver> <name>`** writes new files and prints the line that registers each one. It edits no existing source: a parser round trip loses the user's formatting and comments. `update_app_module` and `update_mod_file` in `crates/ulo-cli/src/commands/generate.rs`, which edit the app module and `mod.rs` today, are removed. It refuses to overwrite an existing file, and refuses a name that is not a valid identifier.
- **`ulo dev`** is unchanged; its Windows gap stays recorded as F305.

---

## 6. Observability [26]

### 6.1 The core: `TraceContext` on the execution (I5)

`TraceContext` is a core value with no OpenTelemetry dependency, runtime-free, needing no clock and no randomness: a trace ID, a span ID, the flags as bits and the trace state, parsed and formatted by the core per W3C Trace Context. Each `Execution` carries its own, read by `Execution::trace()`. `.within(&exec)` forwards it as it forwards the deadline: the RPC client and `ulo_grpc::outgoing` read the context of the execution `.within` names (I6). The parent of an outgoing call is whatever `.within` names, never a span looked up from the thread. PROBES_obs.md §7: nothing in the repository carries a trace context today.

- **Standalone.** `ExecOptions::trace(TraceContext)` seeds a standalone `execute`'s inbound context. Without it, a standalone execution starts a new trace when a `TraceSource` is bound, and carries none in pass-through.
- **On by default: pass-through.** With no exporter, an incoming context is forwarded unchanged, so a ulo service does not break a trace that crosses it.
- **Off.** `AppBuilder::trace(TraceMode::Off)`: nothing is read or written, and `traceparent` is an ordinary header, as F400's correction made it.

### 6.2 Extraction and injection per transport (I9, I6)

| Transport | Extracted from | Injected into |
| --- | --- | --- |
| HTTP, served and embedded | the request's headers, where `AppService` opens the execution | — |
| gRPC | the request's metadata, in the dispatcher | the request's metadata, by `outgoing` |
| RPC | the frame's `h` | the frame's headers, by `RpcClient`, events included |
| WebSocket | the handshake's headers; each message's execution takes the connection's context as its parent | — |

Each transport extracts the context before it opens the execution, as RPC computes `deadline-ms` into `ExecOptions` today. The framework calls out over RPC and gRPC alone; it has no HTTP client. Only `Req`, `Evt` and `Open` frames carry `h`, which covers the request side, all propagation needs (PROBES_obs.md §7, read); since F412's fix a CBOR `open` carries it whole. On NATS, AMQP, MQTT and Kafka, `h` travels as the broker's own headers (transports §5.2, decision 12), so a consumer that is not ulo reads the context there; TCP, UDP and Redis carry the whole frame. An event carries the context though it carries no deadline (F408, F402): its handler is a consumer of the producer's span, as OpenTelemetry's messaging conventions model it, and the RPC dispatcher runs an event through the same path as a request.

### 6.3 Headers (I8)

- **`traceparent` is reserved per call.** The client refuses a caller's `traceparent`, `BadRequest` with `reason: "reserved_header"` at the await, as `deadline-ms` is (transports decision 124), exactly when the client writes one itself: on a call made `.within` an execution that carries a context. Otherwise it is an ordinary header. The client has no mode of its own: a call with no `.within` carries none, and so does an execution of an app with `TraceMode::Off`. A client built by `RpcClient::new(link, runtime)` outside any app follows the same rule. This closes F404.
- **`tracestate` is merged, not reserved.** A caller's `tracestate` entries enter as W3C Trace Context §3.5 has a vendor's: each at the left, an updated key moved left, a key appearing once and the caller's value winning, the execution's unmodified entries after them in their order. The list holds at most 32 members and at least 512 characters are propagated, truncated by whole entries, those over 128 characters first and then from the right (§3.3.1). That is how a vendor adds its own entry. PROBES_obs.md §5: holds, with the mutation rules at §3.5 and the grammar at §3.3.
- **An invalid incoming context** is ignored, and `tracestate` with it: a `traceparent` off the version-`00` grammar, with version `ff`, an all-zero trace or parent ID, uppercase hex, or two `traceparent` fields restarts the trace; a bad `tracestate` never affects `traceparent`. A higher version is parsed by the version-`00` offsets, with the end of the value or a dash after the flags, and written out as version `00`, unknown flag bits zeroed. The request still succeeds. This is the opposite of `deadline-ms`, refused when unreadable: the specification tells a receiver to restart the trace, and a trace context is advisory where a deadline is a bound. PROBES_obs.md §5: holds, read against the Level 1 Recommendation and the W3C conformance harness, with a reference parser passing 16 tests and failing against a planted break.

### 6.4 Spans and export: `ulo-otel` (I7)

`ulo_transport::span::call` (`crates/ulo-transport/src/span.rs`) already opens a `tracing` span per call with the OpenTelemetry semantic conventions' names and attributes (transports §2.9); `ulo-otel` exports those spans and adds no span of its own. A server span counts as an error on a 5xx and not on a 4xx; `span::call` declares `otel.kind` and `otel.status_code` beside its other fields, which the SERVER kind and that rule are recorded through (PROBES_obs.md §2). Database spans are left to each library's own instrumentation.

- **`TraceSource`.** The parent of a `tracing` span can be set only by code holding the `tracing::Span`, after its creation and before it is first entered: `tracing-opentelemetry`'s internals are crate-private, so no layer sets a parent from a recorded field (PROBES_obs.md §2: the field mechanism does not hold; `set_parent` holds). The parent is therefore taken directly from the source, the design's stated alternative: each transport, between `span::call(..)` and `.instrument(..)`, hands the span and the execution's context to the `dyn TraceSource` it reads as `Option<Dep<dyn TraceSource>>`. `ulo-otel` implements it with `OpenTelemetrySpanExt::set_parent`, then reads `span.context()` back to give the execution its own span's IDs. Without `ulo-otel` nothing is called, and pass-through still holds. The call costs about 0.43 µs per span.
- **Installation.** `ulo_otel::Otel::new(options)` is built in `main`, its options from a configuration section or from the standard `OTEL_*` names read from a given `Env`. Its `.layer()` goes into the `tracing` subscriber the application installs; the subscriber is the application's to set up. Its `.module()` is imported. The version set is `tracing-opentelemetry` 0.34.0 with `opentelemetry`, `opentelemetry_sdk` and `opentelemetry-otlp` 0.33.1 (PROBES_obs.md §2).
- **Export.** `ulo-otel-otlp`'s `OtlpExporter::http()`, OTLP over HTTP with protobuf, the exporter's default protocol, on reqwest's blocking client. An export reaches a receiver from a caller with no runtime, and from a current-thread runtime's only thread (PROBES_obs.md §4b: holds). The OTLP exporter's gRPC transport, `grpc-tonic`, needs tokio, and is deferred (§12).
- **Sampling** is parent-based by default; a remote parent whose sampled flag is clear produces no exported span (PROBES_obs.md §2).
- **The flush.** The batch processor exports from a thread of its own, `OpenTelemetry.Traces.BatchProcessor` (PROBES_obs.md §3b: holds). Its `shutdown` blocks the calling thread until that thread has flushed or its timeout, 5 s by default, passes, and stalls for the whole timeout when the exporter's future needs the runtime the caller is blocking (PROBES_obs.md §3c: the deadlock holds in part, as a bounded stall). `.module()` flushes it in `OnApplicationShutdown`, step 7, after the sockets have closed, so the transports' last spans are included. The flush runs on a thread of its own, which signals its end through a `futures-channel` oneshot; the hook awaits the oneshot under its bound, and no worker is blocked. On expiry the thread is left running, detached, and the hook is recorded `TimedOut`. It is the shape of the Kafka link's blocking helper.

### 6.5 Scenarios

On every transport suite, with a recording tracer:

1. One hop links parent to child.
2. An invalid `traceparent` starts a new trace, and the request still succeeds.
3. With tracing off, the header passes through untouched.
4. A caller-set `traceparent` is refused while the client writes one.
5. An event's handler span has the emitter's span as its parent.

---

## 7. Where standards decide

| Standard | What the design takes from it |
| --- | --- |
| Kubernetes probes | The meanings of liveness, readiness and startup; 200 to 399 as success; kubelet stopping the liveness and startup probes at termination and probing readiness until the container stops; endpoints marked terminating at the deletion timestamp, before `preStop` and SIGTERM, with `serving` following Ready; the gRPC probe's `Check` on `""` (§3.2, §3.4) |
| `grpc.health.v1` | `Check`, `Watch` with its initial message and a message per change, the empty service name `""` as the overall status, `SERVING` and `NOT_SERVING` (§3.4) |
| IETF health-check response draft, expired April 2022 | The body's shape and media type only, followed as a shape (§3.3) |
| W3C Trace Context, Level 1 | `traceparent`'s grammar and forward-compatible versions; the restart rule for an invalid header; `tracestate`'s grammar and limits (§3.3, §3.3.1) and the mutation rules (§3.5) (§6.3) |
| OpenTelemetry | Semantic conventions for span names, kinds and attributes, the messaging conventions' consumer parent for events, OTLP over HTTP with protobuf, the `OTEL_*` variable names (§6.4) |
| Database URLs | libpq's connection URI for Postgres, MongoDB's connection-string specification, `redis` and `rediss`; `mysql://` and `sqlite:` by convention; each parsed by a parser that does not echo the password (§4.2, §4.6) |
| TOML 1.0, JSON (RFC 8259), YAML 1.2 | The configuration file formats (§2.1) |
| `.env` | No specification exists; the design takes the common subset and says so (§2.1) |

---

## 8. Build order

The probes of §13 come first, and their verdicts are in. Each stage starts with the core and transport extensions it needs.

1. **Configuration and `ulo-health` with HTTP.** I3, then `ulo-config`, which every store module reads its section through; then `ulo-health` and `ulo-health-http`. Health precedes the stores because every store contributes a `Check`.
2. **`ulo-db` over `.ready`, with SQLx and the suite.** I1 and I2, then `ulo-db`, `ulo-db-sqlx`, I11 and `ulo-db-conformance` together: the suite is how `ulo-db`'s policy is shown to hold, and SQLx is the store with every capability, lazy and migrations included.
3. **The other stores.** SeaORM after SQLx, since it shares SQLx's workspace entry; Diesel, Mongo and Redis after that. `ulo-health-grpc` with I4 at any point after stage 1.
4. **The CLI's templates.** After the stores, since a template's `--db` choice names one.
5. **Trace propagation and `ulo-otel`.** I5, the carriers in every hub and in the client (I6, I8, I9), I7, then `ulo-otel` and `ulo-otel-otlp`. Last, because every transport is touched.

I10's entries go in with each crate, since `scripts/runtime_levels.py` fails a member with no entry.

---

## 9. SPI extensions (summary)

| # | Extension | Where, in the current repo | For |
| --- | --- | --- | --- |
| I1 | `fn name(&self) -> &'static str` on `Spawn`, defaulted to `"unknown"`; the core's `Clocked` forwards its spawner's name, so `.runtime(r).timer(t)` still answers `r`'s; `ulo_runtime_tokio::Tokio` answers `"tokio"`, `ulo_runtime_smol::Smol` `"smol"` | `Spawn` at `crates/ulo/src/runtime.rs:24`; `Clocked` at `runtime.rs:43`, its `impl Spawn` at `:68`; `impl ulo::Spawn for Tokio` at `crates/ulo-runtime-tokio/src/lib.rs:122`; `impl ulo::Spawn for Smol` at `crates/ulo-runtime-smol/src/lib.rs:192` | [18]: a store refuses any runtime but tokio by asking the given runtime (§4.2) |
| I2 | `#[non_exhaustive] pub struct Permanent(BoxError)`, built with `Permanent::new(e)`, `Display` the inner error's and `source()` the inner error; a readiness attempt whose error downcasts to `Permanent` ends the check at once as `ConnectError::Readiness { reason: Errored(..) }`; `ulo::is_permanent(&StartupError) -> bool`, walking the chain as `is_panic` does | the attempt loop in `run_readiness`, `crates/ulo/src/lifecycle/connect.rs:83`, its `Outcome::Done(Err(e))` arm at `:101`, where the attempt's `ConstructError` is read for a `Permanent` inside `Failed`; `is_permanent` beside `is_panic`, `crates/ulo/src/error/mod.rs:583`; the re-exports in `crates/ulo/src/lib.rs` | [14][17]: bad credentials stop retrying; `main` tells a restart that cannot help from one that might |
| I3 | A `serde` feature on `ulo` carrying `impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T>` through `deserialize_newtype_struct("Secret", ..)`; no `Serialize` | `crates/ulo/Cargo.toml` (no `serde` dependency today); `Secret` at `crates/ulo/src/redact.rs:69` | [4]: `ulo-config` records every secret leaf with no thread-local (§2.3) |
| I4 | `GrpcHealth::delegate_overall()`: after it, the server writes no status for `""` | `crates/ulo-grpc/src/health.rs`; the `""` write in `drain` at `crates/ulo-grpc/src/server.rs:406` (`bind` writes the named services alone, `server.rs:385`) | [9]: one writer for `""` when `ulo-health-grpc` is imported (§3.4) |
| I5 | `TraceContext` with the W3C parse and format; `Execution::trace()` and the same on `ExecutionRef`; `ExecOptions::trace(TraceContext)`; `AppBuilder::trace(TraceMode)`, `On` by default, `Off` | `ExecOptions` at `crates/ulo/src/execution/mod.rs:55`, beside its `deadline` at `:66`; `ExecShared` in the same file, the options copied into it at `crates/ulo/src/app/shared.rs:172`; `deadline()` on `Execution` at `:205` and on `ExecutionRef` at `:287`, beside which `trace()` goes; `AppBuilder` at `crates/ulo/src/app/mod.rs:84`; `TraceContext` in a new module of `crates/ulo/src/execution/` | [26]: the context an execution carries (§6.1) |
| I6 | `.within` forwards the execution's context: the RPC client writes it into the frame's headers on every shape, events included; `ulo_grpc::outgoing` writes it into the request's metadata | `Spec` at `crates/ulo-rpc/src/client.rs:438`, the three `within` methods at `client.rs:938`, `:996` and `:1102`, and `written` at `:593`, called at `:556` for a request or stream and `:1020` for an event, which returns early for a call with no bound; `outgoing` at `crates/ulo-grpc/src/client.rs:120`, which returns the request unchanged at `:121` when the execution has no deadline | [26]: injection on the calls the framework makes (§6.2) |
| I7 | `TraceSource`, an SPI each transport reads as `Option<Dep<dyn TraceSource>>` and hands the span and the execution's context between `span::call` and `.instrument`, answering the execution's span IDs; `span::call` declares `otel.kind` and `otel.status_code` | `call` at `crates/ulo-transport/src/span.rs:35`; `TraceSource` in the core beside `TraceContext`; the call sites are I9's | [26]: export through `ulo-otel` with the right parentage (§6.4) |
| I8 | The RPC client refuses a caller's `traceparent` as `reserved_header` on a call where it writes one, and merges a caller's `tracestate` per W3C §3.5 | `Spec::reserved` at `crates/ulo-rpc/src/client.rs:536`; `CallHeaders`'s doc at `crates/ulo-rpc/src/transport.rs:208`, which names `deadline-ms` alone as reserved today | [26], F404 (§6.3) |
| I9 | Extraction into the execution each hub opens, before `Execution::open`, and the `TraceSource` call after `span::call` | HTTP: `Execution::open` at `crates/ulo-http/src/service.rs:160`, the span at `:176`, which the embedding reaches through `AppService`; gRPC: `crates/ulo-grpc/src/dispatch.rs:158`, the span at `:169` through `call_span`, defined at `:507`; RPC: `crates/ulo-rpc/src/dispatch.rs:224` and the unhandled path at `:278`, the span at `:662`; WebSocket: the connect phase at `crates/ulo-ws/src/connection.rs:798`, its span at `:838`, a message at `:1391`, its span at `:1401`, a hand-written gateway's execution at `:143`, the terminal execution at `:1527`, and `crates/ulo-ws/src/gateway.rs:576` | [26]: every transport served (§6.2) |
| I10 | Entries for every new crate: `ulo-config`, `ulo-health`, `ulo-health-http`, `ulo-db`, `ulo-otel` and `ulo-db-conformance`, at its default features as `ulo-rpc-conformance` is, runtime-free; `ulo-health-grpc`, the five store crates and `ulo-otel-otlp` tokio-bound; the ported crates moved from `exclude` to `members` | `runtime-levels.txt`; the workspace `Cargo.toml`'s `members` and `exclude` | CI's `runtime levels` job fails a member with no entry |
| I11 | The RPC suite's `Budget`, `block_on_one_thread`, `startup_failed!`, `report`, `relay::unshadowed`, `reachable` and the `not_applicable` stamping rules moved into a shared `publish = false` crate both suites depend on | `crates/ulo-rpc-conformance/src/lib.rs` (`report` at `:88`, `startup_failed!` at `:107`, `block_on_one_thread` at `:132`, `Budget` at `:221`), `crates/ulo-rpc-conformance/src/relay.rs` (`unshadowed` at `:243`, `reachable` at `:267`), `tokio_harness.rs`, `smol_harness.rs` | [20], handoff §3.7: one copy of the suite harness |

I1, I2, I3, I5 and I7 are additive core changes; I1's default keeps every existing `Spawn` implementation compiling, the test runtimes under `crates/*/tests/` among them. I4 is one method on `ulo-grpc`. I6, I8 and I9 touch the hubs and the client and no core signature; I7's two fields are additive to `span::call`. I11 changes a transport suite's crate boundary and none of its scenarios.

---

## 10. What is refused, and where

| Refusal | When | Mechanism |
| --- | --- | --- |
| `.lazy()` on a store whose library cannot create a client without connecting | compile | no method (E0599) |
| `.migrate(..)` on Mongo or Redis | compile | no method (E0599) |
| Writing a `Secret` out through serde | compile | no `Serialize` impl |
| A configuration source that is missing (unless `.optional()`), unreadable or does not parse | `wire` | recorded by `load`, reported through `try_value` (§2.2) |
| A configuration value missing, ill-typed or invalid, or a validation hook finding | `wire` | `try_value`, every problem with its path, origin and expectation |
| A malformed URL or store option | `wire` | `Store::validate` through `try_value`, the userinfo redacted |
| A zero total connect bound on a store module | `wire` | the module's registration |
| A zero gRPC health re-evaluation interval | where the bridge's setting is read (§14) | as `ping_interval`'s zero (transports decision 11) |
| A second SQLx's `PgPool` read as `Dep<PgPool>` | `wire` | a missing binding (DESIGN §10.1); nothing refuses the second copy at compile time (§4.5) |
| An app on a runtime other than tokio, one declaring no name included, importing a store module | `connect` | `UnsupportedRuntime` in `Permanent`, from the pool's factory |
| Bad credentials or an unknown database | `connect` | `ping` returns `Permanent`; the readiness check ends at once |
| An unreachable database at the end of the connect bound | `connect` | `ConnectError::Readiness`, no `Permanent`: retryable |
| A failed migration | `connect` | `MigrationFailed` in `Permanent`, from the module's `on_init` |
| A zero `Check` bound | `connect` | the health module reading `Many<dyn Check>` |
| A key nothing read, under `.deny_unread()` | `connect` | `ConfigModule`'s init hook; a `warn` without it |
| A lazily loaded store module contributing its check | `load` | `LoadRefusal::Contribution` (DESIGN §8.6); `.without_health_check()` is the opt-out |
| A caller-set `traceparent` on a call where the client writes one | runtime, at the await | `BadRequest`, `reason: "reserved_header"` |
| An invalid incoming `traceparent` | not refused | ignored, the trace restarted, the request served |
| A check that does not answer in its bound | runtime | the check is down |
| A probe during `Draining` | runtime | the HTTP server's 503 before routing |
| A `.lag` past `shutdown_timeout`, or a flush past its bound | shutdown | recorded `TimedOut`, never refused |

---

## 11. Decisions

1. **Configuration loads in `main` and is reported by `wire()`.** Registration is synchronous and does no I/O (DESIGN §3.6, DESIGN decision 4), so `load` runs in `main`, never fails, and records each failure for `ConfigModule` and each section read to report through `try_value`. The file reads happen before anything serves, which keeps them off a runtime worker.
2. **The configuration crate is not built on figment.** figment's errors name a key path and a source but no line for a value, and line numbers are what the origin tree exists for. PROBES_config.md §1: holds; figment also reports the first error alone.
3. **YAML is behind a `yaml` feature, on `serde-saphyr` when its errors give locations, on `serde_norway` otherwise.** `serde_yaml` is archived and `serde_yml` carries an unsound-and-unmaintained advisory; `serde-saphyr` is maintained and contains no `unsafe` code. PROBES_config.md §4: `serde-saphyr` 2.0.0 gives line and column, so the rule picks it, with no field path in its message and a floor of Rust 1.89 the feature states; the probe's alternative, `saphyr`'s `MarkedYaml` into the origin tree, is §14's.
4. **JSON reports a line when a maintained parser gives value spans.** `serde_json` exposes positions for syntax errors alone. PROBES_config.md §3: the premise that no such parser is maintained does not hold, and `jsonc-parser` 0.34.0 in strict mode gives JSON the line and column TOML has.
5. **`.env` is the common subset, with no expansion, and never reaches the process environment.** No specification exists for the format; a test passes a map, and nothing in the layer writes or reads the environment unasked. PROBES_config.md §5: `dotenvy`'s iterator sets no variable, and expands `$VAR` from the process environment with no switch, so it is not the reader (§14).
6. **Unread keys warn at `connect`, and `.deny_unread()` refuses there.** The core has no hook after every module registers, and a lazily loaded module may read a key later, so `wire()` cannot be the place.
7. **`Secret` deserialises through `deserialize_newtype_struct("Secret", ..)`, behind a `serde` feature on `ulo`, with no `Serialize`.** The deserializer that owns the origin tree records each secret leaf itself, with no thread-local and no collector; a same-named foreign newtype costs extra redaction only, and writing a secret out is the leak redaction exists to prevent. `Secret` has no serde impl and `ulo` no serde dependency today. PROBES_config.md §6: holds for plain structs, `Option` and `Vec`; does not hold under `#[serde(flatten)]` or `#[serde(untagged)]` (§14).
8. **A configuration error's `Display` carries every problem, one per line with its path and origin.** `Redacted` flattens `source()` into one string (DESIGN §10.2), so a problem reachable only through `source()` would be lost from the report. PROBES_config.md §7: holds with the renderer as it stands.
9. **Readiness turns false at `Stopping`, in health's before-shutdown hook, and `.lag(d)` follows it.** The before-shutdown stage runs while traffic is normal and before any refusal (DESIGN §9.5), so requirement [10] holds with no core change; `.lag` is a `Duration`, because zero means no lag rather than a refused value, and its hook is bounded `After(d + 1 s)`, so the 10 s default does not cut it.
10. **No health route is served through the drain.** The HTTP server answers every request 503 before routing once the drain begins (transports §3.6), and kubelet stops liveness and startup probes at termination, so a liveness failure during the drain cannot kill a pod and a change to the HTTP transport buys nothing. PROBES_obs.md §6: holds.
11. **The gRPC overall status `""` turns `NOT_SERVING` at `Stopping` when `ulo-health-grpc` is imported, a departure from transports §6.2.** §6.2 keeps health `SERVING` through the before-shutdown stage, "consistent with `is_draining()`", which predates readiness; its own reason, a client polling health stops sending before the GOAWAY, is served better by the earlier flip. Without the bridge, §6.2 is unchanged.
12. **One writer per gRPC service name, and `ulo-grpc` does not depend on `ulo-health`.** `""` is health's once the bridge calls `GrpcHealth::delegate_overall()` (I4), the server's otherwise; named services keep the server's and the application's writes. A transport depending on an integration would put the integration below the transport, and `Readiness` in the core or `ulo-transport` would be the same inversion the other way, so `ulo-health-grpc` is a bridge crate depending on both. PROBES_obs.md §1: the per-name split holds in tonic-health.
13. **The gRPC re-evaluation interval is a `Bound`, and the bridge writes only a change.** `Unbounded`, statuses changing only at lifecycle transitions, is meaningful and so is the opt-out; under the settings vocabulary a `Duration` is a time never off (transports decision 11), and a zero `Duration` here would mean evaluating continuously. `Default` is 5 s. `grpc.health.v1` sends a `Watch` message when the status changes and tonic-health sends one on every write (PROBES_obs.md §1), so an unchanged value is not written.
14. **The health body follows the expired IETF draft as a shape, with detail off by default.** No finished standard exists; detail tells an outsider about internals.
15. **No check result is kept between runs.** A stale "up" is what readiness exists to avoid; concurrent probes share the run in flight.
16. **A check's default bound is 2 s, a zero bound is refused, and a check past its bound is down.** [7] asks for each; `Unbounded` is the explicit opt-out.
17. **Health collects checks through `Many<dyn Check>`, with no core extension, and a lazily loaded store's check is a stated limit.** `contribute` and `Many` exist (DESIGN §3.2, §4), collections are app-wide, and keyed contributions keep their key (DESIGN §8.3). `load` refusing a lazy module's contribution (DESIGN §8.6) is visible rather than silent, and `.without_health_check()` lets the application choose lazy loading over a readiness that sees the store; lifting the core's rule is a core design.
18. **`ulo-db` writes no retry loop: the connect policy is the core's `.ready(..).retries(..).backoff(..).timeout(..)`.** One copy of the policy (handoff §3.7) is the core's, and a library's own retry loop is turned off, as redis-rs's is (§4.6). Backoff is therefore constant, the shape `.backoff(Duration)` offers; exponential backoff is deferred.
19. **`Permanent` is a core type, and a permanent failure ends a readiness check at once** (I2). Without it a readiness check cannot stop its own retries, and wrong credentials are retried to the bound; `is_permanent` lets `main` tell a restart that cannot help from one that might. A store has no error type of its own, so nothing clashes with `ulo::ConnectError` or `ulo_rpc::Misconfigured`. PROBES_db.md §8: SQLx, MongoDB and redis-rs tell permanent errors apart by kind or SQLSTATE; diesel-async does not (§14).
20. **A store asks the given runtime its name, through `Spawn::name`** (I1). A module capturing `Tokio::try_current()` where it is built, or taking `.with_handle(h)`, changes nothing about where a handler's queries are polled, so it does not answer "what runtime is this app on?"; only the given runtime can. An in-task `Handle::try_current()` is the hidden coupling transports decision 46 removes: a smol app driven from a thread with a tokio context sees it succeed. The name is a method because `Runtime` stays dyn-compatible, a string so a third-party runtime can declare itself, on `Spawn` because `Runtime` is a blanket impl, and its `"unknown"` default is refused like any other name, so a runtime that declares nothing fails the check (handoff §3.9).
21. **An unsupported runtime is refused at `connect`, not `wire`.** `wire` builds nothing and cannot read `Dep<dyn Runtime>`; the refusal carries `Permanent`, so it is told apart from an unreachable database by type.
22. **Every library call that may look a runtime up runs through `spawn_with` on the given runtime.** Creating the pool, the readiness ping, migrations and the close: whatever the library spawns at creation runs on the app's runtime, which is the brief's "entered through the application's runtime". PROBES_db.md §1.2, §1.3, §4, §5: each library spawns at creation on the runtime current at the call.
23. **Migrations run under `ulo-db`'s own lock for every migrator, and no store refuses `.migrate` for want of one.** SeaORM's and Diesel's migrators take no lock and SQLx's does; one guard written once in `ulo-db` makes the brief's per-library refusal unnecessary, and SQLx's lock nested inside it, under another key, is harmless. PROBES_db.md §7: holds; without the guard the second replica's start fails on the version table's key rather than applying twice on Postgres, and the guard turns that failure into a wait.
24. **Several instances of one store are `Keyed` modules, not a marker parameter on a wrapper type.** `Keyed<Q, M>` requalifies exports as `T @ Q` and gives each instance its own identity and check (DESIGN §8.3); a `Db<S, Name>` wrapper would duplicate it and stand between the application and the library's type.
25. **Diesel goes through diesel-async.** Its Postgres and MySQL connections are async. A blocking pool owned by the module would duplicate what diesel-async does, and a blocking-work method on the core's `Runtime` has no runtime-free consumer. PROBES_db.md §3.2: SQLite's default `SpawnBlocking` looks the runtime up, and a type of the module's own can be handed in instead (§14).
26. **SQLx is one workspace entry, and CI fails two copies.** `ulo-db-seaorm` depends on SQLx through SeaORM 2.0, so the two crates share the entry and a CI job reads `cargo tree -d`. PROBES_db.md §1.4: one SQLx with SeaORM holds; a compile-time refusal of a second copy does not, which leaves the CI job and the re-export as the guard.
27. **A pool closes in its binding's `on_destroy`, with no core extension.** The core already destroys in reverse connect order after the drain (DESIGN §9.2, §9.5). PROBES_db.md §6: holds; `sqlx::Pool::close` waits on a connection an abandoned execution holds, and the hook's bound contains that wait.
28. **Every store crate is tokio-bound, and smol variants are deferred.** SeaORM supports tokio alone, the MongoDB driver tokio alone in 3.x, and diesel-async is built on tokio-postgres and mysql_async. PROBES_db.md §1.2, §5: holds; SQLx's `runtime-smol` and redis-rs's `smol-comp` spawn on smol's global executor, not the app's runtime.
29. **`ulo-db-conformance` is shaped like `ulo-rpc-conformance`, and the pieces both need move into one shared crate** (I11). Handoff §3.7 rules out a second copy.
30. **`ulo generate` writes new files and edits none.** A parser round trip loses the user's formatting and comments; the printed registration line costs one paste.
31. **A generated project pins the CLI's exact version.** A caret requirement would let a later framework release meet templates written for an earlier one.
32. **Trace context is a core value with no OpenTelemetry dependency, forwarded by `.within`.** The core stays runtime-free and exporter-free, and the parent of an outgoing call is the execution `.within` names, never a span read from the thread. A child span opened inside a handler and passed on explicitly is a refinement, deferred.
33. **Propagation is on by default as pass-through, and `TraceMode::Off` turns it off.** A ulo service then breaks no trace that crosses it, with nothing exported; export starts once `ulo-otel` is imported.
34. **`traceparent` is reserved per call, exactly when the client writes it, and the client has no mode.** A caller's value either contradicts the context the client writes or is the only context the call carries; one rule covers a client inside an app, outside one, and an app with tracing off. It closes F404.
35. **`tracestate` is merged, not reserved.** W3C Trace Context §3.5 lets a vendor add its own entry, which a reservation would forbid. PROBES_obs.md §5: holds as stated in §6.3.
36. **An invalid incoming trace context restarts the trace and never fails the request.** The specification says so, and a trace context is advisory where `deadline-ms` is a bound. PROBES_obs.md §5: holds.
37. **`ulo-otel` exports the spans the transports already open, through a `TraceSource` handed each span before it is entered, into the application's own subscriber.** `span::call` already follows the semantic conventions (transports §2.9), so a `Tracer` SPI of the framework's own would be a second span model; the subscriber is the application's, so `ulo-otel` hands over a layer. PROBES_obs.md §2: a layer setting the parent from a recorded field does not hold, `set_parent` from code holding the span does, which is the design's stated alternative.
38. **OTLP export defaults to HTTP with protobuf on reqwest's blocking client, and gRPC export is deferred.** The exporter's gRPC transport needs tokio; the blocking client runs on a thread of its own and touches no app runtime. PROBES_obs.md §3a, §4: `ulo-otel` runtime-free holds; the default exporter brings tokio, which makes `ulo-otel-otlp` tokio-bound as stated, and a supplied HTTP client would make its tree tokio-free (§14).
39. **The flush runs on a thread of its own and signals through a oneshot the hook awaits under its bound.** The batch processor's `shutdown` blocks, and joining a thread inside an async hook would block a worker. It runs at `OnApplicationShutdown`, after the sockets close, so the last spans are included. PROBES_obs.md §3c: the stall holds in part, bounded by the processor's timeout and only with an exporter bound to the blocked runtime; the off-thread flush keeps the worker free in every case.
40. **Exit codes are not mapped.** The core maps none and `main` returns the error; the distinction between a misconfiguration and a retryable failure is carried by type and read by `is_permanent`. `sysexits.h`'s 78 and 75 are deferred.
41. **Not built, because each is answered by the core or transport design as it stands.** A lifecycle value with a watched state: `AppHandle::phase()` (transports X13) and the before-shutdown stage answer it. An injectable runtime that downcasts to `Tokio` or `Smol`: the injected runtime is the core's wrapper with no `Any` supertrait (DESIGN decision 25), and I1 replaces it. A drain delay on the builder: `.lag` in health's before-shutdown hook. A `serves_during_drain` route option: decision 10. A collector for secrets scoped to one deserialisation: I3's newtype name. An untyped `ConfigTree` read by `options_from`: `from_config(&config, path)` at registration. A `Db<S, Name>` wrapper: decision 24.
42. **Prisma Client Rust is excluded.** Its repository was archived with a "no longer maintained" notice on 2025-03-12 and has had no change since; `crates/ulo-db-prisma`, which F31 concerns, is not ported.

---

## 12. Deferred

- **Reloading configuration while running.** Every binding resolved at `wire` would need a second, live way to be read.
- **Remote configuration sources** (a secrets manager, a configuration service). They are network reads before anything serves, which `load` in `main` does not bound or retry; the phase rules cover no network dependency of configuration.
- **Transactions scoped to an execution.** The core has no asynchronous hook at an execution's end, so a commit or rollback cannot run there; it is a core design of its own.
- **Per-test databases.** They need the suite's container harness settled first.
- **Smol variants of the stores.** SQLx's and redis-rs's smol support spawn on smol's global executor, not the app's runtime (PROBES_db.md §1.2, §5), and SeaORM and the MongoDB driver have none.
- **Exponential backoff on connect.** The core's `.backoff(Duration)` is constant (decision 18).
- **OTLP export over gRPC.** `grpc-tonic` needs tokio, until it can run on the given runtime (decision 38).
- **Exit codes** `EX_CONFIG` (78) and `EX_TEMPFAIL` (75), a convention of BSD's `sysexits.h`, not a standard (decision 40).
- **A child span opened in a handler and passed on explicitly** (decision 32).
- **A second writer for named gRPC services.** The server at `bind` and `drain` and the application through `set_named` both write a named service's status; that predates this layer and is a refinement.
- **Trace context in a broker's native headers is not deferred.** On NATS, AMQP, MQTT and Kafka the frame's `h` is the broker's native headers (transports §5.2), so the context reaches a consumer that is not ulo there with nothing further built (§6.2). On TCP, UDP and Redis, which carry the whole frame, it sits inside ulo's frame; Redis Pub/Sub carries no headers.
- **Publishing `ulo-db-conformance`.** It is `publish = false`, as every suite is (F403).
- **`ulo dev` socket holding on Windows** (F305).
- **A zero bound on the core's own hooks, constructions and readiness checks** (F313). A store module refuses its own zero connect bound; the core's refusal stays deferred.

---

## 13. Premises and their verdicts

Each premise was probed before its stage; the verdict is the probe file's, and the section named is where it stands. "Read" marks a premise the repository answers. A premise marked "not probed" is checked by the stage that depends on it, and a failure is returned as a design question before that stage's code.

### 13.1 Configuration, PROBES_config.md

| Premise | Verdict | Where |
| --- | --- | --- |
| figment gives no line for a value | holds (§1); it also reports the first error alone | decision 2 |
| TOML spans let the crate's deserializer report every problem with its line | holds with limits (§2): a leaf type's own parser failing stops its struct's pass; a re-run records a secret twice | §2.2 |
| No maintained JSON parser gives value spans | does not hold (§3): `jsonc-parser` 0.34.0 | decision 4 |
| `serde-saphyr`'s errors give locations | holds in part (§4): line and column, no path, MSRV 1.89 | decision 3 |
| `dotenvy`'s iterator reads without setting the environment | holds for the environment; fails the subset (§5): forced expansion from the process environment, no line numbers | decision 5 |
| `Secret` through `deserialize_newtype_struct` is transparent and recorded | holds (§6) except under `#[serde(flatten)]` and `#[serde(untagged)]`; found values in messages print a secret | decision 7, §2.3 |
| `Secret` already implements `Deserialize` | does not (§6, read) | I3 |
| A multi-line `Display` survives `try_value` and the renderer | holds (§7) | decision 8 |

### 13.2 Databases and stores, PROBES_db.md

| Premise | Verdict | Where |
| --- | --- | --- |
| SQLx 0.9 exists, with `runtime-tokio` and `runtime-smol`; its MSRV fits 1.88 | exists (§1.1); MSRV 1.94, as SeaORM 2.0.4's | §1 |
| With both runtime features, SQLx picks tokio when a context exists | holds, per spawn, falling back to smol's global executor (§1.2) | §4.5 |
| `connect_lazy` connects nothing; creation spawns | holds (§1.3) | §4.6 |
| SeaORM on one SQLx; a second SQLx refused at compile time | one SQLx holds; the refusal does not (§1.4) | §4.5, decision 26 |
| Parse errors do not echo the URL | holds for SQLx, diesel-async, mongodb and redis-rs; does not for SeaORM; SQLx's and SeaORM's options print the password under `Debug` (§1.5, §2.2, §3.6, §4, §5) | §4.1, §4.2 |
| SeaORM creates a connection without connecting | holds (§2.1) | §4.6 |
| Migrator locks: SQLx yes, SeaORM and `diesel_migrations` no | holds (§2.3, §3.5, §7.1) | §4.4, decision 23 |
| `SpawnBlocking` cannot take a handle | holds in part (§3.2): `with_runtime` and `custom_setup` take a value | §4.6, decision 25 |
| `AsyncConnectionWrapper` must not run on a tokio worker | holds (§3.3) | §4.6 |
| deadpool is maintained | holds (§3.4) | §4.6 |
| SQLx's and redis-rs's smol support spawn on smol's global executor | holds (§1.2, §5) | decision 28 |
| `sqlx::Pool::close` waits for a held connection | holds (§6) | §4.7, decision 27 |
| Guard primitives work from a dedicated connection | holds (§7.2); `File::lock` needs Rust 1.89 | §4.4 |
| Errors divide into permanent and retryable per store | holds for SQLx, MongoDB, redis-rs; diesel-async loses the SQLSTATE (§8) | §4.6 |
| `ConnectionManager` and Diesel's pool create without connecting | `ConnectionManager::new` connects and retries about 9.5 s (§5); deadpool's `build` connects nothing (§3.4) | §4.6, §14 |
| The Redis client `ulo-rpc-redis` and `ulo-ws-redis` use | read: the workspace's `redis` 1, resolved at 1.7.1 | §4.6 |
| `CREATE INDEX CONCURRENTLY` under an advisory lock can deadlock | not probed | §4.4 |

### 13.3 Health and observability, PROBES_obs.md

| Premise | Verdict | Where |
| --- | --- | --- |
| tonic-health's `Watch` behaves as `grpc.health.v1` specifies | holds in part (§1): unregistered `Watch` answers `NOT_FOUND`; an unchanged write sends a message | §3.4, decision 13 |
| A layer sets a span's parent from a recorded field | does not hold; `set_parent` from code holding the span does (§2) | §6.4, decision 37 |
| `opentelemetry_sdk` without `rt-*` pulls no tokio | holds (§3a) | §1 |
| The batch processor exports from its own thread; `shutdown` can deadlock | thread holds (§3b); the deadlock holds in part, as a bounded stall (§3c) | decision 39 |
| `ulo-otel-otlp` must be tokio-bound | holds for the default client; does not with a supplied one (§4) | §1, decision 38 |
| Each W3C rule, against the specification's text | holds (§5); the mutation rules are §3.5 | §6.3 |
| kubelet stops liveness and startup at termination; endpoints terminate at SIGTERM | the first holds; the second holds in part, at the deletion timestamp (§6) | §3.2 |
| The core carries a trace context today; every frame kind carries `h` | none today (§7, read); `Req`, `Evt` and `Open` carry `h` (read, `crates/ulo-rpc/src/frame.rs:35-41`) | §6.1, §6.2 |

### 13.4 The core and transports, read

- Several contributions of one kind: `contribute::<T>()` and `Many<T>` (`crates/ulo/src/module/def.rs:329`).
- Close order: destroy hooks in reverse connect order after the drain, before the sockets (DESIGN §9.5).
- Options read at registration: `ModuleDef::try_value` (`crates/ulo/src/module/def.rs:190`).
- Two bindings of one type: `Keyed` (`crates/ulo/src/module/keyed.rs`).
- The core logs through `tracing` today (`crates/ulo/Cargo.toml`).
- `""` starts `SERVING` in tonic-health 0.14.6's `HealthReporter::new`, and `ulo-grpc` writes it only at `drain` (`crates/ulo-grpc/src/server.rs:406`).

---

## 14. Points each area resolves in its own log

Not settled by the design. The owning area decides and logs the decision.

- **Configuration.** The spelling of the JSON and YAML file constructors beside `File::toml`, and whether JSON sits behind a feature; YAML on `serde-saphyr` with its 1.89 floor or on `saphyr`'s `MarkedYaml` into the origin tree at 1.85, which the probe recommends, anchors, aliases, merge keys and YAML 1.1 scalars unprobed under it; the `.env` reader that keeps the subset, the crate's own parser of about 40 lines being the probe's recommendation; what keeps a `Secret` under `#[serde(flatten)]` or `#[serde(untagged)]` from going unregistered, a documented restriction or a collector `Secret::deserialize` writes to; whether a failure inside a leaf type's own parser stays a single-problem case or gains a second pass; `Problems`' constructors and how a section's problems are prefixed with the section's path; how `ConfigModule` learns which leaves each section read consumed.
- **Health.** The health module's name and how `.lag` is written on it; `Readiness`'s read methods; whether `HealthHttp` is imported as a module or mounted as a controller; the error type of a zero `Check` bound; where the bridge's re-evaluation interval is set and so where its zero is refused; how two keyed instances' checks are named apart, `S::NAME` being one string; whether `ulo-health-grpc` imports `GrpcHealth::module()` itself or requires the application to; whether tonic-health's `NOT_FOUND` for a `Watch` on an unregistered name is documented as a deviation or answered by a `Health` service of the bridge's own over the same reporter.
- **Databases.** Whether each store crate's module is a type alias of `StoreModule<S>` or a newtype; the default `.retries(..)` and `.backoff(..)`, the setter name of the total connect bound, and how an attempt's bound is kept above an SQLx pool's `acquire_timeout`; how `.lazy()` is absent at compile time, and how the Redis store fits `Store::create` "without connecting" when `ConnectionManager::new` connects, a construction failure being outside `.ready`'s retries; why Diesel offers no `.lazy()` when deadpool's `build` connects nothing; the `Migrations` trait's shape and the split between `ulo-db`'s lock protocol and the store's dedicated connection, `ulo-db` naming no driver; the lock key `ulo-db` uses on Postgres; how the SQLite sidecar lock is taken off the worker and at which crate's floor, `File::lock` needing 1.89; the `SpawnBlocking` type Diesel's SQLite connections take through `custom_setup`, the given runtime being a `dyn Runtime` and not a tokio handle; the path Diesel's migrations take off the worker; how diesel-async's errors are classified, the driver's own connect in `ping` or matched messages; which parser SeaORM's store validates with, SQLx's or the `url` crate; how `validate` treats `mongodb+srv`, whose parse resolves DNS; which TLS feature or `mysql-rsa` the MySQL driver enables; whether anything catches an application whose tree enables SQLx's `runtime-smol` or redis-rs's `smol-comp`, and whether `ulo-db-redis` calls `prefer_tokio()`; the on-disk directory of the Mongo crate, `crates/ulo-db-mongodb` today; the shared suite crate's name (I11).
- **Command line.** The transport and database names `--transport` and `--db` accept; whether the no-tests count is factored out of `scripts/unsafe_allowed.py` or written again; what the `resolver` template depends on.
- **Observability.** `TraceContext`'s and `TraceSource`'s module paths in the core and `TraceSource`'s method signature; how a hub reads `TraceMode`, or whether `Execution::open` drops an extracted context under `Off`; whether `TestApp` carries `trace(..)`; whether an invalid incoming context in pass-through leaves the execution with no context, as a standalone execution in pass-through has, or a new trace, which only a bound `TraceSource` can give IDs to; how a caller's `tracestate` merge in pass-through meets W3C §3.4, under which a `traceparent` forwarded unchanged carries its `tracestate` unchanged; W3C Level 1 or Level 2, which adds the `random` flag; whether `ulo-otel-otlp` ships the reqwest blocking client or a supplied HTTP client over `ureq` with no tokio in its tree, and its `runtime-levels.txt` entries per feature; how the exporter is handed to `Otel` and how `OtelOptions` is read from a section.
