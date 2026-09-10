# #153 — The execution owns its cache, and every transport has one

Merged 2026-08-18 into `master` from `feat/the-execution-owns-its-cache`, commit [`16001cf`](https://github.com/ulo-rs/ulo/commit/16001cff2ed6d6b74952ade115a0640548c97ab9).

The per-execution instance cache rode the HTTP request parts. Not because it was HTTP — nothing in a `TypeId`-keyed store of constructed instances is — but because enhancer factories received `Option<&RequestPart>`, and that was the only handle they had to reach it through.

That one carrier decided a lot. RPC, WebSocket and gRPC each refused **at startup** to let a request-scoped provider be an enhancer dependency:

```rust
anyhow::bail!(
    "Guard '{}' has request-scoped dependencies and cannot be used on an \
     RPC controller — RPC has no HTTP request context", token
);
```

Request scope was an HTTP privilege, and the reason was a field's address rather than anything about scope.

## What changes

The cache moves onto the context and is renamed `ExecutionCache`. `HandlerContext` gains `cache()`, so it is reachable on any transport.

Factories receive the execution rather than a slice of one:

```rust
fn create<'a>(&'a self, ctx: &'a C)
    -> Pin<Box<dyn Future<Output = Arc<dyn Guard<C> + Send + Sync>> + Send + 'a>>;
```

Which reorders the dispatcher — the context is built *before* enhancers resolve, rather than assembled from parts afterwards — and retires `requires_http_parts()` along with all three refusals.

`ProviderContext` carries each transport's handle instead of borrowed fields, so its hollow `WebSocket` and `Rpc` variants are filled, and the generated request-scoped code stops destructuring `Http` to find the cache. It asks the execution, whichever one it is.

## Proof

A request-scoped provider injected into two RPC guards, constructed once for the call and shared by both. **That test could not have run before this PR** — not failed, but refused at startup by the `bail!` above.

It exercises two rules composing: a guard injecting a request-scoped dependency must itself be request-scoped, and that elevation is exactly what puts it on the factory path the refusal blocked.

## Three things worth a reviewer's attention

**`ProviderContext` loses `Copy`, and that surfaced a latent bug.** Four generated resolution sites did `provider.execute(vec![], _ctx)` — passing the context *by value*. Correct only while the enum was two borrowed references; now a move error the compiler names. The type change didn't break working code so much as reveal code that worked by accident.

**The transient path's comment contradicted its code.** The comment said transient construction carries no request context; the code passed `_ctx` straight through, and `valid_transient_injects_any_scope` pins that behaviour. The pass-through stands, and the comment now describes it: a transient is built inside whatever execution asked for it, so its request-scoped fields resolve in that same one.

**`begin_connect` stops taking the upgrade's `RequestPart`.** It existed so connect guards could be built from HTTP parts. They get the `WsContext` now, and read the handshake through `ctx.client().handshake` — the same information by a route that also works per-message.

## Breaking

- Enhancer factories take `&C` instead of `Option<&RequestPart>`; `requires_http_parts()` is gone.
- `RequestCache` is `ExecutionCache`, lives on the context, and `install`/`adopt` are removed. `ApplicationContext::resolve` builds its own execution per call; to share one, build a context.
- `ProviderContext` has no lifetime and is `Clone`, not `Copy`. `HttpProviderContext` is removed — the variants hold context handles.
- `GatewayWrapper::begin_connect` / `handle_connect` no longer take parts.

356 integration tests, 86 core unit tests, fmt clean, no new clippy warnings. The unit count is down one: three tests covered the `install`/`adopt` carrier and went with it; two replaced them.
