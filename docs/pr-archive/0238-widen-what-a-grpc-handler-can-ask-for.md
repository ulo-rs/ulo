# #238 — Widen what a gRPC handler can ask for

Merged 2026-09-04 into `feat/grpc-inbound-streams` from `feat/grpc-handler-params`, commit [`0f3069e`](https://github.com/ulo-rs/ulo/commit/0f3069e4393b212a7a2df4cd0e4c17a35e14e275).

A handler could name its request and the context, and nothing else. It can now take:

| parameter | what it is |
| --- | --- |
| `Payload<T>` or `T` bare | the request message |
| `Inbound<T>` | the messages the caller streams |
| `Extensions` | the execution's bag |
| `&GrpcContext` | the call's context |
| `tonic::Request<T>` | the whole request — trailers, peer address, metadata as it arrived |

A parameter naming none of those is read as the request message, the way an RPC handler spells its payload. A misspelled extractor lands there and fails as a type mismatch against the proto message.

The raw request is the one that matters beyond convenience: it keeps this form from being a subset of what the trait impl could express, so removing that form later strands nothing.

- **Tests** — the bag is written by a guard and read in the handler, so the test fails if the parameter hands over a fresh bag rather than the execution's. The raw-request test reads the peer address, which exists on the request and nowhere in the message.

`Validated<Payload<T>>` is not among them. Proto messages are generated, so there is nowhere to hang the `#[validate]` attributes it reads; validation on this transport is a check inside the handler.

Extends ADR-0038.

Depends on #237.
