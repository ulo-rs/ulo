# #13 — feat(core): replace Body enum with struct; HttpRequest body is raw Bytes

Merged 2026-03-26 into `master` from `feat/streaming-body`, commit [`d3767b7`](https://github.com/ulo-rs/ulo/commit/d3767b7d302345cab780a0ad06ad249d7f883734).

## Summary

- `Body` is now a struct with static constructors (`Body::text`, `Body::json`, `Body::binary`) carrying a content-type hint
- `HttpRequest.body` is `bytes::Bytes` — adapters buffer once, content-type stays in headers
- Response side: adapters read `body.content_type()` + `body.into_bytes()`, no variant branching

The old enum forced adapters to classify bytes into Text/Json/Binary at the transport boundary — before any application code ran. That decision belongs to extractors and handlers, not adapters. `Bytes` is also the chunk currency of the hyper/tower ecosystem, making this the foundation for response streaming on a follow-up branch.

## What's next

`feat/tower-compat` rebases cleanly on top of this.
