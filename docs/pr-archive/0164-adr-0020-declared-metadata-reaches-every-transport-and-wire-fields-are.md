# #164 — ADR 0020 — declared metadata reaches every transport, and wire fields are headers

Merged 2026-08-21 into `master` from `docs/adr-metadata-model`, commit [`006173c`](https://github.com/ulo-rs/ulo/commit/006173c0cbd92e22934b50361f865df313f80b4f).

Records how `#[set_metadata]` gets populated, and the naming that follows once it is read on more than one transport.

**What the survey found.** `set_metadata` is consumed by a single macro file, and it reads `&method.attrs`:

- WebSocket, RPC and gRPC macros do not handle the attribute at all — their trait defaults return an empty map nothing overrides.
- Controller-level `#[set_metadata]` is collected nowhere, HTTP included.

Against Nest's `getAllAndOverride([getHandler(), getClass()])`, toni has the handler half on one transport out of four.

**Why it is worth doing now.** A universal guard compiles today:

```rust
impl<C: HandlerContext> Guard<C> for RolesGuard { … }
```

Registered on a WebSocket gateway it does not error and does not refuse. It reads an empty map, finds no requirement, and waves every message through — it fails open, and nothing reports it.

The decision:

- Every structural macro collects the attribute at impl-block and method level, emitting parent entries first. A later `insert` shadows an earlier one, reproducing Nest's first-non-undefined result with no lookup-time search.
- `RouteMetadata` becomes `Metadata` in `context`. `HandlerContext::route_metadata` becomes `metadata`, matching the attribute that writes it — "route" stops being true once a WebSocket event or an RPC pattern carries it.
- `RpcContext` and `GrpcContext` expose `headers()` / `header(k)` where they exposed `metadata()` / `get_metadata(k)`, so the two most confusable things on one object stop differing by a prefix.
- `http_helpers::Extensions` becomes `TypeMap`. It is a second public type of that name whose module doc describes the *other* one's job; renaming it leaves one `Extensions` in the crate.

**The accepted cost:** gRPC's specification calls these metadata, so `headers()` is an infidelity there. The doc comments record it, keeping the spec term findable.

Three roads not taken are recorded with their reasons. Docs only; the implementation follows separately.
