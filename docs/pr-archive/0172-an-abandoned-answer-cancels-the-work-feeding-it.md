# #172 — An abandoned answer cancels the work feeding it

Merged 2026-08-22 into `master` from `feat/cancel-the-abandoned-tail`, commit [`1e25301`](https://github.com/ulo-rs/ulo/commit/1e253017032d9840198359ba748e2b6643704b80).

`CancellationToken` gets its first producer. Depends on #171.

A dropped future already answers the common case — a client that goes away takes the handler with it — which is why four transports carried an unfired token without anyone noticing. What a dropped future does not answer is the tail:

```rust
#[get("/stream")]
async fn stream(&self, ctx: &HttpContext) -> ToniBody {
    let cancelled = ctx.cancellation().clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancelled.cancelled() => break,        // ← at the drop
                rows = expensive_query() => { … }          // ← otherwise, only at the next send
            }
        }
    });
    ToniBody::stream(rx)
}
```

The handler returned the moment it had a stream, so the spawned task is outside the future anything drops. A channel reveals the client eventually — at the next `send` — which for a slow unit of work means running it to completion for nobody first.

## What produces it

`ScopedBody` and `ScopedStream` already hold the execution through the drain. Each now tracks whether its inner body or stream reached the end, and a `Drop` before that fires the token. A body dropped owing frames is the client having gone.

Nothing else produces it. A buffered response has nothing alive that could hear one.

## A layering note

The body carries `on_abandoned: Option<Box<dyn FnOnce() + Send>>` rather than a token. `context` already depends on `http_helpers`, and giving `Body` a `CancellationToken` would point an edge back the other way for a type that has no business knowing what an execution is. The WebSocket wrapper holds the concrete `WsContext` and cancels directly, no callback needed.

`Body` loses its derived `Debug` for a hand-written one, a boxed `FnOnce` having none.

## Tests

The producer is spawned on purpose — that is the shape a token is for. Falsified by removing the callback: the task carries on and its `select!` arm is never reached.

382 integration tests pass.
