# race 3, L: `ulo-cli` templates

Nothing was compiled or run. Every template and every line of the CLI was read for syntax, types,
borrows, visibility and imports against the crates' sources and the skeletons' signatures. The
facts a read could not settle are listed under "Not verified".

## Points resolved

- **The names `--transport` and `--db` accept.** `--transport`: `http` (the default), `ws`, `grpc`,
  `rpc-tcp`, `rpc-udp`, `rpc-nats`, `rpc-redis`, `rpc-rabbitmq`, `rpc-mqtt`, `rpc-kafka`. `--db`,
  `<module>-<backend>` where the module has backends: `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite`,
  `seaorm-postgres`, `seaorm-mysql`, `seaorm-sqlite`, `diesel-postgres`, `diesel-mysql`,
  `diesel-sqlite`, and `mongodb` and `redis` alone, which have none. Both are clap `ValueEnum`s, so
  the names are checked at parse time and `--help` lists them. `http` means the hyper backend:
  `ulo-http-axum` and the other four are embeddings the host's own `main` drives, and no template
  is written for them.
- **The no-tests count is not written here.** The count is a CI script's. The request is to factor
  `RAN` and the counting loop of `scripts/unsafe_allowed.py miri` into an importable function the
  matrix script shares, so "ran a test" has one definition. Request 2.
- **The `resolver` template depends on `async-graphql` and on nothing of `ulo`.** It is a plain
  `#[Object]` root object with one field, `#[derive(Default)]`, to be merged into the application's
  query root with `#[derive(MergedObject)]`. The engine (`ulo-graphql-async-graphql`) reads the
  schema as `Dep<S>` wherever the application builds it, and the generator knows neither that
  schema nor its context type, so the template reaches for no `Dep`. The file's doc comment names
  the merge; `ulo generate` prints the `async-graphql = "7"` line the project needs. `juniper` has
  no template.
- **Every `ulo-*` dependency of a template is pinned exactly, not only `ulo`.** Each is
  `=<the CLI's version>`. The workspace's crates all carry `version.workspace`, and the sibling
  crates already pin one another with `=0.1.0-alpha.1`, so a caret on any would let that crate move
  alone against templates written for the others. The risk is a release that gives `ulo-cli` a
  version no sibling carries (`CLAUDE.md` records `ulo-cli` at 0.1.2 against the framework's 0.2.0
  on `master`): every pin then names a version that does not exist. Releasing the crates together,
  as the workspace has them, avoids it; a per-crate version table in the CLI would not.

## Decisions the designs leave open

- **The layout is flat.** `src/main.rs`, `src/app.rs`, `src/greeter.rs`, `src/controller.rs` (or
  `src/gateway.rs`), `src/database.rs` with `--db`, `config/app.toml`. The `master` template's
  `src/app/` directory with `#[path = ".."]` attributes existed so `ulo generate resource` could
  edit them; with no edits there is nothing for it to serve.
- **Every `new` project has the same spine.** A `Greeter` service, one handler on the chosen
  transport that calls it, an `AppModule` holding the `Config`, `ConfigModule::<AppConfig>`, and
  `config/app.toml` read in `main` under `Env::process().prefix("APP_").separator("__")`. The server
  address comes from the configuration (`server.address`, `server.url` for four brokers,
  `server.brokers` for Kafka), which is how DESIGN §2.2's `main` shape is shown once, in the
  project a user starts from.
- **`main` reads the address after `connect()`.** `app.get::<AppConfig>()` on the connected app
  hands the section, and `bind(..)` follows. A bad configuration is reported by `wire()` before the
  address is needed.
- **Health is wired where a transport has a probe protocol.** `http` and `ws`: `HealthModule` and
  `HealthHttp`. `grpc`: `HealthModule`, `GrpcHealth::module()` and `HealthGrpc`, the order the
  bridge's documentation gives. RPC: none, since no probe protocol rides it.
