# Handoff — the pipeline audit

**SPENT.** The pass ran 2026-09-21 against master `654c7183`. Its memo is `PIPELINE_AUDIT.md`, its
findings are `FRAMEWORK_GAPS.md` F121–F137, its build is `PIPELINE_BUILD_HANDOFF.md`, and the gate
it held over the HTTP build is recorded at the top of `HTTP_AUDIT.md` §10 and released. The four
passes it handed on are `DI_AUDIT_HANDOFF.md`, `EXTRACTION_AUDIT_HANDOFF.md`,
`RENDER_AUDIT_HANDOFF.md` and `PERFORMANCE_AUDIT_HANDOFF.md`. Kept for the brief it set; do not run
it again.

---

The brief as it was issued follows, unedited.

Run this as its own session. It is the next audit, and it gates the HTTP build.

## Scope

Middleware, guards, interceptors, error handlers and the chain, handlers, panic recovery, the
execution order, and how enhancer tiers resolve and stack. **Across all four transports as one
subject, not per transport.** Since #301–#310 there is one generic `run_chain<T>`, one
`EnhancerSet<T>` and one resolver — auditing HTTP's enhancers and then RPC's would read the same
code four times. Middleware is the exception and is HTTP's alone (ADR-0007), so it folds in as the
HTTP-specific half.

Out of scope: the wire. HTTP's is done, WebSocket's, RPC's and GraphQL's have their own handoffs.
Extraction (`FromContext` and the extractors) is also one cross-transport surface and is a
candidate for its own pass — decide at the start whether to take it here or leave it, and say which
in the memo.

## Why it goes first

**It can change ADR-0052.** Dispatch feeds the pipeline: if the miss should reach the chain
differently, or `Dispatched` needs a third variant, that belongs in 0052 before it is built. Record
the outcome at the top of `HTTP_AUDIT.md` §10 either way — "changes nothing" is the result the HTTP
build needs in order to start.

## What is different about this one

**The HTTP audit was mostly a conformance pass. This one mostly is not.** Under the three-category
test in `project-the-spec-conformance-seam`, the pipeline is category (3) — no specification
mentions guards, interceptors or an error chain. So "better" here does not mean "matches the RFC";
it means the design is coherent, each role earns its place, and the same concept is not spelled
three ways.

The one place category (1) reaches in: whatever the pipeline puts on the wire. ADR-0053 is already
taking the HTTP error body. Check whether any other pipeline output crosses a wire a standard
specifies.

## Already filed — read before probing, do not re-derive

F51, F56, F57, F60, F74, F81, F84, F88, F118, F119 in `FRAMEWORK_GAPS.md`. The substantial ones:

- **F60** — the interceptor chain is built four times and the panic it catches is rendered three.
  Named in `AUDIT3_DISPATCH.md` as the one judgement call its series deferred.
- **F57** — three transports merge their enhancer tiers on every call; HTTP merges once.
- **F88** — on RPC and WebSocket, rendering inside an error handler changes nothing.
- **F81** — a panic in a global middleware is recovered and then dropped, reaching no `#[catch]`.
- **F84** — an extraction failure answers the caller without the chain, and no record says that was
  decided.
- **F118/F119** — the middleware scoping surface. Raised during the HTTP audit and filed for this
  pass rather than acted on.

`AUDIT3_DISPATCH.md` §4 is the prior art: it drew the line at what stays per transport (a `Leaf<T>`
impl, a render, an arrival adapter) by measuring differing lines after substituting the transport
noun. Read §8's walked checklist — four boxes are still open.

F22 and F87 are the renderer's reachability and are being taken by ADR-0053. Do not take them here.

## Method that worked on the HTTP pass

- **Probe, then falsify.** Every probe was disabled and re-run to confirm it measured what it
  claimed. A clean first run proves nothing.
- **Fetch specs, do not recall them.** `curl -sS https://www.rfc-editor.org/rfc/rfcNNNN.txt` into
  the scratchpad and `grep -n -A`. `WebFetch` truncated RFC 9110 three times. Two claims written
  from memory were wrong.
- **Confidence marking per claim** — Probed / Read / Spec / Unverified — declared near the top of
  the memo. That marking is what made a wrong probe tractable on the SSE pass.
- Ordering behaviour is best probed with a recording middleware or enhancer that appends to a
  static, the way `global_chain_conformance.rs` does.

## Deliverable

1. A memo, `PIPELINE_AUDIT.md`, at this level beside the others. Form:
   `feedback-the-audit-memo-form` in memory.
2. Findings to `FRAMEWORK_GAPS.md`, numbered from the highest there.
3. Its own build handoff, the way `HTTP_BUILD_HANDOFF.md` was written for the HTTP build.
4. The outcome recorded at the top of `HTTP_AUDIT.md` §10, which is what releases the HTTP build.

Nothing is committed by an audit. `toni-rs` is clean at `654c7183` on master; branch
`docs/http-audit-adrs` holds the HTTP ADRs.
