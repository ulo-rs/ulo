# #227 — Remove the error observer

Merged 2026-09-01 into `master` from `refactor/retire-error-observer`, commit [`f89085b`](https://github.com/ulo-rs/ulo/commit/f89085b11218111194a3aad1e27b642eaa599027).

The error-handler chain becomes the only error seam. `ErrorObserver`, `use_global_error_observer` and the fan-out threaded through the HTTP, RPC, WebSocket and gRPC dispatchers are removed. A handler that returns `None` sees every error the chain sees and shapes nothing, which is the observation the trait offered.

`fan_out_observers(...).await` ran on the error path, one observer at a time, before the response was built. A reporter that reached for the network sat in front of the client's answer. Taking it off that path needs a runtime handle in core or a post-send hook threaded through four adapter boundaries. Publishing onto a transport the app already speaks needs neither, and outlives the process.

- **Core** — the trait, the factory method, the container slot, and the `error_observers` field on the four dispatchers. Two segments sit below the chain and reach no handler: a panicking `#[catch]` handler and a panicking renderer. Each logs at error level, on by default rather than opt-in behind a registration that no default app makes.
- **Errors** — `Cancelled` goes with the trait that was its only delivery. `Drop` is synchronous and `observe` was async, so the one place that knows a client gave up could never raise it. The cancellation token stays the signal, and a holder can act on it.
- **Example** — `examples/error_telemetry.rs`: a declining handler publishes onto RabbitMQ, an `#[event_pattern]` consumer does the observing.
- **Tests** — the panic-recovery suites on HTTP, WebSocket and RPC assert the typed `PanicRecovered` through a declining chain handler. Each fails when the event stops carrying its segment.

Breaking: `ErrorObserver`, `use_global_error_observer` and `toni::errors::Cancelled` are gone. An RPC or gRPC guard panic is refused at the dispatcher rather than routed through the chain, so its wire refusal and one log line are its whole signal; HTTP and WebSocket still deliver a guard panic to `#[catch(PanicRecovered)]`.

Routing that refusal through the chain is deferred: it changes dispatch flow on two transports, not the observer's removal.

Implements ADR-0035.