- **Tests.** Each file that holds logic holds a test, none needing a network, a server or a
  database: `Greeter` by a unit test; each handler file by a direct call of the handler method
  with the injected `Dep::from_arc(Arc::new(Greeter))` (`#[routes]` leaves the method as written,
  per `ulo-handler-codegen/src/emit.rs`); `src/app.rs` by `TestApp::of(AppModule::new(config))
  .runtime(Tokio::current()).wire()`, which checks the whole graph, the configuration and the store
  module's option parsing included, without connecting. No test connects, so the same tests run with
  every `--db` and against every broker transport.
- **`generate` paths and the registration lines.** Files go to `src/<snake>[_suffix].rs`; the
  registration is printed with `crate::` paths so it resolves in whatever module serves it, and
  the `mod <stem>;` line is printed apart because the CLI edits no file. `provider` takes no suffix
  (`ulo generate provider mailer` writes `Mailer`); the other four take one. A name whose snake
  spelling is a keyword (`Type` becomes `type`) is refused, since it cannot name the module.
- **`generate controller --transport <http|rpc>`.** The design names five kinds and no flag. A
  controller exists on every transport, and a project made with `--transport rpc-tcp` has no
  `ulo-http`, so an HTTP-only template would not compile there. The default is `http`; `grpc` is not
  offered, since a gRPC handler binds to a marker `ulo-build` generates from a proto the CLI cannot
  know. The flag is refused on any other kind.
- **The generated database service ends in an `#[allow(dead_code, reason = ..)]`.** The project is a
  binary, so an unused `pub fn` warns, and DESIGN §5 holds generated code to `clippy -D warnings`.
  The service exists to show how a client is injected; the allow names that.
- **`ulo new` removes what it wrote on a failed write.** The root is made with `create_dir`, which
  refuses an existing directory atomically, so the cleanup only removes a directory this run made.
- **A project name that equals a dependency's crate name, a Rust keyword, `std`, `core`, `alloc`,
  `proc_macro` or `test`, or that starts with a non-letter, is refused.** Cargo refuses the first
  few at build time with a worse message.
- **The gRPC template builds no client** (`ulo_build::configure().build_client(false)`): the project
  serves one service and calls none, and an unused generated client is dead code in a binary.

## Divergences

- **`ulo new <name>` with no flags writes a different project from `master`'s.** It was a
  controller/service/module triple on `ulo-http-axum` with `UloFactory`. It is now the `http`
  template above. Not carried over: the `resource` generator, `update_app_module`,
  `update_mod_file`, the regex that edited an `imports: [..]` list, `to_pascal_case` (unused on
  `master` too), and the `{{project_name}}` / `{{version}}` placeholder syntax. The templates use
  `@@name@@` slots and the CLI refuses to write a file with one unfilled, because `{{` is Rust
  syntax in a format string.
- **`regex` is removed from `crates/ulo-cli/Cargo.toml`.** Only the deleted `add_module_to_imports`
  used it. `Cargo.lock` needs the matching refresh (request 1).
- **`ulo generate resource` is gone**, as DESIGN §5 has it. The design names no replacement for the
  CRUD set it wrote; the five kinds write one empty handler each.
- **`generate` has no `--dir`** and writes to `src/` of the current directory, refusing when there
  is none. The design is silent.
- **The `rpc-udp` template is the TCP template with another link.** UDP refuses a streamed shape in
  `prepare`, and the template has only a unary handler, so the combination holds.
- **The DESIGN names `--db <module>-<backend>`; `mongodb` and `redis` take no backend.** The
  pattern's second half does not exist for them.
- **`ulo new` prints no emoji.** `master`'s output opened with a check-mark symbol.
- **The CLI itself carries no tests** (rule 2). The templates' tests are the tests.

## Not verified

A fact I could not settle by reading the sources the stage had. Each is checked by the first
compile or the first run of the template matrix.

- `Env::process().prefix("APP_").separator("__")` mapping `APP_SERVER__ADDRESS` to `server.address`,
  case included. `config/app.toml`'s comment and the README state it. `ulo-config` at this stage
  was a skeleton with `todo!()` bodies when I read it.
