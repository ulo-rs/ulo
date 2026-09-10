# #201 — docs(adr): propose stream-shaped RPC replies

Merged 2026-08-29 into `master` from `docs/adr-a-reply-can-be-a-stream`, commit [`f082e9e`](https://github.com/ulo-rs/ulo/commit/f082e9e725cef6f1d29d0b7bd736ef792368a9fa).

Proposes ADR-0032: an RPC reply can be a stream, and the execution rides it. The decision fixes the handler output enum (`Empty`/`Single`/`Stream` with fallible items), a wire grammar for stream frames (`stream`/`stream_b64` items and an `end` marker that never aliases the `{"response": null}` ack), an upstream cancel notice with per-transport routing, a client `open_stream` verb, and a split that puts the grammar and the drive loop in one core module while adapters carry only their bytes.
