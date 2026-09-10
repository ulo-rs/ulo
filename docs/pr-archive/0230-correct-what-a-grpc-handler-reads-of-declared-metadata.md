# #230 — Correct what a gRPC handler reads of declared metadata

Merged 2026-09-01 into `master` from `docs/grpc-handlers-do-read-metadata`, commit [`0e67f20`](https://github.com/ulo-rs/ulo/commit/0e67f20726a39beac618d29a86453177e2737c4e).

`#[set_metadata]`'s documentation claims a gRPC handler cannot read declared metadata:

> **On gRPC a handler cannot read it.** The tonic trait dictates that signature and never passes the context…

The first half is still true and the second is not. A handler's signature is tonic's, so guards, interceptors and error handlers receive the context as a parameter — and a handler takes it off the request with `GrpcContext::of(request.extensions())`. What the service declared is readable either way.

`GrpcContext`'s own documentation already draws that distinction. This corrects the sentence on the attribute, which is where a reader looking for the rule meets it first.
