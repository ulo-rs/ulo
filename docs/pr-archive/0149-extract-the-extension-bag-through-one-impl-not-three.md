# #149 — Extract the extension bag through one impl, not three

Merged 2026-08-15 into `master` from `feat/extensions-one-impl`, commit [`daf5073`](https://github.com/ulo-rs/ulo/commit/daf50732bb82d58dd3cee6b9ef9aef78153adaf3).

`Extensions` had a separate extraction impl per transport — one via `FromRequestParts` for HTTP, one for WebSocket, one for RPC. Each did the same thing, and each could drift from the others.

```rust
impl<C: HandlerContext> FromContext<C> for Extensions {
    type Error = Infallible;

    async fn extract(ctx: &mut C) -> Result<Self, Self::Error> {
        Ok(ctx.extensions().clone())
    }
}
```

The bag is reached through `HandlerContext`, which every context implements, so one impl covers all of them — and any transport added later, without anyone remembering to add a fourth copy. `ext: Extensions` reads the same in a WebSocket handler as an HTTP one because it is the same code, not because three copies happen to agree.

## What proves it

The existing suite. `Extensions` is already extracted in HTTP, WebSocket and RPC handlers across the integration tests, and those tests were not touched — they now exercise one impl where they used to exercise three.

Added one unit test for the claim itself, which the per-transport impls could not have satisfied:

```rust
async fn read_from_any<C: HandlerContext>(ctx: &mut C) -> Extensions {
    Extensions::extract(ctx).await.expect("infallible")
}
```

A generic function over any `HandlerContext`. It compiles only if extraction is genuinely generic rather than repeated, and it is instantiated at a real context so the bound is shown satisfiable rather than merely well-formed.

## Also

Drops the `event` parameter threaded through three levels of the WebSocket dispatch. It lives on the context, which the handler now receives, so passing it alongside was left over from a signature that no longer exists.
