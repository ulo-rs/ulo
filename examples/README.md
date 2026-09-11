# ulo examples

Each file here runs on its own with `cargo run --example <name>`. Start with
[hello_world.rs](hello_world.rs) for the smallest working app, or with the guide below if you are
arriving from NestJS.

## Parsing, validating and refusing input

[validation_complete_guide.rs](validation_complete_guide.rs) is the one to read if you are looking
for pipes. ulo has no pipe: what `PipeTransform` does — receive the value a handler is about to be
given, then reshape or refuse it — is what an extractor does here, so the rules that apply are
visible in the handler's signature.

```bash
cargo run --example validation_complete_guide
```

It serves HTTP on `127.0.0.1:3000`, RPC on `127.0.0.1:3001` and a WebSocket gateway at `/ws`, and
covers typed path and query parameters, serde defaults, `Validated<Json<T>>`, delimited lists,
newtypes that validate during deserialisation, a hand-written extractor, a guard that refuses on
policy, an interceptor that answers in place of the handler, and `Validated<Payload<T>>` on
WebSocket and RPC. The file itself carries the NestJS mapping table and a TypeScript snippet beside
each Rust one.

## The rest, by subject

| Subject | Examples |
| --- | --- |
| Getting started | `hello_world`, `provider_patterns`, `derive_injectable` |
| Request lifecycle | `middleware_examples`, `error_handling`, `error_telemetry`, `route_metadata` |
| Extraction | `validation_complete_guide`, `custom_extractors`, `file_upload` |
| Configuration | `config_module`, `config_validation` |
| WebSocket | `websocket_chat`, `websocket_rooms`, `websocket_di`, `gateway_http_bridge` |
| RPC | `rpc_controller`, `rpc_udp`, `rpc_nats`, `rpc_nats_client`, `rpc_streaming`, `rpc_tracing` |
| gRPC | `grpc_service`, `grpc_client` |
| Streaming | `sse` |
| Lifecycle and operations | `lifecycle_hooks`, `graceful_shutdown`, `health_checks`, `logging` |
| Scoping | `request_scoped_context`, `multi_protocol_context` |
| Adapters | `salvo_poc`, `poem_poc`, `rocket_poc` |
| Deployment | `socket_activation` |

`middleware_examples` collects reference implementations — logging, CORS, bearer auth, timeouts,
compression, rate limiting. They illustrate the shape rather than being production-ready.

## GraphQL

The GraphQL examples live in their own crates rather than here. Both `async-graphql` and `juniper`
emit crate-anchored paths from their derive macros (`::async_graphql`, `::juniper`), which resolve
only against a direct dependency — so an example using either has to sit where that dependency is
declared.

```bash
cargo run -p ulo-graphql-async-graphql --example hello_world
cargo run -p ulo-graphql-juniper --example hello_world
```

| Example | Shows |
| --- | --- |
| [async-graphql / hello_world](../crates/ulo-graphql-async-graphql/examples/hello_world.rs) | A schema mounted at `/graphql` with the playground |
| [async-graphql / with_auth](../crates/ulo-graphql-async-graphql/examples/with_auth.rs) | A `ContextBuilder` reading a bearer token off the request, so every resolver sees the caller |
| [async-graphql / subscriptions](../crates/ulo-graphql-async-graphql/examples/subscriptions.rs) | `graphql-transport-ws` over the WebSocket path |
| [juniper / hello_world](../crates/ulo-graphql-juniper/examples/hello_world.rs) | The same mount, on juniper |
| [juniper / with_auth](../crates/ulo-graphql-juniper/examples/with_auth.rs) | The same context, on juniper, alongside an injected service |

## Adding one

Write the file, add a `[[example]]` entry to [Cargo.toml](Cargo.toml), and name it in a table
above. `coverage_ledger.rs` in the integration-test crate fails on an example that is missing
either — an example `cargo run --example` cannot reach, or one no index points at, is an example
nobody runs.

An example answers one question a user would ask, which decides both what goes in it and
whether it belongs here or in a crate of its own:
[Where a test goes, and what earns one](../docs/explainers/testing-and-examples.md).

## Related

- [Architecture decision records](../docs/adr/README.md) — why the framework is built the way it is
- [`validator` crate docs](https://docs.rs/validator/) — the validation attributes `Validated<E>` runs

## License

MIT, same as ulo.
