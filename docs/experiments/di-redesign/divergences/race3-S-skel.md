# Divergences: the race 3 spine, S-skel

Every place S-skel departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and fifth
responses, or fills a shape they leave open. Each entry gives what the design says, what the skeleton
writes, and why. All await the user's sign-off.

No cargo was run. Every claim about what compiles is read from the sources and the registry; the
items a compile would have to show are listed under "Not verified".

S-skel wrote 88 skeleton files (13 crates), the root `Cargo.toml` and `runtime-levels.txt`, and removed
the `master` files of the seven rewritten crates (`src/**`, `tests/**`, `examples/**`, `README.md`).
No `master` code is carried over.

## Requests to other owners

R1. **S-core, `crates/ulo/src/__private.rs`: read the texts of a `CollectedSecrets`.** `Section<T>`
(`ulo-config/src/section.rs`) is `Clone + Eq + Hash`, because a store module's identity and
`Module::register(&self)` both need it, and keeps its secrets as `Vec<Secret<String>>`.
`CollectedSecrets` has `register(self)`, `append`, `len` and `is_empty`, and no clone and no
accessor, so C cannot turn one into that vector. Needed: `CollectedSecrets::texts(&self) ->
&[String]`, or `Clone`. If S-core declines, C changes the field to hold the collection behind an
`Arc` and registers through a clone of its texts.

R2. **S-core or H1: redaction of a `Health::Down` reason.** DESIGN §3.1 says the reason is redacted
and `§3.3` prints it under `.detail(true)`. `ulo::redact` is private, so `ulo-health` cannot scrub a
`BoxError` with the graph's registry. `Health::Down` holds a `BoxError` (the skeleton's choice).
Needed: a public entry the health module can call, or the health module handing back `Redacted`
values the core produced. H1 raises it again if it finds a route.

R3. **Coordinator, after the spine: `Cargo.lock`.** It already shows as modified on this branch from
an earlier tool run; S-skel did not touch it. The new dependencies resolve at the next
`cargo check`.

## Signatures S-skel chose where the plan was silent

### `ulo-db`

D1. **`Store::secrets(o) -> Vec<Secret<String>>`, a defaulted method (empty), added to the frozen
trait.** The design (P3, §4.2) registers "the whole URL and its password component" with
`m.secret(..)`. The whole URL is a `Secret<String>` leaf, so the section read registers it. The
password component is derived from the URL, and `StoreModule` names no driver and cannot parse it.
Without a hook, no store can register it. Every existing method keeps its signature; each SQL, Mongo
and Redis store overrides this one, SQLite included.

D2. **`.lazy()` is absent at compile time through a marker trait, `LazyStore: Store`.** `CREATE_CONNECTS`
is a value and cannot gate a method. `impl<S: LazyStore> StoreModule<S> { fn lazy }`, and each store
whose `CREATE_CONNECTS` is `false` implements the marker. Nothing ties the marker to the constant;
D1 may add an inline `const { assert!(!S::CREATE_CONNECTS) }` in `lazy()`, which is a compile error
and not a runtime panic.

D3. **`.migrate(..)` is absent at compile time the same way, through `MigrationLock: Store`.**
`MigrationLock` is the plan's "store-side lock trait": `type Guard`, `lock(&Client, Duration) ->
BoxFuture<Result<Guard, BoxError>>`, `unlock(Guard) -> BoxFuture<()>`. `Migrations<S: Store>` is the
migrator trait, `fn run(self: Arc<Self>, client: S::Client) -> BoxFuture<'static, Result<(), BoxError>>`,
dyn-compatible so the module holds `Arc<dyn Migrations<S>>`. `ulo_db::MIGRATION_LOCK_KEY` is
`3490428366386280370`, checked against the SHA-256 of `"ulo-db/migrations"` (first eight bytes,
big-endian, signed) and `0x30707e1b2c66b7b2`. `Migrations<Postgres> for sqlx::migrate::Migrator` is
orphan-legal because the store type is the local trait parameter.

D4. **`StoreModule<S>` implements `Module` under `S::Options: 'static`.** DESIGN §4.1 bounds
`Options` without `'static`, and `ModuleIdentity::of_value` needs it. A `where` clause on the impl,
not a change to `Store`. The identity is over the options when the section read, and over the
section's path and error when it did not, so equal options at two paths are one module and a failed
read is distinct from a good one.

