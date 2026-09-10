# #128 — refactor(adapter)!: name SPI registration methods for what they register

Merged 2026-07-26 into `master` from `refactor/adapter-spi-registration-names`, commit [`17519b0`](https://github.com/ulo-rs/ulo/commit/17519b0a5bfac8aaf5a1fcf878b3ae623687a0f7).

Renames the adapter SPI registration methods off `bind` and reserves that word for socket acquisition. In Rust, `bind` means acquiring a socket; on this SPI it named registration, while the actual socket work happened in `into_lifecycle` — or, for gRPC alone, inside `bind` itself.

| Trait method | Now |
|---|---|
| `HttpAdapter::bind` / `bind_ws` | `register_route` / `register_ws_route` |
| `RpcAdapter::bind` | `register_handlers` |
| `WebSocketAdapter::bind` | `register_gateway` |
| `GrpcAdapter::bind` | `register_services` |

`into_lifecycle` / `into_lifecycle_handles` keep their names: consume-the-configured-adapter → self-contained lifecycle handle is the one contract uniform across transports, while the effect inside differs (listener transports bind there; brokered transports connect to their broker). Any effect verb would misname part of the fleet.

- **core** — trait definitions, callsites, rustdoc links, and log wording; messages describing genuine socket binding are untouched.
- **adapters** — impl renames across the five HTTP crates, tungstenite, and the seven RPC transports. `toni-grpc` additionally moves socket acquisition from registration into `into_lifecycle`, making `register_services` registration-only like its siblings and dropping the `listener`/`local_addr` carry fields; port-in-use still surfaces as `Err` from `app.bind()`, which awaits `into_lifecycle`.
- **docs** — ADR 0009 records the naming rule (a method is named for what it does at its site; a shared name requires shared semantics) together with the rename history on this surface that motivated making it explicit.
- **doctests** — four pre-existing `rust,no_run` examples in toni-nats/toni-tcp/toni-udp reference free variables or unimported macros and never compiled; CI runs no adapter-crate doctests, so `cargo test --workspace` was the first gate to trip on them. Fenced `ignore`, matching #115.

Breaking for adapter implementors only; the application-facing API is unchanged. Full workspace tests and doctests are green.
