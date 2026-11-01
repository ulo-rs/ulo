# Salvage: performance, render and the campaign README

Subject: the performance audit brief, the render-reachability audit handoff (run and spent 2026-09-23), and the campaign index `README.md` of `PROJECT_SEPTEMBER_SMILE/`.

Tip read: `experiment/di-redesign` at `331c35b9`, 2026-11-01. Old code read where needed from `654c7183`.

Sources read in full (copies under `docs/experiments/di-redesign/salvage/source/`):

| File | Words (approx.) |
| --- | --- |
| `performance/PERFORMANCE_AUDIT_HANDOFF.md` | 700 |
| `render/RENDER_AUDIT_HANDOFF.md` | 1,150 |
| `README.md` | 2,654 |

Also consulted for status: `BUILD_SEQUENCE.md` (rows 30, 58, 83-87, 103, 134, S4-S6), `FRAMEWORK_GAPS.md` (F1, F22, F57, F63, F64, F65, F69, F87, F122, F129, F136, F242-F244), `transports/DESIGN.md` §2.4, the `ulo`, `ulo-transport`, `ulo-http`, `ulo-http-actix`, `ulo-rpc` crates.

Naming note: the DESIGN documents write `fw_*`; the crates on this branch are `ulo-*`. Citations below use the crate names on disk.

## Summary

| Status | Count |
| --- | --- |
| built | 21 |
| superseded | 34 |
| dropped | 8 |
| open | 15 |
| UNVERIFIED | 5 |

Counts are over the 83 rows in the item tables (P, R, C), by the leading word of the status cell.

Five most valuable open items:

1. **P1/P2** - no benchmark surface exists at HEAD and the decision on what a benchmark for this framework measures was never taken. The four-shape taxonomy and the "a number carries its apparatus" rules are the durable value.
2. **R-F69b** - `status_for(Timeout) => 504` survives in `crates/ulo-http/src/render.rs:32`. RFC 9110 §15.6.5 defines 504 as an intermediary's answer for an upstream that did not respond; the ledger entry's question (is 504 the right status for a handler that ran out of its own deadline) was never answered and the rebuild carried the mapping over.
3. **P4** - per-call allocation on the pipeline remains (`Vec` of interceptors built per call, error-handler tier collected per call) with no measurement. F57's claim changed shape but was not resolved.
4. **R-F22** - `render_error`, `problem` and `render_as_is` are still `pub(crate)` in `ulo-http/src/render.rs`. The reachability question is answered by a different route (see R-F22) but one residual case is open.
5. **P5** - the boxing cost of the erased guard/interceptor/handler futures (`Box::pin` at `crates/ulo/src/transport/mod.rs:118`, `pipeline.rs:248`) is the old `async_trait` cost note in a new form, still unmeasured.

## 1. Performance (`PERFORMANCE_AUDIT_HANDOFF.md`)

