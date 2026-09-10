# #218 — Register gRPC enhancers globally

Merged 2026-08-31 into `master` from `feat/grpc-global-enhancers`, commit [`4ba17f5`](https://github.com/ulo-rs/ulo/commit/4ba17f58c9cd5dedaf79db65663a9c370b950c10).

HTTP, RPC and WebSocket each take guards, interceptors and an error handler on the factory that apply to every handler of that transport. gRPC took none — a policy every service must obey had to be named on every service, and adding a service was the moment to forget it.

```rust
let mut factory = ToniFactory::new();
factory.use_global_grpc_guards(Arc::new(RequireApiKey));
factory.use_global_grpc_interceptors(Arc::new(Timing));
factory.use_global_grpc_error_handler(Arc::new(Envelope));
```

Ordering matches the other transports: global, then the service's, then the method's, with guards answering before the interceptor chain is entered.

The container and `grpc_service_resolver` already held global gRPC entries and merged them ahead of a service's own tokens. What was missing was any way to put entries there, which is what the three factory methods add.

## Tests

Four in `grpc_global_enhancers.rs`, keyed on a recorded call order: a global guard runs ahead of the service's own; a rejecting one stops the call before the service's guard is asked; a global interceptor wraps the chain below it; a global error handler reshapes an `Err` the service left. Each fails with the factory's registration loop removed and passes with it.
