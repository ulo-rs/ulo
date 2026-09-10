# #74 — feat(core): replace Context with typed per-transport handler contexts

Merged 2026-05-06 into `master` from `feat/handler-context`, commit [`3864e2b`](https://github.com/ulo-rs/ulo/commit/3864e2ba8687b93630b0c34c8e1a66d46b100e9c).

Enhancers (`Guard` / `Interceptor` / `Pipe` / `ErrorHandler`) are now
generic over the per-request context type. `HttpContext`, `RpcContext`,
and `WsContext` are concrete structs that each implement
`HandlerContext`; a guard reads request data directly off `&HttpContext`
or `&RpcContext` instead of unwrapping `Option`s from
`Context::switch_to_*()`. Universal enhancers — code that genuinely
runs on every transport — write a blanket `impl<C: HandlerContext> Guard<C>`
and slot into all three; transport-specific enhancers can't be
mis-attached because the type system rejects the cast at registration.

### Surface

- **Trait shape.** `Guard<C: HandlerContext>` and siblings; `dyn Guard<HttpContext>`
  and `dyn Guard<RpcContext>` are different concrete trait objects, so
  the registry holds them in per-transport slots and the dispatcher
  walks each transport's chain without runtime discriminants.
- **Per-transport dispatch.** HTTP, RPC, and WebSocket dispatchers each
  build their own typed enhancer pipeline. RPC and WS gain typed
  globals (`use_global_rpc_*`, `use_global_ws_*`) alongside the existing
  HTTP ones; the resolver prepends them.
- **Macros.** `#[guard(http, rpc, ws)]` registers a struct in multiple
  typed slots; bare `#[guard]` on a generic-`C` impl head infers
  universal. `#[error_handler(rpc)]` requires the user to provide
  `ErrorHandler<RpcContext, RpcData>`; the type system enforces the
  response-type contract per transport.
- **Wire-level call info.** RPC adapters now hand the framework an
  `rpc::RpcCallInfo` (pattern + metadata + transport extensions) — the
  per-request handler-side `context::RpcContext` is built by the
  framework from that, plus route metadata + extensions bag +
  cancellation token + abort flag.
- **Examples.** The `multi_protocol_context` example shows one struct
  with three transport-shaped `Guard<C>` impls and one with three
  `Interceptor<C>` impls, attached to an HTTP controller, an RPC
  controller, and a WebSocket gateway from the same module.

### Default error fallbacks

- `DefaultHttpErrorHandler` → `HttpResponse`
- `DefaultRpcErrorHandler` → `RpcData`
- `DefaultWsErrorHandler` → `WsMessage`

(replacing the legacy `ErrorResponse` enum with three transport-typed
fallbacks.)

### Naming

The DI-side request/cache holder previously called `HttpContext` is
renamed to `HttpProviderContext` so `toni::HttpContext` re-exports the
handler context.

### Deferred follow-ups

- Collapse `Controller`'s eight enhancer accessors into a single
  `enhancers() -> ControllerEnhancers` typed accessor — pure shape
  cleanup, no behaviour change.
- Table-drive the 12-variant fan-out across the four macro emission
  sites — identical generated code, half the macro source.
- The TCP / UDP / NATS adapters don't yet surface per-call metadata to
  `RpcCallInfo.metadata`; reading auth from the JSON payload is the
  cross-adapter path until they do.
