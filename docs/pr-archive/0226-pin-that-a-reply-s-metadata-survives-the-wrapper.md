# #226 — Pin that a reply's metadata survives the wrapper

Merged 2026-09-01 into `master` from `test/grpc-response-metadata`, commit [`126bbad`](https://github.com/ulo-rs/ulo/commit/126bbade785d424929a96fb39818ba8cad6f05a3).

`#[grpc_methods]` takes every reply apart and rebuilds it — `into_parts` to re-type a streaming one, `from_parts` to hand it back. Response metadata rides in the part that is carried across, and nothing checked that it arrives.

A unary reply is the case worth asserting on: the rebuild changes nothing there, so losing the metadata would produce no other symptom.

The handler in `grpc_tail.rs` now stamps `x-served-by` on its reply and the test reads it back off the client's response.
