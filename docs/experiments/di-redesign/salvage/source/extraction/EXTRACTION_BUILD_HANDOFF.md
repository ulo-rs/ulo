# Handoff — building what the extraction audit decided

For the session that implements it. The plan is `EXTRACTION_AUDIT.md`; nothing here restates it.

## Status

**D1 through D7 are settled.** `EXTRACTION_AUDIT.md` §9 records them with the alternatives rejected
and why. They were answered on 2026-09-22. **They are not open**, and a recommendation kept beside a
decision is there as its argument. Nothing in §9 is left for the builder to choose.

**Two of the seven are not this build's.** D6 parks a single typed header behind a named trigger and
names `headers` as the implementation when it unparks — no PR. D7 is a DI decision this pass raised;
it is recorded as **D14 in `DI_AUDIT.md`** and built as **group F of `DI_BUILD_HANDOFF.md`**, which
also carries F180 and F181. Do not build either from here.

## Read first, in this order

1. `EXTRACTION_AUDIT.md` §9 — the decisions.
2. `EXTRACTION_AUDIT.md` §1 and §2 — the two halves of what D1 fixes, and why the status is
   downstream of it.
3. `EXTRACTION_AUDIT.md` §3 and §4 — the marker path and the spelling matrix, which are D2's
   evidence.
4. `FRAMEWORK_GAPS.md` F163–F179 for what this closes, plus F84 (with its 2026-09-22 correction),
   F1 (with its gRPC appendix), F53 and F54 for the older entries it closes or spends. **F180 and
   F181 are filed by this pass and built elsewhere** — DI's group F.
5. `DOCS_AUDIT.md` §17 A2, A3, A4, C3, C4, C23, C24 and §20 A16, A17 — the docs-ledger entries this
   build closes. Closing one means re-touching the page that documents it as behaviour.
6. ADR-0023 and ADR-0040 before the first PR. 0023 is the record F178 is about, and 0040 is the rule
   every classification in this build has to satisfy.

## The two ADRs

Written first, on their own branch, both `proposed`. Each is flipped to `accepted` by the PR that
implements it, which is also when its supersession pointers are added.

- **An extraction failure is an error** — D1 and D4. Supersedes ADR-0023's *Roads not taken* on the
  trait-bound question, and carries the pointer under 0023's title that F178 is about. States the new
  bound, the four lift sites, and the status table §2 quotes RFC 9110 for. ADR-0040's rule is the one
  it works under, not one it changes.
- **One extraction language on HTTP** — D2, D3 and D5. Says that a marker is a spelling rather than a
  path, that a connection hook reads its parameters like every other handler, and what `#[param]`'s
  and `#[query]`'s defaults mean.

## The sequence

Two phases. Phase 1 is the bound and everything it unlocks, and its PRs go together because
PR 1 breaks every custom extractor. Phase 2 is the macro work and is independent of Phase 1 except
where a row says otherwise.

### Phase 1 — the error type

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 1 | `FromContext::Error: ulo::Error`; `impl Error` on the eleven extractor errors; `BodyExtractionError<E>` gains `std::error::Error`, a `source()` and a forwarding `kind()` | F165 | — |
| 2 | The four lift sites carry the error instead of stringifying it: HTTP answers `ExecutionResult::Err`, RPC/WS/gRPC lift rather than `to_string()` | F84, F1, C11 | 1 |
| 3 | Each extractor error names its kind: `BadRequest` for a body that will not lex, `UnprocessableEntity` for one that will not fit, `Internal` for `BodyAlreadyRead` and `RequestError` | F163 (the 422 half), F164 | 2 |
| 4 | One `Content-Type` policy, `+json` accepted, 415 through `Custom { status, message }` | F163 (the 415 half), F176 | 3, and F136 |
| 5 | `Validated<E>` carries `E::Error` rather than a `String`, and `details()` serialises `ValidationErrors` | F175 | 1 |

### Phase 2 — the macro

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 6 | Markers lower onto the extractor path: one binding function, signature order, `use_extractors` deleted | A16, A17, C3, F177 | 2 (the shared failure path) |
| 7 | `#[param]`'s `default` refused at expansion; `#[query]`'s parsed at expansion; a missing value distinguished from an unparseable one; `#[query("name")]` refused on a non-scalar | A2, A3, C4, F169 | 6 |
| 8 | The marker read searches the parameter's attributes rather than taking the first; the per-key branch stops reading primitive names | F170, F179 | 6 |
| 9 | `ExtractorKind` collapses to `Context`, `Request`, `Optional`, `Extracted`; the `HttpRequest` arm is deleted and the impl carries it; the `Optional` arm goes to the blanket | F166, F168, C23, C24 | 6 |
| 10 | A parameter whose pattern the macro cannot name is refused with gRPC's diagnostic, in the other three macros | F167 | — |
| 11 | The context reference is diagnosed rather than mis-routed: a reference to something that is not the transport's context is refused at the parameter, naming what was written | F166's context half | 9 |
| 12 | The WebSocket connection hooks read their parameters through `FromContext<WsContext>`; `DisconnectReason` becomes an extractor | A6, F171 | — |

