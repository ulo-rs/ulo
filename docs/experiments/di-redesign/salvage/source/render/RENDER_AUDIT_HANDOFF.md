# Handoff — the render-reachability audit

Run this as its own session, and **read its first section before scheduling it**: this pass may not
be worth running as a whole, and deciding that is its first job.

## Scope

The renderer as a mechanism, on all four transports: who may call it, what it produces, what happens
when it fails, and whether an application can reach it. `render_error`, `HttpError::to_response`,
`RpcError::to_data`, `WsError::to_message`, the three `safe_render` wrappers and their fallback
literals, and `IntoOutput`'s side of the same question — what turns a *success* into bytes.

**Not in scope: each transport's wire shape.** WebSocket's, RPC's and GraphQL's have their own
handoffs, and HTTP's is ADR-0053's. This pass asks whether the rendering *seam* is right, not what
any one transport writes.

## Read this first — the pass may already be spent

Four of the findings that motivated it are claimed by work already decided:

- **F65, F69** — the invented envelope and two imprecise mappings. Taken by **ADR-0053**.
- **F22, F87** — an application cannot restyle the envelope, and a handler catching its own error
  type cannot produce the canonical one. **ADR-0053 makes `render_error` reachable** and says so
  explicitly, on the grounds that rewriting the renderer without making it reachable would leave
  both open against a shape no caller can produce.
- **F136** — the second error vocabulary beside `ErrorKind`. **Settled by the pipeline audit**
  (`PIPELINE_AUDIT.md` §10) and built in its Phase 2 PR 7.
- **F129** — the three `safe_render` copies. Built in the pipeline build's Phase 1 PR 4.

So **run this pass after ADR-0053 and the pipeline build have landed**, and open it by asking whether
anything is left. If the answer is "a handful of entries and no structural question", file them
against whichever build is open and write no memo. An audit that pads itself to justify its own
existence is worse than none.

## What is likely to be left

Three things that none of the above claims, each worth checking against the post-0053 tree:

- **A recovered panic's message reaches the caller.** `PanicRecovered` implements `ulo::Error` with
  `message()` returning its `Display`, and `render_error` puts `message()` in the envelope — so a
  caller receives the panic payload's own text. The generic "internal server error" literal that
  exists to prevent exactly this is unreachable for user code. `DOCS_AUDIT.md` C12, framework-wide
  rather than transport-local, and the one item here with a disclosure edge.
- **The fallback literals.** Three hand-written envelopes reached only when a renderer panics
  (`http/pipeline.rs`, `rpc/rpc_controller_wrapper.rs`, `ws/gateway_wrapper.rs`), gRPC having none
  because `GrpcStatus` cannot panic rendering itself. `AUDIT3_DISPATCH.md` §4 says a render stays
  per transport, so these are correct where they are — the question is whether any test reaches them,
  and whether they still match the shape 0053 writes.
- **Dead and adapter-dependent render paths**: `DOCS_AUDIT.md` C13 (`frame_response`'s `Stream`
  refusal branch is unreachable through every shipped adapter), C14 (the 405 body differs per
  adapter — closed by ADR-0052 if that ships first), C22 (no `DefaultGrpcErrorHandler` where the
  other three exist), A23, A24 (`WsError::EventNotFound` and three `Default*ErrorHandler` types are
  never constructed), **F63** (the RPC load shed answers `overloaded`, which is not an `ErrorKind`
  name), **F64** (one RPC error object spells its two fields in two cases), **F1**.

## What is different about this one

The category question is the whole of it. A renderer's *output* is frequently category (1) — a
standard specifies the bytes — while the renderer's *reachability* is category (3), the framework's
own design. `project-the-spec-conformance-seam` records that trap: an invention whose serialisation
is specified. Keep the two apart section by section, or this pass will re-litigate ADR-0053.

## Deliverable

If the pass survives its own first section: a memo `RENDER_AUDIT.md`, findings to
`FRAMEWORK_GAPS.md`, and a build handoff. If it does not: one line in this file recording that it
was checked and spent, the date, and the commit — so nobody schedules it twice.

---

## Checked and spent — 2026-09-23 at `654c7183`

Run as its own pass and answered at its first section: **no structural question survives, so no memo
and no build handoff.** Seven dispositions, all filed.

