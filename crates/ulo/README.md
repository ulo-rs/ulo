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

The repository, <https://github.com/ulo-rs/ulo>, holds the design in
`docs/experiments/di-redesign/DESIGN.md` and forty-one runnable examples in `examples/`, one concept
each, from `01-hello` to `41-one-guard-two-transports`. `cargo doc --open` renders the API docs.

## License

MIT.
