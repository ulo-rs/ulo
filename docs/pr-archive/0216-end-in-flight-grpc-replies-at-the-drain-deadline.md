# #216 — End in-flight gRPC replies at the drain deadline

Merged 2026-08-31 into `master` from `fix/grpc-drain-stops-streams`, commit [`6797c8b`](https://github.com/ulo-rs/ulo/commit/6797c8b9529774e93675b485aa8e327da4e1ce01).

When the gRPC drain deadline elapsed with a streaming call in flight, the reply kept being served. `close()` returned, the application logged shutdown complete, and five seconds later a reply was still producing.

tonic serves each connection from a task it `tokio::spawn`s and detaches, holding a watch receiver whose drop is how the server learns the connection finished. Dropping the serve future stops the acceptor and reaches none of them. The adapter's comment claimed the drop was an abort — *"hyper closes connections, and any task still executing a streaming handler is cancelled"* — and the crate doc claimed *"open connections close, streaming clients see `UNAVAILABLE`"*. Neither happened.

## What the deadline does now

Nothing in tonic hands back a handle to a connection task, but the framework owns the body each call answers with. `DrainLayer` wraps every reply body and ends it when the deadline flips: the reply closes with `UNAVAILABLE` trailers, its stream drops — firing the execution's cancellation token through `ScopedGrpcStream` — and the connection is left with nothing in flight, so tonic's graceful shutdown closes it.

The serve future is then awaited rather than dropped, bounded so a connection ignoring `GOAWAY` cannot hold shutdown open. That bound is the drain timeout itself, capped at two seconds: the timeout is a statement of how long shutdown may take, and a constant added after it would answer a question the caller did not ask. `close()` returns within twice what is configured, which the builder method and the crate doc now say.

## Tests

`the_drain_deadline_ends_a_reply_it_cannot_wait_for` boots with a 300 ms drain timeout, opens a stream, and shuts down while it is producing. It asserts the three things that were false: shutdown completes, the caller's stream ends with `UNAVAILABLE` rather than losing its connection, and the task feeding the reply learns.
