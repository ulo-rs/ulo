# #152 — A context lives until its answer is complete

Merged 2026-08-17 into `master` from `feat/context-lives-until-the-answer-completes`, commit [`d454f67`](https://github.com/ulo-rs/ulo/commit/d454f6776084365aec3a31ea55222261a0f35705).

An execution was over when the handler returned. A streaming answer has produced nothing at that point, so everything the execution held — the extension bag today, the instances built for it once there is a cache — went away while the response was still being written.

The response body now carries the context, and a WebSocket streaming answer carries it the same way. The adapter drains; the last frame drops the handle; the execution ends there.

## What this adds over making contexts handles

Half of the lifetime problem was already solved by #151: a stream that *captures* a context keeps the execution alive through its own `Arc`, with no help from the dispatcher.

The obvious test — a handler cloning the context into its stream — passes with this PR's mechanism disabled. It measures #151, not this.

What the wrap adds is the other half: the execution's state survives the drain **whether or not the answer holds a reference to it**. That is the difference between a property of the framework and a property of how a particular handler was written, and it is what will make request-scoped instances safe to drop at the end of an execution rather than at the end of a handler.

## Proof

Two streams, pinning different things.

- `/tail/captured` holds the context and reads the bag through it. True since #151; documented so the two mechanisms don't get conflated again.
- `/tail/detached` holds an `AtomicBool` and **never the context**. A guard puts a `Drop` sentinel in the bag; each frame reports whether the execution's state is still there.

The second is the one that matters, and it falsifies cleanly:

```
without the wrap:  0:dropped;1:dropped;2:dropped;
with it:           0:alive;1:alive;2:alive;
```

## Components

- **`Body::keep_alive`** parks a value; `into_box_body` wraps in a delegating `ScopedBody` only when one is present, so a buffered body still reports as buffered — rocket branches on `is_streaming()`.
- **The HTTP dispatcher** gains a single exit: `execute_controller_logic` builds the context, `run_chain` does the work, and the attach happens once instead of at each of the chain's returns.
- **`ScopedStream`** does the same for `WsHandlerOutput::Stream`. Included rather than deferred: a WebSocket stream is a tail that has emitted nothing when the handler returns, and leaving it out would have left ADR 0016 describing behaviour the code didn't have.

Both wrappers are plain delegating types with no pin machinery — `BoxBody` and `BoxStream` are `Pin<Box<_>>` and therefore `Unpin`.

## Not included

Cancellation. `CancellationToken` still has no producer, and this PR is what gives it a sensible one: a response body dropped before it finished streaming is the disconnect signal, which is the analogue of Node's `writableFinished`. Wiring it is per-adapter work and a separate decision.

355 integration tests, 87 core unit tests, fmt clean, no new clippy warnings.
