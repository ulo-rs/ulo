# #88 — refactor(adapter): slim public adapter traits — lifecycle methods off the trait, handles take ownership

Merged 2026-05-30 into `master` from `refactor/slim-adapter-traits`, commit [`492e0f6`](https://github.com/ulo-rs/ulo/commit/492e0f6e3fa37bfb0e94d1d7b038f662f5a04a56).

## Summary

Every public adapter trait — `HttpAdapter`, `WebSocketAdapter`, `RpcAdapter`, `GrpcAdapter` — now carries only configuration methods. Lifecycle methods (`listen` / `serve` / `local_addr` / `close`) fold into a single consuming `into_lifecycle` per trait (plural for WebSocket, where one adapter can serve N ports), returning a self-contained `*LifecycleHandle`. The handles own concrete state — local address, serve future, shutdown closure captured from the adapter's own state — instead of holding a `Box<dyn *Adapter>` they call back into. The four parallel `Erased*Adapter` `pub(crate)` shims that duplicated the public traits to make them object-safe are deleted: `HttpAdapter` converts to `#[async_trait]` to be directly object-safe, the other three traits already were.

## What changes

**Public adapter traits**
- `HttpAdapter`: `listen` + `close` → `into_lifecycle(self: Box<Self>, port, hostname, ctx) -> Result<HttpLifecycleHandle>`. Trait converts to `#[async_trait]`.
- `WebSocketAdapter`: `listen` + `close` → `into_lifecycle_handles(self: Box<Self>, ports: Vec<(u16, String)>) -> Result<Vec<WsLifecycleHandle>>`. Plural because one adapter serves N ports.
- `RpcAdapter`: `serve` + `local_addr` + `close` → `into_lifecycle(self: Box<Self>) -> Result<RpcLifecycleHandle>`.
- `GrpcAdapter`: `serve` + `local_addr` + `close` → `into_lifecycle(self: Box<Self>) -> Result<GrpcLifecycleHandle>`.

**Lifecycle handles** (`toni::adapter::lifecycle_handles`)
- Each `*LifecycleHandle` becomes `pub`, holds concrete state instead of `Box<dyn *Adapter>`, and stores shutdown as a `Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<()>>>>>` rather than re-dispatching through the trait.
- The old `SharedWsAdapter = Arc<parking_lot::Mutex<Option<Box<dyn WebSocketAdapter>>>>` workaround is gone; idempotent shutdown across per-port handles falls out of cloning the adapter's \`watch::Sender\` into each handle's closure.

**HTTP adapter crates** (toni-axum, toni-actix, toni-poem, toni-rocket, toni-salvo)
- Each \`impl HttpAdapter\` block gains \`#[toni::async_trait]\` and replaces \`fn listen\` + \`fn close\` with \`async fn into_lifecycle\`. Body unchanged apart from returning \`HttpLifecycleHandle::new(local_addr, serve, shutdown_callback)\` instead of \`ServerHandle { local_addr, serve }\`.
- Side effect: actix's \`close()\` was a no-op stub on the trait default; the new \`into_lifecycle\` calls \`bound.handle().stop(true)\`, so actix finally gets graceful shutdown.

**WebSocket adapter crates** (toni-tungstenite, plus separate-port WS impls on toni-axum / toni-poem / toni-salvo)
- Each \`impl WebSocketAdapter\` replaces \`fn listen\` + \`fn close\` with \`async fn into_lifecycle_handles\` that iterates the requested ports.
- Rocket has no \`WebSocketAdapter\` impl — its WS path is same-port via \`HttpAdapter::bind_ws\`, untouched.

**RPC adapter crates** (toni-tcp, toni-udp, toni-nats)
- Each \`impl RpcAdapter\` replaces \`fn serve\` + \`fn local_addr\` + \`async fn close\` with \`async fn into_lifecycle\`. NATS has no listener (\`local_addr\` returns \`None\`) and no graceful shutdown signal in the current implementation, so its callback is a no-op (matches pre-existing behaviour).

**gRPC adapter crate** (toni-grpc)
- \`impl GrpcAdapter\` replaces \`fn serve\` + \`fn local_addr\` + \`async fn close\` with \`async fn into_lifecycle\`. The watch-channel idempotency the old \`close()\` doc promised carries through to the closure.

**Internal**
- \`toni::ToniApplication::bind\` rewires its four adapter sites: it now calls \`adapter.bind(...)\` (or routes/services) followed by \`adapter.into_lifecycle(...).await\` directly, with no \`*LifecycleHandle::bind\` wrapper.
- \`Erased*Adapter\` traits and their blanket impls (~50 lines × 4) deleted.

## Shape of the new trait surface

\`\`\`rust
// Before
#[async_trait]
pub trait RpcAdapter: Send + Sync + 'static {
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()>;
    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;
    fn local_addr(&self) -> Option<SocketAddr> { None }
    async fn close(&mut self) -> Result<()> { Ok(()) }
}

// After
#[async_trait]
pub trait RpcAdapter: Send + Sync + 'static {
    fn bind(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>) -> Result<()>;
    async fn into_lifecycle(self: Box<Self>) -> Result<RpcLifecycleHandle>;
}
\`\`\`

## Tests

201/201 integration tests pass. Per-crate adapter tests pass everywhere except two pre-existing doctest compile failures in \`toni-nats\` (the \`app\` symbol isn't defined in the \`rust,no_run\` doc snippet — same failure on \`master\`, unrelated to this PR).

## Public API break

The trait surface change is a breaking API change for anyone outside the workspace implementing the adapter traits. Pre-1.0, this is the cheap window.