**The preconditions were unmet and the pass ran anyway, on the decisions rather than on the code.**
ADR-0053 is `proposed` on `docs/http-audit-adrs` (`9d3fb9c2`, one commit off master) and the HTTP
build has not started; the pipeline build has no branch. What claims a finding is the decision, not
its implementation, and every decision here is written and approved — so "is anything left" is
answerable now, and three of the answers are corrections to records that would otherwise be read as
settled.

### What was left

| Item | Disposition |
| --- | --- |
| **C12** — a recovered panic's message reaches the caller | **F242.** Confirmed on all four transports and probed: `panic_recovery::` passes ten cases, three of which assert the payload on the wire under comments calling it correct. Unclaimed by every build, and it survives ADR-0053 under a new field name |
| **The fallback literals** | **Spent.** Each is reached by a passing test, run this session at `654c7183`: `panic_recovery::panicking_renderer_falls_back_to_safe_envelope`, `ws_panic_recovery::ws_renderer_panic_falls_back_to_safe_envelope`, `rpc_tcp::rpc_renderer_panic_falls_back_to_safe_envelope` — each drives a domain error whose `message()` panics and asserts the literal. Whether they match 0053's shape is PR 11's verification, not an audit's |
| **The envelope census** | **F243**, plus **F244** on the way. ADR-0053 counts six sites; the routing seam ADR-0052 collapses is five of them. The uncounted ones are not a sixth envelope — no adapter has a fallible request conversion, so the arms guarding one are dead; rocket's is an upgrade marker, not an error; actix's bridge and core's chain literal are the framework's. The invariant that falls out is **an adapter never authors an error body** |
| **F22, F87** — the renderer's reachability | **Appended.** 0053's clause is framework-wide and its build is HTTP-only, so PR 11 closes one transport of three. gRPC already ships the exact signature — `GrpcStatus::from_error(&dyn Error)`, public and borrowing |
| **F69** — two imprecise HTTP mappings | **Appended.** 0053 closes the 422 phrase and records the whole entry as closed; `status_for(Timeout) => 504` survives it verbatim, and deleting the entry on 0053's own Consequences line loses the question |
| **F64** — two casings in one RPC error object | **Appended.** F65's "dissolves into this" premise is falsified by 0053 declining RFC 9457 beyond HTTP, and no build handoff carries F64 in any PR row |
| **A23, A24, C13, C14, C22** | **A24 and C22 are already closed** — no `Default*ErrorHandler` exists in the workspace. **A23 is live**: `WsError::EventNotFound` is declared, rendered, close-coded and constructed nowhere. C13 is F223's, in the RPC build's PR 17; C14 is ADR-0052's |

**Corrected 2026-09-24.** F243 as first filed said a request whose body cannot be read answers a
JSON envelope on axum and an empty 500 on actix. Both halves are wrong: `adapt_request` returns
`Ok` unconditionally on both, and the claim was inferred from the presence of `AdapterResult`
rather than from a path that produces `Err`. Reading the two functions to correct it found **F244**
— actix answers a request-line it cannot re-parse by substituting `GET` and `/` and dispatching
that, unreachable only because two `http` crate versions share a grammar, which nothing states.
The corrected finding is stronger than the original and the correction is what produced the second
one.

### What was checked and is not a finding

The three `render_error` copies are one function per transport differing in key names and return
type. `AUDIT3_DISPATCH.md` §4 rules that a render stays per transport, and ADR-0053 makes the three
wire shapes *more* distinct rather than less, so the duplication is the decision working. F129 takes
the `safe_render` triple above them and is right to stop there.

The success side is settled: `IntoOutput<T>` is public and an application implements it, ADR-0050
decided it on three transports, and F240 holds gRPC's absence for the pipeline build's PR 9. The
asymmetry worth naming is that the success renderer is a public trait and the error renderer is
`pub(crate)` — which is F22 and F87, above.

**Do not schedule this pass again.** F243 and F244 belong to the HTTP build, as one invariant stated
in ADR-0052 — *an adapter never authors an error body* — rather than as four repairs. **F242 is the
pipeline build's**, beside PR 8 and together with the gRPC audit's F238: both are one rule about what
a framework event tells a caller, and `GuardRejection` is already the worked example of it. The four
appends are read by whoever deletes those entries.