- `Values::new().set("server.address", ..)` creating the nested path `server.address`; the
  skeleton's docs say dotted paths, the body was not written when I read it.
- `diesel-sqlite`'s URL: the skeleton's doc says "a SQLite path", so the template starts with
  `app.db` and the test uses `:memory:`. `Store::validate` for Diesel's SQLite was `todo!()`.
- `sea-orm = { version = "2.0", default-features = false }` leaving `DatabaseConnection::ping`
  available: it is behind the driver features `ulo-db-seaorm` enables, which unify in.
- `mongodb = "3.9"` at default features beside `ulo-db-mongodb`'s explicit feature set resolving to
  one driver with the `bson` `doc!` macro at `mongodb::bson::doc`.
- `redis::cmd("PING").query_async::<String>(&mut connection)`: read in redis 1.7.1
  (`cmd.rs:648`, one type parameter); `ConnectionManager: Clone` and `ConnectionLike` read in
  `aio/connection_manager.rs`.
- A plain `Vec<String>` return from an `#[ulo_rpc::message]` handler (the generated `controller_rpc`
  template) taking the serialize-a-value arm of `ulo_rpc::__private`'s reply probe. A bare struct
  does, per that module's header; `Vec<String>` was not read.
- Sync `fn` handlers under `#[ulo_http::get]`, `#[ulo_ws::message]` and `#[ulo_grpc::method]`:
  each appears in a test of its crate. A sync `fn` under `#[ulo_rpc::message]` has none, so the RPC
  templates use `async fn`.
- `Path<String>` on a one-parameter route as a scalar: read from `Path`'s documentation.
- That `clippy -D warnings` passes on every generated project, the code `prost` writes in
  `mod pb` included, and that `cargo fmt --check` does: the imports are sorted as rustfmt sorts
  them, and nothing else about layout was checked.
- `HealthGrpc`'s init hook and `GrpcHealth::module()` order: the bridge's own documentation gives
  `HealthModule`, `GrpcHealth::module()`, `HealthGrpc`, which the template follows.
- That `rust-version` in the generated manifest (1.88, 1.89 for Diesel, 1.94 for SQLx and SeaORM)
  matches the crates' floors: copied from the manifests of this tree.

## Requests

1. **`Cargo.lock`** refreshes for `ulo-cli` (the `regex` removal). Not mine to edit.
2. **The template matrix CI job** (owed, the coordinator's), written against what `ulo new` does:
   - The generated crates are not on crates.io, so each generated manifest needs a
     `[patch.crates-io]` section pointing every `ulo-*` crate at the workspace. The CLI has no flag
     for this and gets none: the exact pins are the product.
   - Full cross: 10 transports times 12 database choices (none and 11) is 120 projects. Suggested:
     every transport with no database (10), and every database on `http` (11); the database layer
     touches `app.rs`, `database.rs`, the manifest and `config/app.toml` only, and no transport's
     file. `rpc-kafka` builds librdkafka and wants `cmake`, as `ci.yml`'s `check` job already
     arranges for the broker crates.
   - Per project: `cargo test --locked`, then `cargo clippy --all-targets --locked -- -D warnings`
     on the pinned toolchain of the `generated code lints` job.
   - The no-tests count: fail a project whose `cargo test` output has no `test <name> ... ok`
     line, counting by the regex `scripts/unsafe_allowed.py` uses (`RAN`). Factor `RAN` and the
     loop into an importable function.
   - `ulo generate` in the same job: after `ulo new`, run each of the five kinds (the controller
     with both transports), append the printed `mod ..;` to `src/main.rs`, then `cargo test` and
     `cargo clippy`. The registration line is not needed for the file's own test; an unregistered
     item is dead code in a binary, which clippy reports, so the job registers it first (a
     `sed` into `src/app.rs` before the closing brace of `register`) or runs clippy without
     `-D dead_code`. The `gateway` needs `ulo-ws` and the `resolver` `async-graphql` in the
     manifest, as `generate` prints.