| ID | What | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| P1 | Premise: no benchmark surface (`benches/`, criterion, CI job, recorded number) | open | `find` for `benches` or `*bench*` outside `target/` returns nothing; no `criterion`/`divan` in any `Cargo.toml`; the only matches for "benchmark" in the redesign docs are unrelated. Still true, and the rebuild added none. |
| P2 | Decision: which of four benchmark shapes to build, taken before any code | open | Never decided. `BUILD_SEQUENCE.md:358` records it as "not a row; its first deliverable is a decision from the user". |
| P3 | F122: every guard and interceptor built before the first guard runs | built | `crates/ulo/src/transport/pipeline.rs:6-9` and `:181-193`: guards are read through `Resolver::entries::<AnyGuard<T>>()` and each obtained only once the one before it admitted; "on a refusal, later guards are never built". Interceptors only after every guard admits (`:213`, step 3). The before/after measurement the handoff wanted was not taken. |
| P4 | F57: three transports merge enhancer tiers per call (Vec + Arc increments + lookups) | open (shape changed) | One shared pipeline now serves every transport (`pipeline.rs` header). Per call it still builds `Vec<Arc<AnyInterceptor<T>>>` (`:213-215`) and collects the global error-handler tier (`:273-274`). The three-copies-per-transport part is gone; whether the remaining allocations matter is unmeasured. Old fix row PIPE-4 (row 30, "tiers pre-merged", merged on old master as #368) is not on this branch. |
| P5 | `async_trait` per-call boxing note; trigger "a benchmark surface or stable `dyn AsyncTrait`" | open (shape changed) | The core no longer uses `async_trait` (no match in `crates/ulo`). Role traits use native `impl Future` (`transport/mod.rs:82`, `:100`) with a hand-written dyn twin (`ErasedGuard`, `:108-120`) that boxes once per call via `Box::pin`; the handler/interceptor chain boxes at `pipeline.rs:248`. `async_trait` remains only in adapters/tests (e.g. `ulo-http-salvo/src/handler.rs`, `ulo-http-rocket/src/handler.rs`). Neither trigger has fired. |
| P6 | `AUDIT3_DISPATCH.md` §9: boxing of `T::Answer` unmeasured | superseded | The `Answer` associated type is replaced by `T::Reply` and `Transport::Cx` (`transports/DESIGN.md`, core `DESIGN.md:306`). The question "what does the erased boxing cost" survives as P5. |
| P7 | Candidate shape 1: dispatch without a socket (ADR-0052 `ServeContext::dispatch`) | superseded | ADR-0052 never landed (not in `docs/adr/`; only 0055 and 0056 are new). The equivalent seam on this branch is `AppService` / `service.rs` `respond` (`ulo-http/src/service.rs:428`, used by `ulo-http-actix/src/service.rs:81`), and every transport has conformance harnesses that drive an app without a bound port. UNVERIFIED that a Criterion-style loop can call it without a socket; it needs a small probe. |
| P8 | Candidate shape 2: per-call allocation counts via a counting global allocator | open | No such harness. This is the shape that turns P4 and P5 into numbers. |
| P9 | Candidate shape 3: end-to-end through each adapter | open | No adapter comparison exists. Adapters now differ in a way the old brief did not know: seven sit on `ulo-http-hyper`/`ulo-server-hyper`, and two runtimes (tokio, smol) exist, so "which server" is a wider axis. |
| P10 | Candidate shape 4: compile time and binary size of the macro layer | open | Not measured. More pertinent at HEAD: the rebuild has 80+ crates and several proc-macro layers (`ulo-handler-codegen`, `ulo-*-macros`). |
| P11 | Rule: every number carries apparatus, machine, profile, commit; one process; `--release`; regression needs both commits | open (rule, not code) | No apparatus exists to apply it to. |
| P12 | Rule: confidence category "Measured" means a number with its apparatus stated; anything else Unverified | open (rule) | Not recorded in any file under `docs/experiments/di-redesign/`. |
| P13 | Policy: no finding filed on cost alone | open (rule) | Holds: no cost-only entry exists in the rebuild's ledger. |
| P14 | Ordering: runs last, after the pipeline build and DI audit | superseded | The build was paused 2026-10-01 and the pipeline and DI were rebuilt. The precondition ("everything it would measure is about to change") is now satisfied, which makes the pass runnable against the new tree. |
| P15 | Deliverable: `PERFORMANCE_AUDIT.md`, ledger findings, committed apparatus (ask first) | open | None produced; the "is the apparatus committed" question was never asked. |

### Value to extract

**P2 and P7-P10, the benchmark taxonomy.** The useful content is the distinction a framework benchmark must draw: measure the framework or the adapter beneath it, and the hard part is separating them. Ordered by cost: (1) dispatch with no socket, the only shape that measures ulo rather than hyper; (2) counting-allocator per-dispatch allocation counts, which are what turn "allocates per message" into a number; (3) end-to-end per adapter, valid for adapter comparison and not for the pipeline; (4) compile time and binary size, which macro-heavy frameworks pay for and nobody has asked about. Source: `PERFORMANCE_AUDIT_HANDOFF.md` "Its first job". At HEAD the cheapest start is shape 2 around one `AppService::respond` call or one `pipeline` run, because it needs no new server and the two open claims (P4, P5) are both allocation-shaped.

**P4, what to count.** Per call on the shared pipeline: one `Vec` for interceptors (`pipeline.rs:213`), one collected `Vec` for global error handlers (`:273`), one `Box::pin` per guard (`transport/mod.rs:118`), one for the handler chain (`pipeline.rs:248`), and the `Arc::clone` of handler info (`:70`, `:137`). The measurable question is whether any of these is worth pre-computing at wiring, which is what PIPE-4 did on the old tree.

**P11-P13, the epistemic rules.** A performance finding is falsified by a measurement under different conditions, not by a probe. The rules (apparatus on every number; one process, one build, never across sessions; `--release`; a regression needs the same benchmark on both commits; Measured vs Unverified) are the only part of the brief that does not depend on the old tree and should be carried to whoever writes the benchmark.

**P15, the infrastructure question.** An audit commits nothing, but the benchmark apparatus is infrastructure and a scratchpad copy means the next pass rebuilds it. The handoff asks the user whether to commit it; unanswered.

## 2. Render (`RENDER_AUDIT_HANDOFF.md`)

Scope claims first, then the seven dispositions, then the incidental items.

| ID | What | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| R-F65 | Three transports invent an error payload a standard specifies | superseded | HTTP bodies are RFC 9457 `application/problem+json` (`transports/DESIGN.md:272`, `ulo-http/src/render.rs:37-41`, tests `ulo-http/tests/errors.rs`). gRPC carries `google.rpc.Status` in `grpc-status-details-bin` (DESIGN :272). RPC and WebSocket keep `{kind, message, details}` (`ulo-rpc/src/codec.rs:224-226`, `ulo-ws/src/envelope.rs:7`, `:437`). The same split ADR-0053 chose, built by the rebuild; the `{status, kind, message}` shape is gone. |
| R-F69a | 422 phrase | built | `ErrorKind::Unprocessable => 422` (`render.rs:29`); the title is the status phrase from RFC 9457. |
| R-F69b | `status_for(Timeout) => 504` | open | `render.rs:32`. The appended note on F69 ("survives ADR-0053 verbatim") still applies. See value below. |
| R-F136 | Second error vocabulary beside `ErrorKind` | built | One `CallError` with `ErrorKind` plus `Details` (`ulo-transport/src/error.rs`, DESIGN §2.4); no per-transport `HttpError`/`RpcError`/`WsError` variants remain. Three statuses no kind has (405, 413, 415) are derived from the error's source type in `render_as_is` (`render.rs:63-90`). |
| R-F129 | Three `safe_render` copies | dropped | There is no user-authored renderer to wrap: rendering is a framework function over `CallError`. Panics are caught at the poll boundaries (`ulo-http/src/service.rs:877-909`, `ulo-grpc/src/dispatch.rs:968`, `ulo-ws/src/connection.rs:1510`) and become `PanicRecovered`. |
| R-F242 | Recovered panic's text reaches the caller (C12), on all four transports | built | `PanicRecovered` maps to `internal()` with a generic message (`ulo-transport/src/error.rs:290-292`, DESIGN :249); the payload goes through `Redacted` (`DESIGN.md:911`, `:1266`). Pinned by `ulo-http/tests/errors.rs:286-292` (`a_panic_in_a_handler_is_a_500_that_withholds_the_message...`, asserts no "secret"). The old S5 decision (row 58 REN-1: message generic, payload on the event) is how the rebuild behaves. UNVERIFIED for RPC/WS/gRPC at the conformance level: not checked here that each suite asserts withholding. |
| R-FB | Fallback literals reached only when a renderer panics | dropped | The three hand-written literals existed because a user-visible `message()` could panic during rendering. Rendering reads only framework fields; `Redacted` text is already a string. No renderer-panic path or literal exists on this branch (no match for `safe_render` or a renderer-panic test). Old tests (`panicking_renderer_falls_back_to_safe_envelope` x3) have no counterpart. |
| R-F243 | Envelope sites outside the routing seam; invariant "an adapter never authors an error body" | built | Each adapter now answers through `AppService`/`service.rs`, which renders. The sole error body an adapter writes at HEAD is actix's bare 400 for a head that does not re-parse (`ulo-http-actix/src/service.rs:72-74`, `HttpResponse::BadRequest().finish()`, no body). That is the one exception to the invariant and has no body to author. |
| R-F244 | actix substitutes `GET /` for a request-line it cannot re-parse | built | `head()` returns `None` on a method, URI or header name/value that does not convert (`ulo-http-actix/src/service.rs:92-100`) and the caller answers 400 (`:72-74`). The doc comment at `:90-91` states the invariant ("a URI that does not re-parse, which actix parsed already and so never yields"). The header drop is now a refusal instead of a silent drop. |
| R-F22 | Application cannot restyle the envelope without enumerating its error types | superseded (one residual open) | The mechanism is different. `#[derive(Classify)]` gives a domain error a canonical rendering with no handler (DESIGN §2.4); an error handler returns `Result<Reply, BoxError>` (`transport/mod.rs:96-100`) so reshaping is by returning another error, and an unclaimed error renders canonically. `render_error` stays `pub(crate)` (`render.rs:47`), so the case "an error handler wants the canonical problem document for a `CallError` it built, as a response" has no public function; it returns `Err(CallError)` and lets the pipeline render. UNVERIFIED that this covers every use ADR-0053's S4 meant (HTTP, RPC and WebSocket in one PR). |
| R-F87 | Handler catching its own error type cannot produce the canonical envelope | superseded | Same mechanism: return `Err(CallError)`, or derive `Classify` on the domain error. `ErrorHandler::handle` returns `Result<T::Reply, BoxError>`, so claiming and reshaping is by returning an error. |
| R-F64 | One RPC error object spells two fields in two cases | dropped | The object is `{kind, message, details}`, all single lowercase words (`ulo-rpc/src/codec.rs:224-226`, `ulo-ws/src/envelope.rs:465-467`). The `statusCode`/`status` mix is gone with the old wire. |
| R-F1 | Extraction failure on RPC/WS renders `Internal` | built | `ExtractError` maps to `BadRequest` or `Unprocessable` before anything else (`ulo-transport/src/error.rs:152-156`, `from_extract`); HTTP additionally 400/413/415/422 (`render.rs:70-86`). |
| R-F63 | RPC load shed answers a word that is not an `ErrorKind` | built | An over-limit call is refused as `ErrorKind::Unavailable` with a `Detail::RetryAfter` (`ulo-rpc/src/dispatch.rs:216`, `refusal` at `:664-666`). The `overloaded`, `unsupported` and `error` strings of the F63 count correction do not appear (grep for the three strings in `crates` returns nothing). |
| R-A23 | `WsError::EventNotFound` declared, rendered, close-coded, never constructed | dropped | The type does not exist (no match in `crates`). |
| R-A24, R-C22 | `Default*ErrorHandler` types never constructed; no `DefaultGrpcErrorHandler` | dropped | No `Default*ErrorHandler` in the workspace. Already closed at `654c7183` per the handoff. |
| R-C13 | `frame_response`'s Stream refusal branch unreachable | dropped | No `frame_response` in the tree. Whether a transport that cannot carry a stream answers a clean refusal is DESIGN-level (RPC call shapes, `transports/DESIGN.md:936`). |
| R-C14 | 405 body differs per adapter | built | 405 with `Allow` is rendered once, in core (`render.rs:63-70`, `ulo-http/src/miss.rs:45`); adapters do not render. |
| R-F22b | Appended note: ADR-0053's "render_error becomes reachable" is framework-wide, build is HTTP-only; gRPC already has `GrpcStatus::from_error(&dyn Error)` | superseded | Single `CallError` renders on all transports through shared `Classify`/`from_boxed`; per-transport rendering is in each transport crate's own file (HTTP `render.rs`, RPC `codec.rs`, WS `envelope.rs`, gRPC `status.rs`). |
| R-F69c | Appended note: F69 closed by ADR-0053 would lose the 504 question | open | = R-F69b. |
| R-F64b | Appended note: F65's "F64 dissolves into this" premise is falsified, nothing owns F64 | dropped | Moot (R-F64). |
| R-ADR | ADR-0052 (HTTP routing seam), ADR-0053 (problem details, render reachable), amendments S6 | superseded | Neither file exists on this branch (`docs/adr/` holds 0001-0051, 0055, 0056). They lived on `docs/http-audit-adrs` (`9d3fb9c2`), which is not among the local or remote branches listed by `git branch -a`. UNVERIFIED whether that branch survives in `old-build-branches-2026-11-01.bundle` at the workspace root. The decisions they carried are in `transports/DESIGN.md` §2.4 and §3. |
| R-S1 | Success side: `IntoOutput<T>` is public; error renderer was `pub(crate)` | superseded | Replaced by `Reply` per transport and `IntoReply`-style conversions (`transport/mod.rs`, `ulo-http/src/response.rs`). Asymmetry remains by design: the error rendering is framework-owned (R-F22). |
| R-S2 | `AUDIT3_DISPATCH.md` §4: a render stays per transport (three `render_error` copies are the decision working) | built | One renderer per transport crate (see R-F22b); the shared piece is the classification (`CallError`), not the bytes. |
| R-S3 | Pairing: F242 with F238 (a framework event tells the caller only what `message()` says; `GuardRejection` the worked example) | built | `GuardRejected` maps to `Forbidden` with a fixed message (`error.rs:293-295`, `FORBIDDEN_MESSAGE`) and `PanicRecovered` to `internal()`; the guard's name stays on the event. |
| R-CAT | The category question: renderer output is category (1) spec-decided, reachability is category (3) the framework's design | open (method) | Method note, not code. See value. |
| R-DISP | "Do not schedule again", and 7 dispositions filed (F242, F243, F244 + 4 appends) | superseded | The ledger entries F242-F244 refer to code that no longer exists on this branch except as recorded above. |

### Value to extract

**R-F69b, the 504 mapping.** HTTP: a handler that exceeds its own deadline is a server-side timeout, but RFC 9110 §15.6.5 defines 504 Gateway Timeout as "the server, while acting as a gateway or proxy, did not receive a timely response from an upstream server". 503 with `Retry-After` and 408 (a client's request that the server timed out waiting for) are both also wrong for a handler overrun. The rebuild keeps 504 for `ErrorKind::Timeout` for every transport (`render.rs:32`, DESIGN :263 row). The question to take to the designer is whether `Timeout` means "the callee exceeded its budget" (500-class, no standard status; 504 is the common convention) or "an upstream did". Provenance: ledger F69 plus the render handoff's appended note.

**R-CAT, the two-category trap.** `project-the-spec-conformance-seam` is the memory; the handoff's reduction is: a renderer's *output* is frequently specified by a standard (so debate there is closed), while its *reachability* is the framework's own choice. Audits of a rendering seam keep the two sections separate or re-litigate the standard. The remaining relevance at HEAD: when a future `CallError` renderer is added (e.g. for a new transport), the output shape follows the spec table in DESIGN §2.4 and the reachability rule is "framework renders, application classifies".

**R-F243/R-F244, the invariant.** "An adapter never authors an error body" and "a conversion across a dependency boundary refuses rather than substitutes" are the two reusable rules. At HEAD the actix adapter is a worked example of both (`head()` returns `Option`, caller answers 400). The remaining structural risk named in F244 (two `http` crate versions agreeing on grammar, held by no test) is still true: actix-http carries `http` 0.2 and ulo `http` 1.x. The new doc comment states the invariant but no test enforces it.

**R-FB, fallback-literal tests.** The retired test idea: drive a domain error whose `message()` panics and assert a canonical literal. It has no direct analogue, but the transferable test is "a classified error whose `Display` panics must still produce a response", since `Redacted` renders text eagerly and a user type's `Display` is called there. UNVERIFIED that such a test exists at HEAD (not searched in `ulo-*-conformance`).

**R-F242, the rule.** `Display` is the diagnostic, `message()` is what the caller is told. Applied to `PanicRecovered` and `GuardRejected` at HEAD. The handoff's S5 decision text is the rationale; the rebuild adopted it.

## 3. The campaign README (`README.md`)

### 3.1 What the README promised

1. An index of eight subject folders, each with handoff, memo and build handoff, and a state column (rows below, C-L).
2. A rule that each subject folder has `artifacts/` for probes, spikes, measurements and probe output, never a shared one.
3. The things that stay outside the folder: the two ledgers, `CLAUDE.md`, `AUDIT3_DISPATCH.md`, `STANDARDS_AUDIT.md`, referenced by bare filename.
4. A list of known cross-subject couplings ("Eighteen are already known"; the bullets actually number 24, see C-X).
5. A pointer that the triage ran 2026-09-24 and its output is `BUILD_SEQUENCE.md`.
6. A shared rule: pre-release, delete rather than deprecate, rename rather than alias (D13 in `DI_AUDIT.md`).

### 3.2 Folder map and what became of each pointer

| ID | Pointer | Status | What became of it |
| --- | --- | --- | --- |
| C-L1 | `di/` handoff, memo, build handoff; D1-D14 settled | superseded | The settled decisions were overtaken by the DI redesign on `experiment/di-redesign` (user's decision 2026-10-01; memory "The outside chat's DI design is the direction"). Files copied to `salvage/source/di/`. |
| C-L2 | `pipeline/` ; D1-D5 settled | superseded | Pipeline order is ADR-0055 and pre-dispatch ADR-0056 (`docs/adr/`), built as `crates/ulo/src/transport/pipeline.rs`. |
| C-L3 | `http/` (memo and build handoff only) | superseded | Rebuilt: `ulo-http`, hyper backend, five adapter crates. ADRs 0052-0054 never landed on this branch. |
| C-L4 | `extraction/` ; D1-D7 | superseded | Extraction is `Param::extract`/`FromCall` per transport (`transports/DESIGN.md` :107 entry), not `FromContext`. |
| C-L5 | `render/` handoff only; run and spent | built | Covered by section 2. |
| C-L6 | `performance/` handoff only; "not run; runs last by its own brief" | open | Covered by section 1. Never run. |
| C-L7 | `graphql/` ; F182-F195, D1-D8 | superseded | Separate crates `ulo-graphql`, `-http`, `-ws`, `-async-graphql`, `-juniper`. |
| C-L8 | `grpc/` ; build approved 2026-09-23, eight PRs | superseded | Not built; rebuilt in `ulo-grpc` with the pipeline unified. |
| C-L9 | `rpc/` ; 23 PRs in three phases | superseded | Not built as planned; rebuilt as `ulo-rpc` plus seven links; request-side call shapes through `Inbound<T>` (`transports/DESIGN.md:929-936`). |
| C-L10 | `ws/` ; D1-D8 | superseded | Rebuilt as `ulo-ws`, `ulo-ws-hyper`, `ulo-ws-conformance`. |
| C-A | `<subject>/artifacts/` rule, added 2026-09-23 | built (as method) | The rebuild keeps probes under `docs/experiments/di-redesign/probes/` and `transports/probes/`. The per-subject rule did not survive as a rule; the reason it existed (session-scratchpad files lost on reboot) still holds. |
| C-O1 | Two ledgers outside the folder | superseded | `FRAMEWORK_GAPS.md` and `DOCS_AUDIT.md` remain at workspace root, outside the repo. The rebuild keeps its own ledger inside the repo (QUESTIONS_*.md, DIVERGENCES). |
| C-O2 | `AUDIT3_DISPATCH.md` §4 measurement rule; `STANDARDS_AUDIT.md` | built (kept) | Both still at workspace root. The measurement rule is the one P11-P13 and R-S2 lean on. |
| C-O3 | Bare-filename references so layout can change | built | Held up: the folder moved into `salvage/source/` without breaking references. |
| C-T | Triage ran 2026-09-24; `BUILD_SEQUENCE.md` is the one control surface | superseded | The sequence was PAUSED 2026-10-01; only the rows whose `[m]` mark shows a merge (e.g. row 30 PIPE-4, #368) landed on old master. Rows 58, 83-87, 103 and 134 are unchecked. The rule "memos and handoffs are frozen inputs" survives only as this salvage. |
| C-D13 | Rule: delete rather than deprecate, rename rather than alias | built (kept) | Followed by the rebuild (no compatibility shims; `DIVERGENCES.md` records removals). |

### 3.3 Cross-subject couplings (each bullet of "What is owed")

The README claims 18 and lists more; the table lists every bullet. A status of "superseded" means the rebuild removed the surface the coupling joined. The owning subject readers carry the detail; this table records only the outcome at HEAD and the evidence I found.

| ID | Coupling | Status | Outcome at HEAD |
| --- | --- | --- | --- |
| C-X1 | DI D2 (token single-bound unless multi) belongs in pipeline PR 5, else F138 reopens | superseded | The container is rebuilt; binding and ambiguity rules are in core `DESIGN.md` §3-§4 (`AmbiguousModule` at :811). F138 is the old globals registration, which no longer exists. |
| C-X2 | DI F73 = pipeline PR 12; pipeline Phase 2 changes trait signatures DI macros emit | superseded | Roles are native-async traits with dyn twins (`transport/mod.rs`); macros are rebuilt. |
| C-X3 | Extraction PR 4 needs pipeline PR 7 (`Custom { status, message }`, F136) for 415 | superseded | 415 is derived from `ExtractError::UnsupportedMediaType` in `render_as_is` (`render.rs:73-80`); no `Custom` arm. |
| C-X4 | Extraction PR 14 and ADR-0052 touch one function (query parsing crate; F117, F174) | UNVERIFIED | Which query parser the rebuild picked, and whether one parser serves query, path struct round-trip and WS handshake, was not checked by this reader (needs a grep of `ulo-http/src/extract/`). |
| C-X5 | F180/F181 built by DI build behind D14; delete `Http::Request` | superseded | `ulo_http::Request` exists on this branch (`ulo-http/src/request.rs`, re-exported at `lib.rs:118`) as a plain struct with `head` and `body`. D14 (Arc for singleton/execution) is a design given by the rebuild, not a gate. |
| C-X6 | GraphQL F182/F184 and extraction F163: one argument on two surfaces (status seam answers one code where a standard gives three) | superseded | Statuses come from the one `render.rs` mapping; 405/413/415 are derived from error source (`render.rs:63-90`). |
| C-X7 | GraphQL F191 = DI ambiguity seen from outside (fixed token string, first import wins silently) | UNVERIFIED | Not checked in `ulo-graphql*`. |
| C-X8 | GraphQL D7 rides HTTP PR 1 `AdapterCapabilities` | superseded | `Capabilities` exists as a struct for RPC links (`transports/DESIGN.md:1051`); the HTTP seam is `Backend`/`BackendLimits` (`ulo-http/src/lib.rs:108`). No `AdapterCapabilities`. |
| C-X9 | F190/F198/F199: list-valued `Sec-WebSocket-Protocol` read as one token; rocket and tungstenite negotiate nothing | superseded | Gateways declare `subprotocols = [..]` in the attribute (`transports/DESIGN.md:812`); handshake is core-owned (`ulo-ws`). Whether negotiation parses the list per RFC 6455 §4.2.2 at HEAD is UNVERIFIED. |
| C-X10 | WS build Phase 2 consumes HTTP build (`Dispatched`, `AdapterCapabilities`); must not precede it | superseded | Neither type exists on this branch; WS upgrades through `ulo-http/src/upgrade.rs` and `ulo-ws-hyper`. |
| C-X11 | WS D8 and HTTP build decide one enum (`Dispatched::Upgrade` carries `WsUpgrade`); PR 21 co-designed | superseded | Same as C-X10: no `Dispatched`. Value: the argument was that shipping the variant with `Arc<WsConnectionCallbacks>` then replacing it breaks one enum twice in a release. Spiked against `Rc<()>` on five adapters. Does not apply to HEAD's design but is the justification for deciding a cross-transport seam once. |
| C-X12 | GraphQL PR 6 blocked by WS PR 2 (F187/F201: `WsError::Refused` writes envelope, no close frame) | dropped | `WsError` is gone from the public surface; close codes are DESIGN-defined (`close_codes` suite named in `SWITCH_INVENTORY.md:97`). |
| C-X13 | Four query parsers is five (WS handshake query map, F206, does not percent-decode) | UNVERIFIED | Same check as C-X4. |
| C-X14 | Four query parsers, three owners; ADR-0052 picks the crate | UNVERIFIED | Same check as C-X4. |
| C-X15 | RPC F226: `pattern_map.insert` last-write-wins, reports nothing, same shape as DI ambiguity | UNVERIFIED | Not checked in `ulo-rpc` (needs a read of its pattern index). Open if the rebuild still inserts into a map without detecting a duplicate. |
| C-X16 | RPC F219 and extraction F163 rhyme, must not be merged (unrouted on subject brokers cannot be raised) | built | `transports/DESIGN.md:965`: "A pattern nothing handles is `Unavailable` where the link signals that nothing is listening, and the client's Timeout otherwise", with a per-link `Capabilities` struct (:1051) saying which links can signal. |
| C-X17 | RPC build Phase 2 rides HTTP PRs 1-9; D1 capability declaration, one `#[non_exhaustive]` type across HTTP, WS, RPC, GraphQL | superseded | Capabilities are per-transport (RPC `Capabilities`, HTTP `BackendLimits`); no shared type. Whether one was warranted is a design question for the designer. |
| C-X18 | RPC PR 13 reserves `RpcMessageCallbacks` taking `Items<RpcData, RpcError>`; read gRPC's `Inbound<T>` | built | `Inbound<T>` is a handler parameter on RPC (`transports/DESIGN.md:929-936`), call shape read from types. |
| C-X19 | gRPC F235 + RPC F222: abandonment observable only on a dropped `ScopedStream`; buffered answers outside it | built | `CancelReason::ClientCancelled` covers WS/RPC cancel frames and gRPC abandonment "on every abandonment hyper reports" (`transports/DESIGN.md:319`); HTTP `Disconnected` (:515); `on_stream_end` (:329). Rows 131-133 (D3-ii) are the replaced plan. |
| C-X20 | gRPC D4 / pipeline PR 7: `Custom { status, message, source }` on all four transports | superseded | One `CallError` with a `source` (`ulo-transport/src/error.rs:148-156`, "the original is kept as the source"); kinds are the fixed 11-variant `ErrorKind`; no `Custom`. Statuses outside the kinds exist only for 405/413/415 on HTTP. The lost capability: a handler cannot name an arbitrary gRPC code outside the kind table. UNVERIFIED whether the rebuild has an escape for that. |
| C-X21 | gRPC F240: ADR-0050 reaching three transports, gRPC matches `Result` by name | superseded | Handler shape is read from types by "two probes" (`transports/DESIGN.md:936`); `#[grpc_methods]` is gone. |
| C-X22 | gRPC F241 / HTTP D6: `Error::details()` dropped on gRPC | built | `Detail` maps one-to-one to `google.rpc` details, carried in `grpc-status-details-bin` (DESIGN §2.4). |
| C-X23 | Pipeline PR 12 decides whether gRPC `add_service` bypass shrinks; `layer()` anchor vs generated method | superseded | `ulo-grpc/src/pre_dispatch.rs` and ADR-0056 (pre-dispatch role). |
| C-X24 | ADR-0053 clause framework-wide, build HTTP-only; decide once in PR 11 (S4) | superseded | See R-F22b. |
| C-X25 | Render pass ran, no memo; F242, F243 appended | built | Section 2. |
| C-X26 | "Neither the DI memo nor the pipeline memo recommends a queue place, by decision" | dropped | Moot; the queue is dead. |

### Value to extract (README)

**C-X20, the lost capability.** Pre-rebuild, ADR-0039's `caused_by` let a `GrpcStatus` carry a hand-named code and a typed error together, so `#[catch(MyError)]` stayed reachable on a status outside the eleven kinds. D4's settled answer was to propagate this to all four transports as `Custom { status, message, source }`. The rebuild keeps `source` (`CallError::source`) but fixes the kind table. A designer review should confirm that the loss (no way to name a gRPC code, an HTTP status or a close code the 11 kinds do not reach, other than the three derived HTTP statuses) is intended. Provenance: README (the D4 bullet), `GRPC_AUDIT.md` §8 (D4), `transports/DESIGN.md` §2.4 table.

**C-X11, one enum twice.** The argument for co-designing a seam used by two transports in the same release (ship it with the type that will be replaced and the enum breaks twice) is general. The same risk exists at HEAD if a `Capabilities` type is added to HTTP after RPC's.

**C-X19, one root, one answer.** The lesson in the README's F235/F222 bullet: when two transports show "an execution never told its caller has gone", deciding it for one answers a quarter of it. The rebuild decided it once via `CancelReason` (DESIGN :319).

**C-X16, rhyming findings must not be merged.** F219 (structural: three transports cannot raise the event) and F163 (a hardcoded status) share a shape but have different fixes. At HEAD the structural half is recorded as `Capabilities`; the status half was removed with the macros.

**C-A, artifacts.** The README's reason for per-subject `artifacts/` (six handoffs pointed at scratchpad files which are session-scoped) is still true of the files referenced by those handoffs. If a handoff cites a probe path, check `salvage/source/<subject>/artifacts/` first. For performance and render, no artifacts exist.

## 4. Worth discussing with the designer

1. **Is `Timeout` -> 504 correct for a handler that ran out of its own deadline?** (R-F69b.) Recommendation: keep 504 only if documented as the convention, or add a note in `transports/DESIGN.md` §2.4 saying why 504 was chosen over 500/503.
2. **Should a benchmark apparatus be committed, and which shape first?** (P2, P15.) Recommendation: shape 2 (allocation counts) around one dispatch, committed under `benches/` or `crates/ulo/benches`, because it measures P4/P5 with no new server.
3. **Is the loss of a way to name an arbitrary status acceptable?** (C-X20.) Recommendation: yes for HTTP and RPC; confirm for gRPC, where `grpc-status` has 17 codes and `ErrorKind` has 11, so four codes (`CANCELLED`, `UNKNOWN`, `DATA_LOSS`, `OUT_OF_RANGE`, among others) are unreachable by a handler.
4. **Should `render_error`-equivalent be public?** (R-F22.) Recommendation: no, as long as `Err(CallError)` from an error handler reaches the same renderer and a test pins it.
5. **Is a renderer-with-a-panicking-`Display` test needed?** (R-FB.) Recommendation: one conformance case per transport; cheap.
6. **Does the 4-way pre-existing question "one shared capabilities type or per-transport" need to be decided?** (C-X17.) Recommendation: per-transport, as built, with a note that the choice was made.

## 5. Not verifiable here

- Whether RPC, WS and gRPC conformance suites assert that a panic's text is withheld (R-F242 beyond HTTP): needs a read of the three suites' error cases or a run.
- Whether a counting-allocator loop can drive `AppService::respond` with no socket (P7): needs a probe build.
- Which query-string parser the rebuild uses and whether WS handshake queries percent-decode (C-X4, C-X13, C-X14).
- Duplicate-pattern handling in the RPC pattern index (C-X15).
- Whether `Sec-WebSocket-Protocol` is parsed as a list at HEAD (C-X9).
- Whether branch `docs/http-audit-adrs` (ADRs 0052-0054) is recoverable from `old-build-branches-2026-11-01.bundle` (R-ADR).