### Independent of both phases

| # | PR | Closes |
| --- | --- | --- |
| 13 | `multer`, `validator` and `bytes` re-exported, or `multer::Error` and `ValidationErrors` wrapped | F172 |
| 14 | `Path` reports the error it was given; `PathError::NotFound` deleted or constructed | F174 |
| 15 | One `Multipart` round trip and one test per `parse_bytes` arm | F173 |
| 16 | ADR-0023 gains its pointer to ADR-0040 | F178 |

PR 16 belongs to the ADR branch rather than to a PR of its own, if the ADRs are written first.

## Verification each PR needs

Four probe files were written and removed during the audit. Each is the test its PR owes, promoted
from a `println!` to an assertion.

- **PR 2** — a global error handler that claims everything claims an extraction failure on all four
  transports. Falsify by removing the handler, which must restore the framework's own rendering. The
  HTTP half replaces `extraction_failure_shape.rs`'s negative assertion, which this PR inverts.
- **PR 3** — a table test over the four failure classes asserting the status, on all five HTTP
  adapters. Include `BodyAlreadyRead` through a guard that read the body, which must answer 500.
- **PR 4** — `application/json`, `application/json; charset=utf-8`, `application/problem+json`,
  `application/vnd.api+json`, no header, and `text/plain`, against both `Json<T>` and `Body<T>`; the
  last one answers 415 and the rest answer 200.
- **PR 6** — one handler carrying `#[param("id")] id: i32`, `Json<Dto>`, a custom extractor and
  `&HttpContext`, which does not compile today in any of four ways. Assert the run and the values,
  not the compile.
- **PR 7** — `?n=abc` against a marker with a default answers 400 rather than the default; a
  `default = "abc"` on a `usize` is a compile error rather than a per-request 500.
- **PR 9, PR 11** — the §4 matrix as a `trybuild` suite or as compile-fail fixtures: each refused
  spelling's diagnostic must name what the author wrote.
- **PR 12** — a macro-written `#[on_disconnect]` reads `DisconnectReason`, and an `#[on_connect]`
  reads `Session`. Falsify against a hook taking nothing.

**Each spelling is compiled on its own.** The audit's batched run reported two refusals and hid
twelve unrun cases, because a resolution error aborts the crate before the type checker reaches the
`const` assertions. A suite that puts them in one crate has the same defect.

## The branch

None exists. Nothing was committed by the audit and master is clean at `654c7183`.

## The divergence gate — stop and ask

Approval of this build is approval of what was read and understood. If the build has to depart from
that, it pauses and reports rather than proceeding on judgement.

Divergence: a signature that differs from what §9 describes; an alternative §9 records as rejected
turning out to be necessary; a finding that reorders the sequence, removes a PR or adds one; a
verification that cannot be written as above.

Not divergence: naming, file placement, test names, the order of two PRs inside a phase.

On divergence: state what the memo said, what the tree requires, and what the options are. Wait.

## Couplings that cross this folder

The triage pass owns sequencing across audits; what follows is the input to it, not an order.

- **PR 4 waits on F136**, which the pipeline build's PR 7 answers. 415 is reachable through
  `Custom { status, message }` and that variant does not exist yet.
- **F117 and F111 are ADR-0052's**, and the crate that build picks for query parsing decides `Path`'s
  round trip as well (`extract/path.rs:70-72`). PR 14 touches the same function; sequence them or
  merge them.
- **PR 3's statuses are what ADR-0053 renders.** 0053 owns the envelope and this owns the status;
  they meet at `render_error`, which PR 2 makes reachable for an extraction failure for the first
  time.
- **PR 12 edits the gateway macro**, which the WebSocket audit has not run over yet.
- **F180 and F181 are this pass's findings and DI's work.** They sit in `DI_BUILD_HANDOFF.md` group F
  behind D14, because deleting `ulo::http::Request` needs injectables to be shared first. Nothing in
  *this* build depends on them, and nothing here should wait for them.

## Per-PR checklist

Walk it. The recorded failure mode is a checklist written and not walked
(`feedback-the-law-review-protocol`). The two most often missed here:

- **Production text this change falsifies.** The workspace `CLAUDE.md`'s *Extraction* row states the
  inline 400 and the chain bypass as behaviour, its *Errors* row states the envelope, and
  `crates/ulo/src/extract/mod.rs`'s module doc says an extractor "the handler macro recognises" is
  rejected at compile time, which PR 9 makes untrue in its own terms. `crates/ulo/src/rpc/extractors.rs`
  still documents the removed bare-DTO form (B11) and PR 2 touches that file.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs (F80).

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest there. Do not move
`Last synced HEAD` or the `Docs status` section in `CLAUDE.md`. Append one line per merged change to
`DOCS_AUDIT.md` §18 naming the pages it invalidates; PRs 2, 3, 4, 6 and 7 each invalidate
`extractors/built-in.mdx`, `extractors/markers.mdx` or the error-handling pages, and PR 12
invalidates every page showing a WebSocket lifecycle hook.
