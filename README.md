# ulo

A Rust framework for server applications that serve more than one kind of client. HTTP routes,
WebSocket gateways, RPC handlers, gRPC services and GraphQL resolvers are declared on plain structs,
share one dependency-injection container, one configuration, one set of health probes and one
shutdown, and run on a runtime and a server you choose.

The core owns no async runtime and no socket. A runtime crate (`ulo-runtime-tokio`,
`ulo-runtime-smol`) and a transport crate (`ulo-http-hyper`, `ulo-rpc-nats`, ...) are added beside it,
so the same application module runs on either runtime and behind any server. A runtime written
outside this repository runs `ulo-runtime-conformance`, the suite both runtimes pass.

## Install

`0.1.0-alpha.3` is the current release, on crates.io. Each crate is pinned with `=`, since a caret
on a pre-release also takes a later alpha, which may break this one:

```toml
[dependencies]
ulo = "=0.1.0-alpha.3"
ulo-http = "=0.1.0-alpha.3"
ulo-http-hyper = "=0.1.0-alpha.3"
ulo-runtime-tokio = "=0.1.0-alpha.3"
serde = { version = "1", features = ["derive"] }
```

Or start from a template. `ulo new` writes a working application with health probes and tests; a
pre-release is installed by naming its version:

```bash
cargo install ulo-cli --version 0.1.0-alpha.3
ulo new my-app && cd my-app && cargo run
```

