# ulo-cli

`ulo` creates projects and files for the `ulo` framework and runs a project, rebuilding it when its
source changes.

```bash
cargo install ulo-cli
```

## Create a project

```bash
ulo new my-app
cd my-app
cargo test
cargo run
```

```bash
curl http://127.0.0.1:3000/greetings/Ada
# {"message":"Hello, Ada!"}
```

`ulo new` writes a project from templates compiled into the binary and fetches nothing. The project
depends on `ulo = "=<the CLI's version>"`, and on every other `ulo-*` crate at the same exact
version, so a later release of the framework never meets templates written for an earlier one. Its
tests run offline, with no server and no database.

```bash
ulo new <name> [--transport <transport>] [--db <module>-<backend>]
```

`--transport` defaults to `http`.

| Transport | Serves | Health |
| --- | --- | --- |
| `http` | HTTP/1.1 and HTTP/2 on hyper | `/health/live`, `/health/ready`, `/health/started` |
| `ws` | WebSocket gateway on the HTTP server's port | the same HTTP probes |
| `grpc` | gRPC from `proto/greeter.proto` | `grpc.health.v1` |
| `rpc-tcp`, `rpc-udp` | RPC over TCP or UDP | none |
| `rpc-nats`, `rpc-redis`, `rpc-rabbitmq`, `rpc-mqtt`, `rpc-kafka` | RPC over a broker | none |

`rpc-kafka` builds librdkafka and needs a C toolchain. A broker transport connects to the broker
named in `config/app.toml` when the project starts.

`--db` adds a database layer: the store module imported in `src/app.rs`, a `[database]` section in
`config/app.toml`, and `src/database.rs` holding a service that injects the client.

| `--db` | Client | Rust |
| --- | --- | --- |
| `sqlx-postgres`, `sqlx-mysql`, `sqlx-sqlite` | `sqlx::PgPool`, `MySqlPool`, `SqlitePool` | 1.94 |
| `seaorm-postgres`, `seaorm-mysql`, `seaorm-sqlite` | `sea_orm::DatabaseConnection` | 1.94 |
| `diesel-postgres`, `diesel-mysql`, `diesel-sqlite` | a deadpool `Pool` of diesel-async connections | 1.89 |
| `mongodb` | `mongodb::Client` | 1.88 |
| `redis` | `redis::aio::ConnectionManager` | 1.88 |

The generated manifest states the Rust version the combination needs. Migrations are not
generated: each store module takes them through `.migrate(..)`.

### What a project holds

```text
config/app.toml     the address to serve on, and the database URL with --db
src/main.rs         loads the configuration, wires the application, serves until SIGINT or SIGTERM
src/app.rs          AppConfig and the root module
src/greeter.rs      the service the handlers share
src/controller.rs   the transport's handlers (src/gateway.rs for ws)
src/database.rs     with --db
```

Every file with logic carries a test, and `src/app.rs` wires the whole application in one.
Environment variables named `APP_<SECTION>__<KEY>` override `config/app.toml`.

## Generate a file

```bash
ulo generate <module|provider|controller|gateway|resolver> <name> [--transport <http|rpc>]
```

`ulo generate` writes one new file under `src/` and prints the lines that register it. It edits no
existing source, since rewriting a file loses its formatting and comments. It refuses to overwrite a
file and refuses a name that is not a Rust identifier.

| Kind | File | Item |
| --- | --- | --- |
| `module` | `src/<name>_module.rs` | `<Name>Module` |
| `provider` | `src/<name>.rs` | `<Name>` |
| `controller` | `src/<name>_controller.rs` | `<Name>Controller`; `--transport rpc` for RPC handlers, HTTP otherwise |
| `gateway` | `src/<name>_gateway.rs` | `<Name>Gateway` |
| `resolver` | `src/<name>_resolver.rs` | `<Name>Resolver`, an `async-graphql` root object |

```bash
ulo generate controller user_profile
# Created src/user_profile_controller.rs
#
# Declare it where the project's modules are, usually src/main.rs:
#     mod user_profile_controller;
#
# Register it in the module that serves it:
#     m.controller::<crate::user_profile_controller::UserProfileController>();
```

A kind that needs a crate the project may not have prints the `Cargo.toml` line for it. A generated
file carries a test; the `provider` and `resolver` tests use `tokio`'s test macro, which a
`ulo new` project already depends on.

## Run with rebuild

`ulo dev` runs the project and restarts it when a `.rs` or `.toml` file changes.

```bash
ulo dev
ulo dev --release -p my-app
ulo dev --listen 127.0.0.1:3000
```

`--listen` holds the socket across rebuilds and hands it to the child as file descriptor 3 through
`LISTEN_FDS` (Unix only); the application adopts the inherited socket.

## License

MIT.
