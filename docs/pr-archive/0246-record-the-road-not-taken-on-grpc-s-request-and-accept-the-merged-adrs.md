# #246 — Record the road not taken on gRPC's request, and accept the merged ADRs

Merged 2026-09-06 into `master` from `docs/record-the-road-not-taken-and-accept-the-adrs`, commit [`76b3763`](https://github.com/ulo-rs/ulo/commit/76b37638dea14dc9609b387e38f8ac3332614cd5).

Two documentation corrections, no code.

**ADR-0042 gains the alternative it was chosen over.** `Payload<T>` is a `FromContext` on HTTP, RPC and WebSocket, and a `GrpcRequest` on gRPC — one spelling over two mechanisms. The design that would unify them is to shrink `GrpcRequest` to `type Arg` alone, put `request.into_inner()` into the `GrpcContext` type-erased, and extract the request through `FromContext<GrpcContext>` like every parameter after it.

It buys the spelling and loses on every other count: a typed move becomes a runtime downcast, `tonic::Request<T>` as a parameter stops working because the context is built *from* that request's extensions and cannot hand it back, and the positional rule survives regardless — the macro must name the message type in the signature it writes, and erasing the value does not change where that type comes from. `GrpcContext` carries the method, headers, peer and deadline, not the message, which is what the comparison turns on.

The consequence goes in beside it: the shared spelling with an unshared mechanism is the price of gRPC being the one transport whose signature toni does not own.

**Thirteen ADRs describing shipped behaviour still read `Status: proposed`,** against twenty-nine `accepted` — 0018, 0019, 0021 and 0033 through 0042. The field distinguished nothing while a third of it was drift, so all thirteen are marked accepted rather than only the recent ones.
