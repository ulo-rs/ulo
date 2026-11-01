# Handoff — the extraction audit

Run this as its own session. Deferred out of the pipeline audit, which took only the one place
extraction touches the pipeline.

## Scope

`FromContext` and everything built on it: the HTTP extractors (`Json`, `Path`, `Query`, `Body`,
`Bytes`, `BodyStream`, `Multipart`, `Extensions`, `HttpRequest`, `LastEventId`), the
transport-agnostic ones (`Payload`, `Validated`, `take_body`), gRPC's `Inbound` and `GrpcRequest`,
the markers (`#[body]`, `#[param]`, `#[query]`), the one-body rule, and what a failed extraction
answers on each of the four transports. **One cross-transport surface**, the way the pipeline was —
`FromContext<C>` is one trait and auditing HTTP's extractors then RPC's would read the same
machinery twice.

Out of scope: the wire shapes, which the per-transport audits hold; percent-decoding and the query
parser's dependency, which ADR-0052 moves into core and which that build owns (F111, F117).

## What is different about this one

**Mostly category (3), with one category-(1) edge.** Extraction is the framework's own design and no
specification mentions it. The edge is what a failure puts on the wire, which is a status and a body
— ADR-0053 owns the HTTP body, and the other three transports' shapes belong to their own audits.
So this pass decides the *mechanism*: when extraction runs, what it may consume, and who hears about
a failure.

**The one question the pipeline audit handed over is F84**, and it is a design question, not a bug:
an extraction failure answers the caller from inside the generated wrapper, above the guards and
outside the error chain. `extraction_failure_shape.rs` pins it as intended and the workspace
`CLAUDE.md` states it as behaviour, so it was chosen — but ADR-0035 made the chain the one error
seam, and nothing records why extraction is the exception. Decide it, and write down whichever way it
goes. It is the same shape as the pipeline audit's §2, which was decided in favour of the chain.

## Already filed — read before probing, do not re-derive

`FRAMEWORK_GAPS.md`: **F84** (the chain bypass, above), **F117** (a repeated query key cannot be
extracted — a dependency swap that touches `Path` as well), **F1** (an extraction failure on RPC or
WebSocket renders `Internal`, where HTTP answers 400), **F53** (`PayloadError` is two different types
under one name), **F54** (only HTTP lets a handler declare what it reads beyond the payload),
**F75** (a wrong `Payload<T>` on gRPC fails on the first call, not at compile time — ADR-0043 records
it as a deliberate follow-up).

`DOCS_AUDIT.md`: **A1–A4** (the marker forms: the one-body count, `#[param]`'s ignored `default`, a
bad `#[query]` default panicking at request time, the misnamed `NotJson` on an empty body), **C3**
(marker failures speak a different error JSON than typed extractors), **C11** (every extraction
failure on WebSocket renders kind `Internal`), **C23** (`ExtractorKind` has drifted from its purpose
— only three variants change the generated code), **C24** (`HttpRequest` as a parameter has two
implementations that disagree on the message a client sees).

Prior art: ADR-0023 (one trait), ADR-0040 and ADR-0041 (the `CONSUMES` compile check and the bare-DTO
removal), ADR-0043 (gRPC's parameters). Memory: `project-extraction-is-one-trait`,
`project-extraction-uniformity-parked` (the erased-context alternative, **declined outright** — do
not re-propose it), `project-controller-macro-extractor-dispatch`,
`project-path-scalar-extractor-broken`.

## Method

**`feedback-the-spelling-audit` is the central tool for this pass** and extraction is where it was
born — the extractor classification by literal type name was one of its four recorded instances.
Run both halves: write each extractor's type behind an alias, through a renamed import, boxed,
nested one level and fully qualified, and compile each; then check whether two accepted spellings of
one intent are treated differently. C23 and C24 are both tells that the classification still reads
text somewhere.

Otherwise as the pipeline pass: probe then falsify, confidence marking per claim, and
`AUDIT3_DISPATCH.md` §4's measurement rule before collapsing anything.

## Deliverable

1. A memo, `EXTRACTION_AUDIT.md`, at this level. Form: `feedback-the-audit-memo-form`.
2. Findings to `FRAMEWORK_GAPS.md`, numbered from the highest there.
3. Its own build handoff.
4. A decisions section, F84 first.

Nothing is committed by an audit.