D5. **`StoreModule::from_options(options)` beside `from_config(&config, path)`,** and
`ulo_config::Section::from_value(path, value)` to build it. The suite builds modules from a host's
options without building a `Config`.

D6. **`.keyed::<Q>()` is `Module::keyed`, not an inherent method.** The plan lists it as a
`StoreModule` method "through `Keyed`"; the trait method already exists, so an inherent one would
only shadow it.

D7. **Store crate module names are type aliases of `StoreModule<S>`,** not newtypes: `SqlxModule<S>`,
`SeaOrmModule`, `DieselModule<S>`, `MongoModule`, `RedisModule`. An alias keeps `.lazy()` and
`.migrate(..)` gated by the store's own marker without a forwarding method per builder step.

D8. **`UnsupportedRuntime` and `MigrationFailed` are `#[non_exhaustive]` structs with public fields**
(`module: &'static str`, `required`, `given`; `module`, `source: BoxError`) and real `Display` and
`Error` impls, which are not panics. The fifth response's Q4 gives the fields; the type is
non-exhaustive so D1 can add one.

D9. **`Policy` is `pub(crate)`** and carries `connect_within`, `retries`, `backoff`. The skeleton sets
the defaults 30 s, `0` and `Duration::ZERO` in `StoreModule::new`; the plan leaves `.retries(..)` and
`.backoff(..)` defaults to D1, who replaces them.

### `ulo-config`

C1. **`Source` is sealed.** The plan names "the source trait and the five sources" and defers remote
sources. An open trait would make `Layer` a public construction surface nobody designed.

C2. **JSON is not behind a feature; only `yaml` is.** The plan's manifest row lists `jsonc-parser`
unconditionally and `saphyr` behind `yaml`. PROBES_config.md §3 suggested a `json` feature; the plan
wins. Constructors: `File::toml`, `File::json`, `File::yaml` (cfg `yaml`), `.optional()`.

C3. **`DotEnv` gains `.prefix(..)` and `.separator(..)` beside `Env`'s.** The design gives `DotEnv::file(..)`
and `.optional()` only, and a `.env` key such as `APP_HTTP__PORT` cannot reach a nested path without
them. Defaults match `Env`'s (`__`, no prefix).

C4. **`Env::var(&self, name) -> Option<String>`,** reading one full name from the map or the process.
`ulo-otel` needs it to read the `OTEL_*` names from a given `Env` (plan, O2).

C5. **`Values::set(path, value: impl Into<Scalar>)`** with `Scalar::{Bool, Integer, Float, String}` and
`From` impls for the integer and float widths, `bool`, `&str` and `String`. The design's
`.set("http.port", 8080)` infers `i32`.

C6. **`Problems::at(path, message)` is public; `at_origin(..)` is `pub(crate)`.** `Problem` exposes
`path()`, `origin()`, `message()`. How a section prefixes its problems with its path is C's.

C7. **`Section<T>` (the section read), `ConfigError`, `Origin`.** `Section::read(&Config, path)`,
`.register(&self, &mut ModuleDef) -> Result<T, ConfigError>` (registers the secrets, answers the value
or the failure for `try_value`), `.value()`, `.path()`. `Origin` is `#[non_exhaustive]`: `File { path,
line, column }`, `Variable { name }`, `Explicit { key }`, `Default`; a `.env` value reports its line
and the value's column.

C8. **`ConfigModule<T>`: `new(&Config)`, `.validate(Fn(&T, &mut Problems))`, `.deny_unread()`;**
identity `of_type::<Self>()`, since the validation closure has no equality.

C9. **`Config`'s and `Tree`'s `Debug` print no value,** `finish_non_exhaustive()`, since a leaf may be
a secret.

### `ulo-health`, `ulo-health-http`, `ulo-health-grpc`

H1. **Names.** `HealthModule::new().lag(Duration)`; `Readiness` with `liveness()`, `readiness()`,
`startup()` (each `async`, answering `Arc<Report>`), `is_stopping()`, `subscribe() -> Changes` and
`Changes::changed()`; `Report::is_up()` and `Report::results()`; `ZeroBound` is the error of a zero
`Check` bound, carrying the check's name. `Health::Down(BoxError)`, see R2.

