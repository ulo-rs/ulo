# #168 — An RPC client attaches headers

Merged 2026-08-22 into `master` from `feat/rpc-client-header`, commit [`5888b78`](https://github.com/ulo-rs/ulo/commit/5888b784903d13f90276533fafcb9e9e94b37741).

An RPC client attaches wire fields under the name a handler reads them by. Depends on #167.

`RpcClient::request(p).metadata(k, v)` attached the fields `RpcContext::headers()` returns, so one call site said metadata and the other said headers about the same map:

| before | after |
| --- | --- |
| `RpcRequest::metadata(k, v)` | `header(k, v)` |
| `RpcCallInfo::with_metadata(k, v)` | `with_header(k, v)` |
| `RpcCallInfo::get_metadata(k)` | `header(k)` |
| `RpcCallInfo::metadata` | `headers` |

`#[doc(alias)]` keeps the old names reachable in rustdoc's search index, as on the context accessors.

## The wire is untouched

`"metadata"` stays the key in the JSON envelope every TCP and UDP peer already speaks, and the headers, user properties and record headers the broker transports map onto. Renaming a protocol field to match a Rust method would break existing clients to tidy a name.

## Verification

378 integration tests pass, and the four crates gating tests behind `integration` are checked with the feature on — those carry client call sites the default build never compiles.

The rename targets `.metadata("` — the two-argument builder call. A pattern matching the field access alone would take `ctx.metadata()` with it, that being the accessor for declared metadata and a different thing entirely.
