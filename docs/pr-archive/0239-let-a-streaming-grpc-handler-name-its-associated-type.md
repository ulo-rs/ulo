# #239 — Let a streaming gRPC handler name its associated type

Merged 2026-09-04 into `feat/grpc-handler-params` from `feat/grpc-stream-assoc-override`, commit [`94525fc`](https://github.com/ulo-rs/ulo/commit/94525fca7ccff19cc64ba712f520a80791cff900).

`#[grpc_stream]` infers the associated type from the method — `greet_many` pairs with `GreetManyStream` — which holds because tonic-build derives both names from one proto identifier. `tonic_build::manual` sets the Rust name and the route name independently, so a `watch` there can declare `StreamProgressStream` and the inference is wrong.

The method names it: `#[grpc_stream(StreamProgressStream)]`.

Without this, a manual-built trait has no expression in the handler form, and removing the trait-impl form would strand it — the old form's `#[stream(...)]` exists for exactly this case.

- **Tests** — `grpc_manual_trait_form.rs` serves the manual fixture whose route is `StreamProgress` and whose method is `watch`. Serving the call at all says the named associated type matched the trait's; dropping the reply says the wrapper still read it as streaming and carried the execution across it.

Also recorded in the ADR: a handler's error type implements `toni::Error`, and `GrpcStatus` does not — it is what a `toni::Error` maps into, so implementing both sides would collide with that blanket. A handler wanting a code no `ErrorKind` reaches takes the raw request and answers `tonic::Status` itself.

Extends ADR-0038.

Depends on #238.