To try it without building on your own machine, open the repository in a
[Codespace](https://codespaces.new/ulo-rs/ulo), which has the CLI installed and opens on the
folder above the repository, so a project created there sits beside the source:

```bash
ulo new my-app && cd my-app && cargo run
```

A throwaway container works too. The application listens on `127.0.0.1` by default, which a
published container port cannot reach, so the address is overridden:

```bash
docker run --rm -it -p 3000:3000 rust:1 bash
cargo install ulo-cli --version 0.1.0-alpha.3
ulo new my-app && cd my-app
APP_SERVER__ADDRESS=0.0.0.0:3000 cargo run
```

## Quick start

A module, one controller with one route, and `run`, which serves it. This is `examples/01-hello`
in one file.

```rust,no_run
use ulo::{controller, handlers, module};
use ulo_http_hyper::Server;

#[controller]
pub struct HelloController;

#[handlers]
impl HelloController {
    #[ulo_http::get("/")]
    fn hello(&self) -> &'static str {
        "Hello, ulo!"
    }
}

#[module(controllers = [HelloController])]
pub struct AppModule;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_runtime_tokio::run(AppModule, Server::new("127.0.0.1:3000"))?;
    Ok(())
}
```

`cargo run`, then `curl http://127.0.0.1:3000/`. With no `tracing` subscriber installed, `run`
writes `listening on 127.0.0.1:3000 (http)` to stderr; example 01 installs one, which logs the same
address with every other `info` event. `run` builds a tokio runtime and starts the
application in five phases: `wire` checks the whole module graph and reports every problem at once,
`connect` builds the providers and runs their hooks, `bind` and `listen` acquire the sockets, and
`serve` runs until Ctrl-C or SIGTERM, then drains and closes. An application that sets a knob, a
drain timeout or a test clock, writes the phases itself, starting from
`App::builder(AppModule).runtime(Tokio::current())` (example 10).

A handler names what it needs in its signature. Extractors read and validate the request, and a
provider is injected as `Dep<T>` (`examples/15-http-requests` is the full version of this):

```rust
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ulo::{Dep, controller, handlers, injectable, module};
use ulo_http::{Created, Json, Query, Valid, Validate};

#[derive(Clone, Serialize)]
pub struct Note {
    pub id: u32,
    pub title: String,
}

#[derive(Deserialize, Validate)]
pub struct NewNote {
    #[validate(length(min = 1, max = 20))]
    pub title: String,
}

#[derive(Deserialize)]
pub struct Page {
    pub limit: Option<usize>,
}

#[injectable]
pub struct Notes {
    #[injectable(default)]
    all: Mutex<Vec<Note>>,
}

#[controller]
pub struct NotesController {
    notes: Dep<Notes>,
}

#[handlers]
impl NotesController {
    #[ulo_http::post("/notes")]
    fn create(&self, Valid(Json(new)): Valid<Json<NewNote>>) -> Created<Json<Note>> {
        let mut all = self.notes.all.lock().unwrap();
        let note = Note { id: all.len() as u32 + 1, title: new.title };
        all.push(note.clone());
        Created::at(format!("/notes/{}", note.id), Json(note))
    }

    #[ulo_http::get("/notes")]
    fn list(&self, Query(page): Query<Page>) -> Json<Vec<Note>> {
        let all = self.notes.all.lock().unwrap();
        Json(all.iter().take(page.limit.unwrap_or(usize::MAX)).cloned().collect())
    }
}

#[module(providers = [Notes], controllers = [NotesController])]
pub struct NotesModule;
# fn main() {}
```

## What it serves

| Transport | Crates |
| --- | --- |
| HTTP | `ulo-http` with a server: `ulo-http-hyper` (the reference), or `ulo-http-axum`, `ulo-http-actix`, `ulo-http-poem`, `ulo-http-rocket`, `ulo-http-salvo`. TLS through `ulo-net`. |
| WebSocket | `ulo-ws` with `ulo-ws-hyper`; `ulo-ws-redis` carries a broadcast across processes. |
| RPC | `ulo-rpc` over TCP (`ulo-rpc-tcp`, `ulo-rpc-tcp-smol`), UDP (`ulo-rpc-udp`), NATS, Redis, MQTT, RabbitMQ or Kafka (`ulo-rpc-nats`, `-redis`, `-mqtt`, `-rabbitmq`, `-kafka`). Each link passes `ulo-rpc-conformance`, the suite a link written outside this repository runs too. |
| gRPC | `ulo-grpc`, with `ulo-build` generating the service shapes from a `.proto`. |
| GraphQL | `ulo-graphql` over HTTP and WebSocket, with `ulo-graphql-async-graphql` or `ulo-graphql-juniper`. |

Around them: `ulo-config` (layered configuration), `ulo-health` with `ulo-health-http` and
`ulo-health-grpc` (probes), `ulo-db` with `ulo-db-sqlx`, `-seaorm`, `-diesel`, `-mongodb` and
`-redis` (a database as a module), `ulo-otel` with `ulo-otel-otlp` (trace context across
transports), and `ulo-cli` (`ulo new`, `ulo generate`, `ulo dev`).

## Will it work for me

- **Rust 1.88** is the floor for the workspace. Five crates need more: `ulo-http-salvo` 1.92,
  `ulo-graphql-async-graphql` and `ulo-db-diesel` 1.89, `ulo-db-sqlx` and `ulo-db-seaorm` 1.94.
- **Runtimes:** tokio and smol. Which one an application runs on is a line in `main`; example 22
  is example 01 on smol.
- **Without macros:** every macro expands to calls on a public value API, so an application can be
  written without them (example 14).
- **Testing:** `TestApp` builds the production module graph with parts replaced (example 13).
- `ulo-rpc-kafka` builds librdkafka and needs a C toolchain.

## Where to go next

- The documentation site, <https://ulo-rs.vercel.app/>, is the guide: getting started, core
  concepts, each transport, testing, deployment and recipes.
- [`examples/`](examples) holds one runnable application per concept, below. Run one with
  `cargo run -p ex-01-hello`; the directory's README says what each needs.
- `cargo doc --open` renders the API docs.
- [`docs/experiments/di-redesign/DESIGN.md`](docs/experiments/di-redesign/DESIGN.md) is the design
  rationale: keys, bindings, scopes, executions, modules, lifecycle, and each transport's contract.

| Example | Shows |
| --- | --- |
| `01-hello` | One module, one controller, one route, served by `run` |
| `02-providers` | The two ways to declare a provider, injected as `Dep<T>` |
| `03-traits-and-qualifiers` | Binding a provider under a trait, and qualifiers for two instances of one type |
| `04-collections-and-optional` | Many providers under one key (`Many<dyn T>`) and an optional dependency |
| `05-scopes` | `singleton`, `execution`, `transient` and the default |
| `06-modules` | Imports, exports and visibility |
| `07-configured-modules` | A global module, `for_root(value)`, one module imported twice under two qualifiers |
| `08-dynamic-and-lazy` | A module built at runtime and a module loaded after startup |
| `09-lifecycle` | Every hook kind in the order it runs, and a readiness check with retries |
| `10-shutdown` | Stop accepting, drain, close, and the two timeouts |
| `11-wiring-report` | `wire()` collecting every wiring problem into one report |
| `12-standalone` | An application with no server, run as a job |
| `13-testing` | `TestApp` with parts replaced |
| `14-without-macros` | Example 06 written against the value API |
| `15-http-requests` | Extractors, validation, and typed errors on HTTP |
| `16-http-enhancers` | Guards, interceptors and an error handler at three tiers |
| `17-http-pre-dispatch` | CORS, a tower layer and middleware that run before routing |
| `18-http-streaming` | Streamed bodies and Server-Sent Events |
| `19-http-uploads` | Multipart uploads under body limits |
| `20-http-tls` | HTTPS from a PEM chain and key |
| `21-http-in-axum` | A ulo app mounted inside an axum server |
| `22-http-on-smol` | Example 01 on smol |
| `23-ws-gateway` | A WebSocket gateway: replies, guards, connect and disconnect hooks |
| `24-ws-rooms` | Rooms and broadcast |
| `25-rpc-basics` | An RPC controller over TCP: messages, events and error kinds |
| `26-rpc-streams` | Streamed requests, streamed replies and duplex calls |
| `27-rpc-deadlines` | A deadline that travels with the call |
| `28-rpc-links` | The same handlers over TCP, NATS, Redis, MQTT, RabbitMQ or Kafka |
| `29-grpc-service` | A gRPC service from a `.proto` |
| `30-grpc-client` | A generated client as an ordinary provider |
| `31-graphql` | async-graphql over HTTP with an execution-scoped context |
| `32-graphql-subscriptions` | Subscriptions over `graphql-transport-ws` |
| `33-config` | Layered configuration |
| `34-health` | Liveness, readiness and startup probes |
| `35-db-sqlx` | A database as a module, with migrations at connect |
| `36-db-two-instances` | Two databases of one kind |
| `37-db-diesel` | A blocking driver beside an async runtime |
| `38-tracing` | W3C trace context across HTTP, RPC and gRPC |
| `39-one-app` | HTTP, WebSocket, RPC and gRPC in one application |
| `40-socket-activation` | Serving on a socket a supervisor opened |
| `41-one-guard-two-transports` | One guard on an HTTP route and an RPC message |

## License

MIT. See [LICENSE](LICENSE).
