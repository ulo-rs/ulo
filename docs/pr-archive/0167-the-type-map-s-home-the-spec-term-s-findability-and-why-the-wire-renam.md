# #167 — The type map's home, the spec term's findability, and why the wire rename was forced

Merged 2026-08-22 into `master` from `feat/metadata-naming-followups`, commit [`17cd28a`](https://github.com/ulo-rs/ulo/commit/17cd28ac58e5b7cc3134d8a69a42a24fdc9958fc).

Three follow-ups to ADR 0020, which moves to `accepted` here, its decisions being implemented.

**The type map is not an HTTP helper.** Its two users are a declared-metadata map read on four transports and an RPC call descriptor. `TypeMap` moves to `toni::type_map`; leaving it under `http_helpers` repeated one level down the misfiling the ADR corrects one level up, where `RouteMetadata` sat in that module while every transport read it. A bare URL in its module doc is bracketed while the file is open, rustdoc having warned on it.

**`#[doc(alias)]` on the wire accessors.** The ADR claimed doc comments keep gRPC's spec term findable. Prose in a doc comment does not reach rustdoc's search index; an alias does. `headers` and `header` are now reachable by searching `metadata` and `get_metadata`, verified present in the generated name index.

**Why the wire rename was not a choice.** The decision listed keeping `metadata()` on the wire side among the roads not taken. It is not a road:

```rust
fn generic<C: HandlerContext>(c: &C) -> _ { c.metadata() }  // declared — no inherent candidate
fn concrete(c: &GrpcContext)      -> _ { c.metadata() }     // wire — inherent wins
```

Both compile, and neither `rustc` nor `clippy` reports anything. A guard written generically would read what `#[set_metadata]` declared while the identical line specialised to one transport read the wire — the same fail-open the ADR opens with, on `RpcContext` and `GrpcContext` alike. That belongs in the decision as the constraint that forces it, not in a list of weighed alternatives, so the second rename now reads as what the first one costs.

Two things the ADR could have claimed and did not, now in Consequences: `get_metadata(k)` sheds a `get_` prefix the Rust API guidelines discourage, and `headers()` / `header(k)` is a plural and its singular rather than two unrelated shapes. Also recorded: a gRPC transport file imports `context::Metadata` and `tonic::metadata::MetadataMap` together, names that differ enough to compile and not enough to skim.

378 integration tests pass, and the four crates gating tests behind `integration` are checked with the feature on.