H2. **`HealthHttp` is imported as a module,** `m.import(HealthHttp::new())`, with `.live_path`,
`.ready_path`, `.started_path` and `.detail(bool)`; it is not a controller. Its `register` mounts the
routes.

H3. **`HealthGrpc` is the bridge module,** `HealthGrpc::new().interval(Bound)`; the interval is set
on the module and a zero is refused at `connect` as `ZeroInterval`. Whether it imports
`GrpcHealth::module()` itself is H2's; the module documentation shows the application importing it.

### `ulo-db-sqlx`

D10. **`permanent_or_not` takes the engine:** `permanent_or_not(Engine, sqlx::Error) -> BoxError`.
The design's `.map_err(permanent_or_not)` has no engine, but the same SQLSTATE means different things
on Postgres, MySQL and SQLite.

D11. **`Engine`, `check_scheme(engine, url)` and `validate_url(engine, url)` are public,** which is the
shared scheme check `ulo-db-seaorm` calls (P3). `Engine` is always compiled; the option types are too,
the stores are behind the engine features.

D12. **Features:** `default = ["tls"]`; `postgres`, `mysql`, `sqlite` off by default. The plan lists
the three engines as features without a default. SQLx's `macros` feature, which `sqlx::migrate!`
needs, is not enabled by this crate; the application enables it.

D13. **Option types carry `max_connections`, `min_connections` and `acquire_timeout_secs`** as
`Option`s, and derive `Debug` through `Secret`, whose `Debug` already prints the URL redacted.

### `ulo-db-seaorm`

D14. **`SeaOrmMigrations<M: MigratorTrait>` wraps a `sea-orm-migration` migrator,** because a foreign
trait cannot be implemented for a generic `M`. The lock guard `SeaOrmLock` holds a `Box<dyn Any +
Send>`; S2a decides whether to reuse D2's primitives, which would need them public.

### `ulo-db-diesel`

D15. **`Given` is `pub` and `#[doc(hidden)]`, not `pub(crate)`.** It is part of the SQLite pool's type,
`Pool<SyncConnectionWrapper<SqliteConnection, Given>>`, which is `Store::Client` of a public impl; a
`pub(crate)` type there is E0446. It has no constructor outside the crate. It exists only with the
`sqlite` feature, because `diesel_async::sync_connection_wrapper` does.

D16. **One options type, `DieselOptions`,** for the three engines, and one guard type `DieselLock`
(`Box<dyn Any + Send>`). `DieselMigrations<M>` wraps any `MigrationSource<DB>`, so it takes
`embed_migrations!()`'s value.

D17. **Features:** `postgres`, `mysql`, `sqlite` off by default; `diesel-async` with `deadpool` and
`migrations`; `deadpool` with `rt_tokio_1` (a new workspace entry, `deadpool` 0.13).

### `ulo-db-mongodb`, `ulo-db-redis`

D18. **MongoDB enables the driver's three default features explicitly,** `compat-3-0-0`, `rustls-tls`
and `dns-resolver`. `rustls-tls` is rustls over ring, the workspace's provider; `dns-resolver` is what
`mongodb+srv` needs. The workspace entry has `default-features = false`.

D19. **Redis's `rediss://` is behind an opt-in `tls` feature,** mirroring `ulo-rpc-redis`'s
(`tokio-rustls-comp`, `tls-rustls-webpki-roots`). The plan lists no TLS feature for the store; without
one a `rediss://` URL validates and fails at connect. The `OnceLock` outcome and `prefer_tokio_once`
are `pub(crate)` stubs in `store.rs`; the plan's file table lists no separate file. The design question
about `prefer_tokio()` being uncallable under `tokio-comp` alone (plan, S2d) is open and unchanged.

### `ulo-db-conformance`

D20. **The host trait is `Host`,** with `Harness`, `Store`, `PARALLEL`, `start`, `options`,
`unreachable_options`, `bad_credentials`, `malformed_options(password)`, `cut`, `restore`, `lazily`,
`migrating`, `capabilities` and `budget`. `lazily` and `migrating` are methods because a generic
scenario cannot call `.lazy()` or `.migrate(..)` on a store without the marker bounds; a store
without the capability returns the module unchanged and declares the scenarios not applicable. The
outage is `cut`/`restore`, which a host implements through the support crate's `Relay`.
`MigrationProbe` counts migration runs. The suite exposes `PASSWORD = "hunter2"`.

