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
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Or start from a template. `ulo new` writes a working application with health probes and tests; a
pre-release is installed by naming its version:

```bash
cargo install ulo-cli --version 0.1.0-alpha.3
ulo new my-app && cd my-app && cargo run
```

To try it without building on your own machine, open the repository in a
[Codespace](https://codespaces.new/ulo-rs/ulo), which has the CLI installed, or use a throwaway
container. The application listens on `127.0.0.1` by default, which a published container port
cannot reach, so the address is overridden:

```bash
docker run --rm -it -p 3000:3000 rust:1 bash
cargo install ulo-cli --version 0.1.0-alpha.3
ulo new my-app && cd my-app
APP_SERVER__ADDRESS=0.0.0.0:3000 cargo run
```

## Quick start

A module, one controller with one route, and the phases that start an application. This is
`examples/01-hello` in one file.

```rust,no_run
use ulo::{App, controller, handlers, module};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::{Tokio, shutdown_signal};

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

The documentation site, <https://ulo-rs.vercel.app/>, is the guide. The repository,
<https://github.com/ulo-rs/ulo>, holds forty-one runnable examples in `examples/`, one concept each,
from `01-hello` to `41-one-guard-two-transports`, and the design rationale in
`docs/experiments/di-redesign/DESIGN.md`.

## License

MIT.
