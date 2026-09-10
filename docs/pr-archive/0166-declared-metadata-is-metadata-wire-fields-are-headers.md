# #166 — Declared metadata is metadata; wire fields are headers

Merged 2026-08-21 into `master` from `feat/metadata-naming`, commit [`f7c0437`](https://github.com/ulo-rs/ulo/commit/f7c04377338c6b9cde33f17387da937f769e808c).

Completes ADR 0020 (#164). The population half merged in #165; this is the naming.

Two things were called metadata on one object. `ctx.metadata()` returned the fields a call arrived with — NATS headers, AMQP headers, Kafka record headers, MQTT user properties, gRPC's `Metadata` — while `ctx.route_metadata()` returned what `#[set_metadata]` declared. Two unrelated concepts, a prefix apart, on the same type.

| before | after |
| --- | --- |
| `HandlerContext::route_metadata()` | `metadata()` |
| `RpcContext::metadata()` / `get_metadata(k)` | `headers()` / `header(k)` |
| `GrpcContext::metadata()` / `get_metadata(k)` | `headers()` / `header(k)` |
| `http_helpers::RouteMetadata` | `context::Metadata` |
| `http_helpers::Extensions` | `http_helpers::TypeMap` |

The declared side takes the name of the attribute that writes it. "Route" stopped being true once a WebSocket event or an RPC pattern carries it, and the type no longer sits in `http_helpers` while all four transports read it.

`TypeMap` is the synchronous map underneath. It was a second public `Extensions`, and its module doc described what `context::Extensions` does rather than what it is; there is now one type of that name in the crate.

## The accepted cost

gRPC's specification calls these metadata, so `headers()` is an infidelity there. Its doc comments record that, keeping the spec term findable by search.

## Docs

`HandlerContext::metadata` now states what it holds and what it is not, and the two renamed accessors carry the wire-versus-declared distinction. ADR 0015 gets a pointer on the consequence it recorded — that WebSocket metadata "is always empty" — which is no longer true.

## Verification

378 integration tests pass. The rename was done by renaming definitions and letting the compiler find call sites rather than sweeping text.

That missed four crates on the first pass: `toni-kafka`, `toni-rabbitmq`, `toni-redis-rpc` and `toni-mqtt` gate their tests behind `#![cfg(feature = "integration")]`, so `--all-targets` compiled none of them. Every crate carrying that feature is now checked with it enabled.