D21. **Scenarios: 12 stamped tests, one per case function,** in the ten files of the plan:
`wire::malformed_url_is_refused`, `connect::{unreachable_fails_at_the_bound,
bad_credentials_are_permanent, create_loop}`, `lazy::waits_for_the_first_ping`,
`health::outage_and_recovery`, `close::after_the_drain`,
`stall::connecting_does_not_stall_the_thread` (through `block_on_one_thread`),
`runtime::unsupported_runtime_is_refused`, `keyed::two_instances`, `migrate::{only_on_request,
once_across_replicas}`. The macro is copied from `ulo-rpc-conformance`'s, renamed.

### `ulo-otel`, `ulo-otel-otlp`

O1. **The exporter is handed to `Otel` as a value:** `Otel::new(options, exporter: impl Exporter) ->
Result<Otel, BoxError>`. The plan writes `Otel::new(options)`. `Exporter` has an associated
`Span: SpanExporter` type, because the SDK's trait returns `impl Future` and is not dyn-compatible,
and `build(self, &OtelOptions) -> Result<Span, BoxError>` so a refusal at construction (the rustls
provider case) reaches `main`.

O2. **`OtelOptions`:** `service_name`, `endpoint`, `headers: Option<Secret<String>>` (they carry
credentials), `sample_ratio`, `flush_timeout_ms`; `from_config(&Config, path)` and `from_env(&Env)`.
The `OTEL_*` mapping is O2's.

O3. **`ulo-otel-otlp` takes `ureq` with `rustls-no-provider` and `rustls-webpki-roots`,** and the
workspace `rustls` (ring) for the provider it hands ureq. The plan names neither feature. O3 may take
`platform-verifier` instead, which would match the other links' roots but may reach for a process
default provider.

## Plan or design inconsistencies met

I1. **S-skel's list excludes `ulo-cli`.** The brief for this stage mentions "ulo-cli's template/generate
module skeletons as the plan lists"; the plan's S-skel row lists the tables of C, H1, H2, D1, D2, D3,
S2a-S2d, O2 and O3, and `L` is not among them (`BUILD_PLAN_3.md`, S-skel table, "each crate's
`Cargo.toml` and `src/**` in the tables of ..."), and the file count of 90 equals the 88 skeletons
of those tables plus the two workspace files. `ulo-cli` is untouched.

I2. **DESIGN §4.2 says `ulo-db` registers the password with `m.secret(..)`;** `StoreModule` cannot see
it without D1 above.

I3. **PROBES_config.md §3 says JSON is behind a `json` feature;** the plan's manifest row does not
(C2).


## Not verified

- Every signature that names a library type: `diesel_async::sync_connection_wrapper::SpawnBlocking`
  (read at `diesel-async-0.9.2/src/sync_connection_wrapper/mod.rs:19`), `opentelemetry_http::HttpClient`
  (`lib.rs:137`, `#[async_trait]`), `tracing_opentelemetry::OpenTelemetryLayer<S, T>` and its
  `LookupSpan` bound (`layer.rs:1126`), `sea_orm_migration::MigratorTrait`, `sqlx::migrate::Migrator`,
  `redis::aio::ConnectionManager`, `mongodb::Client`. The paths were read in the registry; none was
  compiled.
- That `OpenTelemetryLayer<S, SdkTracer>` has no bound on its struct definition (read: none).
- That the `serde::forward_to_deserialize_any!` list in `ulo-config/src/de.rs` is accepted with
  `deserialize_newtype_struct` defined by hand beside it (`newtype_struct` is left out of the list).
- That `[$]` and the nested `macro_rules!` in `conformance_suite!` expand as `ulo-rpc-conformance`'s
  do; the text is copied from it.
- That `ulo-db-diesel` with no engine feature compiles: `Given`, the pools and the migrations are all
  behind a feature, so the crate is then empty apart from `DieselOptions` and `DieselLock`.
- `cargo tree -d` showing one SQLx with `ulo-db-sqlx` and `ulo-db-seaorm` together.
- The `Cargo.toml` files were not parsed by a TOML reader (none was available); they were read.
