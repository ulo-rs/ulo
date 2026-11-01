# ulo

A Rust framework for server applications that serve more than one kind of client. HTTP routes,
WebSocket gateways, RPC handlers, gRPC services and GraphQL resolvers are declared on plain structs,
share one dependency-injection container, one configuration, one set of health probes and one
shutdown, and run on a runtime and a server you choose.

The core owns no async runtime and no socket. A runtime crate (`ulo-runtime-tokio`,
`ulo-runtime-smol`) and a transport crate (`ulo-http-hyper`, `ulo-rpc-nats`, ...) are added beside it,
so the same application module runs on either runtime and behind any server.

## Install

The `v0.1.0-alpha.1` tag is the current release. Depend on it from git:

```toml
[dependencies]
ulo = { git = "https://github.com/ulo-rs/ulo", tag = "v0.1.0-alpha.1" }
ulo-http = { git = "https://github.com/ulo-rs/ulo", tag = "v0.1.0-alpha.1" }
ulo-http-hyper = { git = "https://github.com/ulo-rs/ulo", tag = "v0.1.0-alpha.1" }
ulo-runtime-tokio = { git = "https://github.com/ulo-rs/ulo", tag = "v0.1.0-alpha.1" }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Quick start

A module, one controller with one route, and the phases that start an application. This is
`examples/01-hello` in one file.

```rust,no_run
use ulo::{App, injectable, module, routes};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

#[injectable]
pub struct HelloController;

#[routes]
impl HelloController {
    #[ulo_http::get("/")]
    fn hello(&self) -> &'static str {
        "Hello, ulo!"
    }
}

#[module(controllers = [HelloController])]
pub struct AppModule;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::builder(AppModule)
        .runtime(Tokio::current())
        .wire()?
        .connect()
        .await?
        .bind(Server::new("127.0.0.1:3000"))
        .listen()
        .await?;
    for bound in app.addresses() {
        println!("listening on http://{}", bound.addr);
    }
    app.serve(shutdown_signal()).await?;
    Ok(())
}
```

`cargo run`, then `curl http://127.0.0.1:3000/`. `wire` checks the whole module graph and reports
every problem at once, `connect` builds the providers and runs their hooks, `listen` acquires the
sockets, and `serve` runs until the signal future resolves, then drains.

A handler names what it needs in its signature. Extractors read and validate the request, and a
provider is injected as `Dep<T>` (`examples/15-http-requests` is the full version of this):

```rust
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ulo::{Dep, injectable, module, routes};
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

#[injectable]
pub struct NotesController {
    notes: Dep<Notes>,
}

#[routes]
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
| RPC | `ulo-rpc` over TCP (`ulo-rpc-tcp`, `ulo-rpc-tcp-smol`), UDP (`ulo-rpc-udp`), NATS, Redis, MQTT, RabbitMQ or Kafka (`ulo-rpc-nats`, `-redis`, `-mqtt`, `-rabbitmq`, `-kafka`). |
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

- [`docs/experiments/di-redesign/DESIGN.md`](docs/experiments/di-redesign/DESIGN.md) is the design:
  keys, bindings, scopes, executions, modules, lifecycle, and each transport's contract.
- [`examples/`](examples) holds one runnable application per concept, below. Run one with
  `cargo run -p ex-01-hello`; the directory's README says what each needs.
- `cargo doc --open` renders the API docs.

| Example | Shows |
| --- | --- |
| `01-hello` | One module, one controller, one route, and the five phases that start an application |
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
