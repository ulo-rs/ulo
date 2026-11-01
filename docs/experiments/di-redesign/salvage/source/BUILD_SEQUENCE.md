# The root build sequence — Project September Smile

Triage run 2026-09-24 against master `654c7183`, over the ten subject folders beside this file:
eight build handoffs, eight memos, the render pass's dispositions, the performance brief, the three
ADRs on `docs/http-audit-adrs` (`9d3fb9c2`), `FRAMEWORK_GAPS.md` to F244, the campaign `README.md`,
project memory, and the session transcripts the audits ran in.

**This file is the one control surface for the build.** It carries the order every PR runs in,
the state of every row, the sign-offs the build waits on, the mechanical loop each row follows, and
the halt log. It adds nothing to any memo or handoff and changes nothing in them. Where this file
and a handoff disagree about *order*, this file wins, because sequencing is what the triage pass
owns (`DI_AUDIT.md` D7, `README.md`). Where they disagree about *what a PR does*, the handoff wins
and the disagreement is a halt.

Row ids name the handoff row they execute: `HTTP-3` is `HTTP_AUDIT.md` §10's PR 3, `DI-19` is
`DI_BUILD_HANDOFF.md`'s PR 19, `EXT-4`, `GQL-5`, `GRPC-7`, `RPC-13`, `WS-21`, `PIPE-12` likewise.
`REN-1` is the render pass's one build item. Rows the triage added carry an `S` sign-off and do not
start until it is signed.

---

## 0. Standing orders

The rules the build runs under. Every one is the user's, stated 2026-09-20 (`7e9d8ede`, HTTP
audit), 2026-09-21/22 (the DI and extraction sessions) and 2026-09-24 (the triage brief).

**0.1 The plan is frozen.** The memos, the build handoffs, the audit briefs, the render
dispositions and the campaign README are read and executed, never edited. The build writes: code
and tests; ADRs exactly as a handoff instructs (a proposed record at the row that says so, the flip
to `accepted` at the row that implements it, and the correction of a clause the implementing PR
finds false in the tree — `feedback-a-record-written-ahead-of-code-is-a-prediction`); the two
ledgers; the workspace `CLAUDE.md` rows a change falsifies; `DOCS_AUDIT.md` §18; and this file.
Nothing else. The memo-form's rule that a built section is marked in its memo is suspended for this
campaign: status lives in §3's state column only, so no memo is touched.

**0.2 Divergence halts. Nothing else does.** Divergence is the union of the eight handoffs' lists:
a type or signature that differs from what a memo or ADR writes; an alternative a memo records as
rejected turning out to be necessary; a variant, parameter or capability the design does not name;
a finding that reorders the sequence, removes a row or adds one; a verification that cannot be
written the way the handoff describes it; a decision whose rejected alternatives turn out not to
have been the only ones; a row's plan text contradicting another plan document; the named per-build
tripwires (an adapter whose upgrade does not work at the dispatch point; `AdapterCapabilities`
settling on a shape RPC cannot use; a socket type losing `Send + 'static`; the graphql-sse binding
unable to answer both bindings from one taxonomy; gRPC's PR 3 needing a change to `ulo-build`'s
public API or shipping only a diagnostic; a conformance case failing for a reason unrelated to the
PR). On divergence the build writes a §6 entry — the row, the plan's words quoted with file and
section, what the tree requires, the options, a recommendation — pushes the branch as a draft PR,
and ends the turn. It does not proceed on judgement, does not pick the smaller version, and does
not build around the gap.

**0.3 Not divergence:** naming, file placement, test names, the order of two rows this file marks
independent, splitting one row into two PRs. Merging two rows into one PR is allowed only where §2
says so.

**0.4 No pausing otherwise.** The build does not ask for a go-ahead per row. The go-ahead is this
file: a row whose `Needs` are open on the stack or merged, and whose `S` items are signed, is
authorised.

**0.5 The Law runs before every push and every PR open.** A dedicated agent, in the background,
reviews every commit message before the branch is pushed and every PR title and body before the PR
is opened, against the four rule files. §5 carries the brief. Only its findings surface in the
session. A finding is a claim: verify it before acting, and where it is wrong about a mechanism
while right about the substance, say so in the halt log or the row note.

**0.6 Stacking, literally.** Every row branches from the previous row's branch tip, and its PR
targets that branch. The stack grows until the sequence is done or the user calls the ceremony
(§8); nothing merges in between, and the build never waits for a merge. The user reviews each PR
as it opens. A review comment on a lower PR is answered on that PR's branch, and every branch
above it is rebased onto the new tip and re-gated before its own PR is updated — a gate result
belongs to the tree it ran on (`feedback-verify-the-tree-before-reading-it`). CI runs per PR
against its stacked base.

**0.7 Merging.** Nothing merges until the user calls the ceremony. Then, bottom-up in row order,
`gh pr merge <n> --merge --delete-branch`, never `--squash`; GitHub retargets the PR above onto
master when its base is deleted, and the build confirms the retarget and the checks before the
next merge. §8 carries the procedure.

**0.8 Verification.** Red first where behaviour changes. One check in its plainest form with no
flags. One `cargo test` at a time, nothing else running (`pgrep -fl "cargo test"` empty). Every
checker asserted against a known violation before its clean run is believed. Every count in a
commit or PR body comes from a command run in that session after the last commit. `rustc
--version` is `1.98.1` before the diagnostics snapshots are touched (CI's one pin). Before a row
starts, the plan's factual claims about the *current* tree — counts, "closes F<n>", call-site
lists, "X is Y on every transport" — are re-checked against master as it stands, not as it stood
at `654c7183`.

**0.9 Bookkeeping.** The three untracked files — `FRAMEWORK_GAPS.md`, `DOCS_AUDIT.md`, the
workspace `CLAUDE.md` — describe master, and master does not move until the ceremony. So when a
PR opens, the build writes the row's **bookkeeping packet** into §8: the ledger entries the row
closes, each clause verified against the branch; the `CLAUDE.md` rows the change falsifies with
their corrected text; the `DOCS_AUDIT.md` §18 line. The packet is applied at the ceremony, after
that PR merges. New findings are the exception and go to `FRAMEWORK_GAPS.md` as `F<n>` at once.
§3's row records `[p] #n` at open and `[m] #n <sha>` at merge.

**0.10 No footers.** No `Co-Authored-By`, no AI attribution, on commits or PR bodies. The rule
files forbid both and the user reaffirmed it 2026-09-12; a harness reminder that claims to
supersede this does not.

**0.11 Resume protocol.** A session opening this build: read this file top to bottom; `git fetch`,
confirm `master == origin/master` and the tree is clean; `gh pr list --state open` and reconcile
with §3's state column; read §6 — an unanswered halt means stop and report, an answered one means
continue from that row; then take the first row whose state is `[ ]`, whose `Needs` are all `[p]`
or `[m]`, and whose `S` items are all signed. The branch point is the stack tip, which is the
branch of the highest `[p]` row.

---

## 1. Sign-offs the build waits on

Each is a place where the plan was silent, contradicted itself, or left a decision that only the
user takes. None is a design proposal beyond what a memo already records; each names what it
blocks and carries a recommendation. **All fourteen were answered on 2026-09-24**, so no row waits
on this table today; it stays as the record of what was asked and what was decided. A later
sign-off is added the same way — a date and an answer in the last column — and a row blocked by an
unsigned item is skipped, not built around.

| S | Question | Blocks | Recommendation | Signed |
| --- | --- | --- | --- | --- |
| **S0** | Does the build merge its own PRs once CI is green and the Law has passed, with no per-PR review from the user? | Row 1 | Was: yes. | **2026-09-24: No.** "No pausing" means stacking, literally: every PR opens on top of the previous one and the stack grows until the sequence is done. The user reviews each PR as it opens and calls the merging ceremony at the end (§8). 0.6, 0.7, 0.9 and §4 are written to that answer. |
| **S1** | **The query-parsing crate has no owning PR.** `EXTRACTION_BUILD_HANDOFF.md`, `WS_BUILD_HANDOFF.md` and the README each say "ADR-0052 picks the crate"; ADR-0052 names no crate and `HTTP_AUDIT.md` §10 leaves F117 out of its sequence on purpose. Five parsers wait on it: `Query<T>`, the `#[query]` marker, `Path`'s round trip, `RequestPart` (after DI-21/22), the WebSocket handshake map (F206). | Row 80 (`HTTP-Q`), and through it WS-19, EXT-14 | Add one row in the HTTP tranche after HTTP-3: one query parser for the framework, on a crate that carries repeated keys and percent-decodes once — closes F117, and F206 becomes a consumer (WS-19). Candidates to verify, not assert: `serde_html_form` (a `serde_urlencoded` successor with sequence support) or `serde_qs`. The user names the crate or declines the row; declined, F117 stays open and WS-19/EXT-14 run against `serde_urlencoded`. | **2026-09-24: `serde_html_form`.** Verified against the registry: 0.4.1 (2026-06-30), MSRV 1.63, a fork of `serde_urlencoded` with the same `from_str` / `from_bytes` / `to_string` surface plus sequence fields (`Vec<String>` from a repeated key), built on `form_urlencoded`, already in the lock file, which does the percent-decoding. Six call sites in four files under `crates/ulo/src/http` move, one of them deleted by DI-21 first. One behaviour change to document: an empty numeric value (`n=`) reads as `None` for an `Option`. |
| **S2** | **gRPC D3 was settled as (ii) — an execution learns it was abandoned on every transport and both answer shapes — sequenced gRPC first. Only gRPC has rows (GRPC-7, GRPC-8).** No handoff carries the HTTP, RPC or WebSocket buffered-answer drop site. | Rows 131–133 | Add three rows after GRPC-8 has validated the drop-site design, one per transport, under ADR-0021's clause. Or file with a trigger. | **2026-09-24: approved, three rows.** The RPC half is also filed as F245: the in-band cancel frame aborts a unary call's task without firing its token, while two docs say it does. |
| **S3** | **RPC-17's word.** F63 and F223: which `ErrorKind` name replaces `overloaded` (tcp/udp load shed), `unsupported` (a stream on a transport with no stream grammar), and `error` (a udp stream item too large for a datagram). The gRPC audit found `statuscodes.md` folds a shed load into `RESOURCE_EXHAUSTED`, which `code_for` already maps from `TooManyRequests`; F63's own note names `Unavailable` as the better fit. An unauthorised substitution of exactly this word is the incident `feedback-a-deferral-is-filed-when-agreed` records, so only the user's answer moves it. | Row 115 (`RPC-17`) | `overloaded` → `TooManyRequests` on the gRPC-parity argument; `unsupported` → `Unimplemented`, and note that RPC-9's capability refuses the case at `bind()` so the branch may be unreachable afterwards; `error` → `Internal` (the item is the server's, not the caller's). | **2026-09-24: approved as recommended.** |
| **S4** | **HTTP-11's reach.** ADR-0053's clause "`render_error` becomes reachable" is framework-wide and the HTTP build is HTTP-only; `rpc/rpc_error.rs` and `ws/ws_error.rs` keep private renderers no row touches. The README says decide it once in PR 11; gRPC already ships the public borrowing signature. | Row 103 (`HTTP-11`) | HTTP-11 makes the renderer reachable on HTTP, RPC and WebSocket in one PR; F22 and F87 close whole rather than by a third. | **2026-09-24: approved.** |
| **S5** | **F242's fix.** The render pass located it beside PIPE-8 with GRPC-2 and recorded the candidate: `PanicRecovered::message()` answers the generic literal, the payload stays on the struct's public field where `#[catch(PanicRecovered)]` reads it and the catch sites already log it; the three tests assert the generic text. The user deferred the decision to triage (2026-09-24). | Row 58 (`REN-1`) | Approve the candidate as the decision. It is the rule `GuardRejection` already follows — `Display` is the diagnostic, `message()` is what the caller is told — applied to the one event that escaped it. | **2026-09-24: approved.** |
| **S6** | **Amendments to the three proposed ADRs on the branch**, made while they are still `proposed`: (a) ADR-0052: strike "runs the global interceptors" and name the tier a miss is offered to; add the F125 constraint on `dispatch`; `Upgrade` carries `WsUpgrade` per WS D8 (all three already sanctioned by `HTTP_AUDIT.md` §10 and `WS_AUDIT.md` D8); plus the invariant *an adapter never authors an error body* (F243, F244, the render pass's disposition). (b) ADR-0053: "F69 closes" becomes the 422 phrase alone with `status_for(Timeout) => 504` kept open; "one renderer instead of six" corrected to the count the tree holds; the reachability clause scoped per S4. | Rows 76, 81, 103 | Yes to all. (a)'s fourth item and all of (b) are the only clauses not already ordered by a memo; both come from the render pass the user closed as spent. | **2026-09-24: approved** — the ADRs are part of the build. |
| **S7** | **WS-9's position.** `WS_BUILD_HANDOFF.md` puts PR 9 "after 7, 8", puts PR 8 behind PR 21, and then says PR 9 "gates everything after it" and that PR 21's verification is "every case in §12 passes unchanged on all five" — which needs the suite before PR 21. | Row 2 (`WS-9`) | WS-9 follows WS-7 directly, before the cluster, carrying the F196 assertion and the passing cases as the floor; WS-8 extends the suite with the F197 assertions after WS-21. | **2026-09-24: approved.** |
| **S8** | **`HTTP-3` and `WS-21` as two PRs.** `HTTP-3` introduces `Dispatched::{Respond, Upgrade(WsUpgrade)}` and the `WsUpgrade` type; `WS-21` merges immediately after with the transport trait, the core loop and the five adapters' connection seams; the adapter migrations `HTTP-4..8` then mount on both. If the enum cannot be introduced without the loop, the two rows become one PR. | Rows 81, 82 | Two rows, one PR permitted (0.3 exception). | **2026-09-24: approved.** |
| **S9** | **RPC Phase 3 (rows 119–122) is its own approval** — `RPC_BUILD_HANDOFF.md` says so. It adds a feature where every other row closes a defect, and owes one ADR. | Row 118 | Sign now or at the gate; the rows are in the sequence either way. | **2026-09-24: signed.** Row 118 is open. |
| **S10** | **The capability record's shape.** `RPC_BUILD_HANDOFF.md` allows either one shared ADR or extending ADR-0052. ADR-0052's subject is the HTTP route table and it carries `AdapterCapabilities` inside that decision. | Row 76 (`HTTP-1`) | One separate record — *a transport declares what it can carry* — written at HTTP-1 covering `AdapterCapabilities` (HTTP), `negotiates_extensions` (WS-20), `RpcCapabilities` (RPC-9) and the GraphQL engine SPI's shape (GQL-3), with a pointer under 0052's capability paragraph. | **2026-09-24: approved.** |
| **S11** | **RPC D6 has no PR.** Settled as a CI job for nats, redis, mqtt and rabbitmq on merge and a `make` target for kafka; `RPC_BUILD_HANDOFF.md`'s tables carry no row for it. | Row 11 (`RPC-CI`) | Add the row after RPC-5, in the free-standing wave — the suite passes today, so it can gate today. | **2026-09-24: approved.** |
| **S12** | **F120 has no closing row.** ADR-0052's consequences say "`app.routes()` becomes a read"; no HTTP row lists F120. | Row 79 (`HTTP-2`) | HTTP-2 ships the read accessor and closes F120; otherwise F120 stays open and is not the build's. | **2026-09-24: approved.** |
| **S13** | **PIPE-12's gRPC anchor.** Pipeline D2 settles a pre-dispatch role on all four transports and not where it anchors on gRPC; `GRPC_AUDIT.md` §8 D2 recommends above tonic's router in the `Server::builder().layer(…)` stack, which covers `add_service`d services. The user approved the gRPC decisions as a set. | Row 101 (`PIPE-12`) | Above the router, as recommended. | **2026-09-24: approved.** |

---

## 2. Sequence decisions the triage took inside the plan's own allowances

Listed so they are visible, not because they need a signature: each is sanctioned by a memo, a
handoff or the README.

- **The free-standing rows go first** (wave 1). Every one is marked independent by its own
  handoff, none touches a surface a later wave rewrites except where the handoff says to ship it
  anyway (WS-7), and they close the MUST-class and data-loss defects before any design risk is
  taken.
- **DI before the pipeline.** `DI_AUDIT_HANDOFF.md`: "everything sits on it"; the pipeline build's
  PR 5 needs DI's D2 (DI-7) and its PR 11 edits provider role registration by scope, which DI-6
  also touches. Pipeline PRs 1–4 are pipeline-internal and run in wave 1.
- **`PIPE-6` is absorbed by `DI-2`.** Both handoffs say whichever runs first, the other drops that
  part; DI-2 runs first.
- **`DI-14` drops its C20 half.** README: F191 and D6 — "the two builds should not both write a
  fix for it". GQL-4 closes C20 by deleting the token.
- **`RPC-7` sits beside `DI-9`.** README: F226 is the DI ambiguity finding at a fourth site,
  "whoever fixes the three container sites should take this one in the same pass". It stays its
  own PR.
- **`GRPC-3` runs after `PIPE-9`** (README's placement) **with the handoff's full scope** — the name
  matching removed, `IntoOutput<Grpc>` in `ulo-grpc`. The memo §7's "diagnostic only, structural
  half sized elsewhere" predates the approval; the handoff written at approval supersedes it and
  makes the diagnostic-only version a divergence.
- **`GRPC-2` and `REN-1` ride beside `PIPE-8`.** `RENDER_AUDIT_HANDOFF.md`: F242 with F238, "both
  are one rule about what a framework event tells a caller".
- **GraphQL Phase 2 and 3 run after `HTTP-10` and `HTTP-11`**, so the new crate is written once
  against `StatusCode`, `HeaderMap` and the problem-details renderer. GQL-3 needs only HTTP-1's
  shape and nothing needs GQL-3 early.
- **Adapter migration order: poem, salvo, rocket, axum, actix.** The spike covered poem and salvo;
  rocket already ships the shape; axum and actix are inferred, and actix goes last so a divergence
  there stops with the most done. F244 rides the actix row.
- **The capability rows follow `HTTP-1` directly** (WS-20, RPC-9) so the shared type is settled
  while it is fresh; the rest of RPC Phase 2 follows HTTP-9 as the README says.
- **ADR numbers are provisional.** They follow write order: 0055/0056 pipeline, 0057/0058 DI,
  0059/0060 extraction, 0061 capability, 0062 WebSocket D8, 0063/0064 RPC, 0065 RPC Phase 3,
  0066/0067 GraphQL. The number a row uses is whatever is free when it writes.
- **What is not in the sequence stays out.** Ledger entries no handoff assigns a row — F5, F6,
  F11, F16, F34, F35, F38, F39, F40, F43, F44, F45, F46, F47, F48, F52, F55, F58, F59, F61, F64,
  F66, F70, F71, F72, F75 (parked), F76, F79, F80, F82, F85, F86, F88, F89, F90, F96–F102, F110,
  and the residuals the audits named — remain open. F53 and F54 are spent by the extraction memo's
  D6 without a row. Taking any of these is a scope change and a halt.

---

## 3. The sequence

State: `[ ]` pending · `[b]` branch open · `[p] #n` PR open on the stack · `[m] #n sha` merged at
the ceremony · `[x]` absorbed by another row · `[h]` halted, see §6.

### Wave 1 — free-standing (no design risk, no cross-build dependency)

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 1 | WS-7 | Poll the read half once more after a Close, four adapters; verify against axum as the control | F196 | — | [m] #341 3cc3651 |
| 2 | WS-9 | Promote `ws_wire_probe.rs` to the `WsAdapter` conformance suite, stamped per adapter, tungstenite and rocket exceptions written in; F196 asserted, passing cases as the floor | F36 | 1, **S7** | [m] #342 3cc3651 |
| 3 | WS-1 | Correct the `close_code` doc comment (the registry has auth codes) | half F202 | — | [m] #343 3cc3651 |
| 4 | WS-2 | `WsError::Refused` answers a close frame on the message path | F201 | — | [m] #344 3cc3651 |
| 5 | WS-3 | Pin the `ulo-ws-redis` broadcast envelope in a test | part F210 | — | [m] #345 3cc3651 |
| 6 | WS-4 | `#[non_exhaustive]` on `WsMessage` | F208 | 5 | [m] #346 3cc3651 |
| 7 | WS-5 | Drop `Session`'s `Default`; `new()` becomes `pub(crate)` | F214 | — | [m] #347 3cc3651 |
| 8 | RPC-1 | `data_to_json` refuses a `Binary` payload on tcp and udp before any I/O | F217 | — | [m] #348 3cc3651 |
| 9 | RPC-4 | `Broker` for `ulo-rpc-tcp` and `ulo-rpc-udp` | half F233 | — | [m] #349 3cc3651 |
| 10 | RPC-5 | The three probe cases promoted into `conformance_suite!`'s default list as assertions | half F233 | 9 | [m] #350 3cc3651 |
| 11 | RPC-CI | The conformance suite runs in CI for nats, redis, mqtt, rabbitmq on merge; a `make` target for kafka | D6 | 10, **S11** | [m] #351 3cc3651 |
| 12 | RPC-6 | Kafka's `Timeout::After(5s)` literal becomes the client timeout and the server's remaining budget | F230 | — | [m] #369 3cc3651 |
| 13 | RPC-8 | A stable Kafka reply topic per logical client; partition and replication knobs on `ensure_topics` | F227 | — | [m] #354 3cc3651 |
| 14 | RPC-2 | The two client-lifecycle docs stop claiming a call and a flush nothing performs | F225 (doc half) | — | [m] #352 3cc3651 |
| 15 | RPC-3 | The lane doc states what the `err` lane means | F232 | — | [m] #353 3cc3651 |
| 16 | GRPC-1 | `split_at_checked` in `parse_grpc_timeout`, plus a non-ASCII case | F239 | — | [m] #355 3cc3651 |
| 17 | GRPC-7 | Fire the execution's token when the deadline passes; delete `CancellationToken`'s falsified sentence | F236 | — | [m] #356 3cc3651 |
| 18 | GRPC-8 | A drop guard on the unary path, disarmed at a clean return; the drop-site design is validated here | F235 | 17 | [m] #357 3cc3651 |
| 19 | GRPC-6 | The metadata view: ASCII multimap plus a binary map, `headers_all(k)` and `header_bin(k)` beside `header(k)`; `grpc-status-details-bin` written | F237, F241 (second half) | — | [m] #370 3cc3651 |
| 20 | GRPC-4 | `to_tonic`'s and `Error::details()`'s docs state what travels on gRPC until row 19 | F241 (doc half) | — | [m] #358 3cc3651 |
| 21 | GRPC-5 | A startup `warn` per service registered through `add_service`, naming the bypass | gRPC D2 (half) | — | [m] #359 3cc3651 |
| 22 | GQL-1 | The playground is served as `text/html`, both crates | F185 | — | [m] #360 3cc3651 |
| 23 | GQL-2 | Delete the juniper crate's header comment that contradicts the ledger it cites | F193 | — | [m] #361 3cc3651 |
| 24 | EXT-13 | `multer`, `validator`, `bytes` re-exported, or their types wrapped | F172 | — | [m] #362 3cc3651 |
| 25 | EXT-15 | One `Multipart` round trip and one test per `parse_bytes` arm | F173 | — | [m] #363 3cc3651 |
| 26 | PIPE-ADR | The two pipeline ADRs, `proposed`: *the pipeline's order* (D1, D3, D4, §8, the API pass, the enhancer grammar) and *the pre-dispatch role* (D2); provisional 0055, 0056 | — | — | [m] #364 3cc3651 |
| 27 | PIPE-1 | One ordered enhancer vector; the closure form as the third spelling | F121, F56 | 26 | [m] #365 3cc3651 |
| 28 | PIPE-2 | Guards evaluated before any interceptor is built, built one at a time | F122 | 26 | [m] #366 3cc3651 |
| 29 | PIPE-3 | The stale texts and the not-found diagnostic's article | F132 | — | [m] #367 3cc3651 |
| 30 | PIPE-4 | `Resolved<T>`, the tiers pre-merged, `closure_leaf::<Grpc, _>`, one `safe_render` | F128, F129, F57, F74, F60 | 26 | [m] #368 3cc3651 |

### Wave 2 — DI, the foundation

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 31 | DI-ADR | The two DI ADRs, `proposed`: *the container answers an ambiguous question, or refuses it* (D1–D3, pointer under ADR-0029) and *a provider declaration is a value* (D4, D8–D12); provisional 0057, 0058 | — | — | [p] #372 |
| 32 | DI-1 | `Provide::{value, factory, alias}`, `T::provide()`, `.under`/`.multi` as one blanket over `ProviderFactory`, `IntoFactory` per arity, async-only; a role-typed token makes a value or factory take that role (Halt 3); rebuilt on ADR-0059: every key a type, `Key`/`key!`, the `_key` forms, a role from the slot's `Value` (Halt 6) | — | 31 | [p] #373 |
| 33 | DI-2 | Migrate every call site; delete `provide!` and the four macros; `__ulo_provider_factory` leaves the public surface. **Absorbs PIPE-6**; call sites onto `provide!`, `#[inject(CONST)]` checked against the token's type (Halt 4); rebuilt on ADR-0059: string keys, `Token<T>`, `APP_*` and the by-token lookups deleted, `#[inject(K)]` checked against `K::Value`, `for_root_keyed::<K>` (Halt 6) | F161, F162, F143, F148, F50 | 32 | [p] #374 |
| 34 | DI-3 | `#[module]`'s four keys parse expressions; `exports:` lists key types, the one builder form left once Halt 6 took strings out; `Extension<T>` implements the declaration trait (done by row 33, which needed it once `providers:` goes through the trait); one `.provider(value)` and one `.controller(value)` | F144, C18 | 33 | [p] #375 |
| 35 | DI-4 | `#[new]` is the only constructor; `init = "…"` deleted with `#[controller]`'s refusal message; `init = "from_request"` deleted | F145, F156, F157 | — | [p] #376 |
| 36 | DI-5 | `#[default]` beside `#[new]` refused naming both; construction mistakes report against the user's items | F158, F159 | 35 | [p] #377 |
| 37 | DI-6 | A lifecycle hook that cannot fire is refused; the execution and transient rejections, unreachable by construction, replaced by const checks that fire; a per-call target's init `Err` fails the call | A9, C17 | 35 | [p] #378 |
| 38 | DI-7 | Multi becomes ordinary token semantics; a duplicate single binding is refused naming both; `APP_GUARD` becomes a multi token; `APP_INTERCEPTOR` with it; single vs many per key, `into` declared, mixing refused (Halt 4) | F138, F31 | 33 | [p] #379 |
| 39 | DI-8 | Insertion-ordered storage for the four maps; the lifecycle ordering contract on every phase | F140 | — | [p] #380 |
| 40 | DI-9 | `.or_global()` narrowed to exported and global tokens; the two-modules-one-token ambiguity refused at startup and `AmbiguousModule` where startup cannot see it. **0057 flips to `accepted`** | F141, F142 | 38 | [p] #381 |
| 41 | RPC-7 | `create` refuses two controllers declaring one pattern, naming both — the refusal shaped like ADR-0029's | F226 | 40 | [p] #382 |
| 42 | DI-10 | The `Vec<Arc<dyn Trait>>` branch extracted and shared with the controller resolver | F153 | — | [p] #383 |
| 43 | DI-11 | The two twin field resolvers collapsed into one parameterised on the map and the execution | F155 | — | [p] #384 |
| 44 | DI-12 | A factory declaring an execution-scoped dependency is refused at startup | F154, A27 | — | [p] #385 |
| 45 | DI-13 | `Injectable` becomes `Registration`. **0058 flips to `accepted`** | D11 | 33 | [p] #387 |
| 46 | DI-14 | `UloApplication::new` becomes `pub(crate)`; `ModuleRef`'s two by-type lookups call `token_of`. **C20's GraphQL half dropped → GQL-4** | F147, C19 | — | [p] #388 |
| 47 | DI-15 | A startup check joins the module identity | F152 | — | [p] #389 |
| 48 | DI-17 | The unreachable retry budget deleted; the per-module cycle reports through `find_dependency_cycle` | F146, F149 | — | [p] #390 |
| 49 | DI-16 | `Arc<dyn Provider>` across the SPI, the container and the macro expansions | F150 | 45 | [p] #391 |
| 50 | DI-19 | An injectable is shared: `Arc<T>` for singleton and execution scope, by value for transient; the field written `Arc<T>`; the provider macro's `#[derive(Clone)]` deleted in the same PR; a field of a `Shared` handle type injects by value (decision of 2026-09-28) | D14, F7's cause | 49 | [p] #392 |
| 51 | DI-20 | `ExtensionFactory<T>` deleted; the container answers any `Extension<T>` token from the execution's bag | F181, F144's `Extension` half, half F160 | 50 |[p] #393 (draft, Halt 9) |
| 52 | DI-18 | The in-source doc defects; `ExtensionsFactory` renamed; `Module::_get_controller_by_token` deleted; the `CLAUDE.md` rows naming `ProviderContext` and `provider_scope`; the owed `Execution` variant criterion sentence (`project-provider-context-variant-criterion`) | F151, F160, B23 | 51 | [ ] |
| 53 | DI-21 | `Request` loses its two precomputed maps | F180 (first half) | — | [ ] |
| 54 | DI-22 | `ulo::http::Request` deleted; `RequestPart` registered as a `Provider` and a `FromContext<HttpContext>` | F180 | 50, 53 | [ ] |

### Wave 3 — the pipeline's surface, extraction's error type, and the rows that ride them

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 55 | PIPE-5 | One global registration surface per transport per role, taking a token or a value, carrying DI D2's single-bound-unless-multi refusal; the per-transport global tokens are role-typed (Halt 3); the role collections are the global sets, `APP_*` the every-transport ones (Halt 4) | F133, F134, F135 | 38 | [ ] |
| 56 | PIPE-7 | One error vocabulary on each transport: `Status { kind, message }` / `Custom { status, message, source: Option<Arc<dyn Error>> }` / `AppError`, `GrpcCode` where HTTP has `u16`; today's constructors kept as sugar. **The three-field `Custom` is required** (`GRPC_AUDIT.md` §8 D4, README) | F136, gRPC D4 | — | [ ] |
| 57 | PIPE-8 | A guard's refusal is one event on all four transports and both WebSocket phases | F130 | 56 | [ ] |
| 58 | REN-1 | `PanicRecovered::message()` answers the generic literal; the payload stays on the public field; the three tests assert the generic text and read the payload off the event | F242, C12 | 57, **S5** | [ ] |
| 59 | GRPC-2 | A guard's refusal renders `Error::message()` on gRPC; the index stays on the event and in the `tracing::debug!` | F238 | 57 | [ ] |
| 60 | PIPE-9 | The API pass: `Transport` public and sealed, one type parameter on all three roles, `Next<T>`, context first, `catch`, one result alias, `HasHeaders` | F137, F127's doc half | 30 | [ ] |
| 61 | GRPC-3 | F240: the reply shape read off the `MethodShape` marker (message for unary, item for streaming); `IntoOutput<Grpc>` in `ulo-grpc`; the `Result`/`Response` name matching removed; `grpc_spelling_probe.rs` compiles. Read F75's design first so it stays a later addition | F240 | 60 | [ ] |
| 62 | PIPE-10 | A guard answers `Result<(), E>` | F131 | 57, 60 | [ ] |
| 63 | PIPE-11 | Error handlers get the `Factory` arm; a non-singleton middleware is refused by a diagnostic naming the scope. **0055 flips to `accepted`**; error-handler role tokens and collections take per-execution factories (Halt 4) | F126, F51 | 60 | [ ] |
| 64 | EXT-ADR | The two extraction ADRs, `proposed`: *an extraction failure is an error* (D1, D4; supersedes ADR-0023's *Roads not taken* and carries the pointer under 0023's title) and *one extraction language on HTTP* (D2, D3, D5); ADR-0023 gains its pointer to ADR-0040 (EXT-16); provisional 0059, 0060 | F178 | — | [ ] |
| 65 | EXT-1 | `FromContext::Error: ulo::Error`; `impl Error` on the eleven extractor errors; `BodyExtractionError<E>` gains `std::error::Error`, a `source()` and a forwarding `kind()` | F165 | 64 | [ ] |
| 66 | EXT-2 | The four lift sites carry the error: HTTP answers `ExecutionResult::Err`, RPC/WS/gRPC lift instead of `to_string()` | F84, F1, C11 | 65 | [ ] |
| 67 | EXT-3 | Each extractor error names its kind: `BadRequest` / `UnprocessableEntity` / `Internal` for `BodyAlreadyRead` and `RequestError` | F163 (422 half), F164 | 66 | [ ] |
| 68 | EXT-4 | One `Content-Type` policy for both body extractors, `+json` accepted, 415 through `Custom`. **0059 flips to `accepted`** | F163 (415 half), F176 | 67, 56 | [ ] |
| 69 | EXT-5 | `Validated<E>` carries `E::Error`; `details()` serialises `ValidationErrors` | F175 | 65 | [ ] |
| 70 | EXT-6 | Markers lower onto the extractor path: one binding function, signature order, `use_extractors` deleted | A16, A17, C3, F177 | 66 | [ ] |
| 71 | EXT-7 | `#[param]`'s `default` refused at expansion; `#[query]`'s parsed at expansion; a missing value distinguished from an unparseable one; `#[query("name")]` refused on a non-scalar | A2, A3, C4, F169 | 70 | [ ] |
| 72 | EXT-8 | The marker read searches the parameter's attributes; the per-key branch stops reading primitive names | F170, F179 | 70 | [ ] |
| 73 | EXT-9 | `ExtractorKind` collapses to `Context`, `Request`, `Optional`, `Extracted`; the `HttpRequest` arm deleted; the `Optional` arm goes to the blanket | F166, F168, C23, C24 | 70 | [ ] |
| 74 | EXT-10 | A parameter whose pattern the macro cannot name is refused with gRPC's diagnostic in the other three macros | F167 | — | [ ] |
| 75 | EXT-11 | The context reference is diagnosed at the parameter, naming what was written | F166's context half | 73 | [ ] |

### Wave 4 — the co-design cluster: the HTTP seam, the WebSocket loop, the capability type

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 76 | HTTP-1 | Rename `docs/http-audit-adrs` and rebase it onto master (it carries 0052–0054 as `proposed` into master); `AdapterCapabilities` absorbing `streams_responses`, `register_ws_route`'s `Err` and rocket's listener refusal; **the capability ADR** (S10), provisional 0061 | F103, F104 | **S6**, **S10** | [ ] |
| 77 | WS-20 | `negotiates_extensions: false` in `AdapterCapabilities`, with the upstream position stated | F209 | 76 | [ ] |
| 78 | RPC-9 | `RpcCapabilities` on both RPC SPIs, read at `bind()`: payload support, `max_frame`, delivery modes, pattern-name rules, streaming; enforcement in core, declaration in the adapter; extends the capability ADR | D1(d), part F220, F231 | 76 | [ ] |
| 79 | HTTP-2 | The core route table and matcher (`matchit`), built independent of any adapter; duplicate detection over it; the `routes()` read (S12) | F4, F120 | 76, **S12** | [ ] |
| 80 | HTTP-Q | **One query parser for the framework** (S1): `serde_html_form` replaces `serde_urlencoded` at its six call sites, so `Query<T>`, the `#[query]` marker, `Path`'s round trip, `RequestPart` and the handshake map all read one crate that carries repeated keys and decodes once; the `n=` → `None` change documented | F117 | 79, **S1** | [ ] |
| 81 | HTTP-3 | `ServeContext::dispatch` and `Dispatched::{Respond, Upgrade(WsUpgrade)}` carrying 404, 405, `Allow`, `HEAD`, `OPTIONS`, `//` collapsing and the D2 decoding rule; `Unrouted` reaches the global handlers; the global chain's own failure reaches them (F125's constraint); ADR-0052 amended per S6 | F105, F107, part F106, F125, F81 | 79, **S6**, **S8** | [ ] |
| 82 | WS-21 | Core owns the WebSocket connection loop: `WsConnectionCallbacks` becomes a transport trait, `Dispatched::Upgrade` carries `WsUpgrade`, `MessageCallbackResult` collapses into `Items` plus a task handle, reply streams merge into core's loop, five `run_ws_connection` and five `TokioSender` go, core spawns nothing; `spike-d8-transport-bounds.patch` re-run as the first commit. **The D8 ADR** written and accepted here, provisional 0062 | F215, F212, F204 | 81, **S8** | [ ] |
| 83 | HTTP-5 | Migrate poem: delete the matcher, the fallback, the envelope copy, the per-route registration; upgrade at the dispatch point. No adapter authors an error body (F243) | part F106, F111 | 82 | [ ] |
| 84 | HTTP-6 | Migrate salvo, as 83 | part F106, F111 | 82 | [ ] |
| 85 | HTTP-8 | Migrate rocket, as 83; `ws_marker_response` dies with `Dispatched::Upgrade` | part F106, F111, part F243 | 82 | [ ] |
| 86 | HTTP-4 | Migrate axum, as 83; `native_error_response`'s dead arms go | part F106, F111, part F243 | 82 | [ ] |
| 87 | HTTP-7 | Migrate actix, as 83; the request conversion becomes infallible in the type, the header loop counts and logs what it drops | part F106, F111, F243, F244 | 82 | [ ] |
| 88 | HTTP-9 | Remove `register_route`, `register_ws_route` and `RequestHandler` from the SPI. **0052 flips to `accepted`**, with its pointers into ADR-0009 and ADR-0007 | — | 83–87 | [ ] |
| 89 | WS-13 | `Gateway` declares its subprotocols; `WsUpgrade` carries the selected one; `accept_hdr_async` on the separate-port path | F198, F199, F190 (adapter half) | 82 | [ ] |
| 90 | WS-8 | A normalised read error on the WS transport trait, mapped to a §7.4.1 close code; the suite gains the F197 assertions | F197 | 82, 2 | [ ] |
| 91 | WS-12 | A frame ceiling on the registration surface, default 16 MiB, `1009` on breach | F203 | 90 | [ ] |
| 92 | WS-15 | Origin verification as `Dispatched::Respond(403)` | F207 | 88 | [ ] |
| 93 | WS-16 | The peer address into the request extensions and out to `WsHandshake` | F205 | 88 | [ ] |
| 94 | WS-10 | `close_with` returns a `Result`, refusing `1005`, `1006`, `1015` and anything under `1000` | F200 | — | [ ] |
| 95 | WS-11 | `ErrorKind` maps to `3000` / `3003` / `3008` where the registry has them | rest of F202 | 3, 94 | [ ] |
| 96 | WS-14 | `event_field` becomes `route(&self, msg) -> Option<String>` | F211 | — | [ ] |
| 97 | WS-17 | `trait Broadcast`, async throughout, `send -> Result<(), _>`; both services behind it | F213 | — | [ ] |
| 98 | WS-18 | `BroadcastFrame` as `ulo-ws-redis`'s own wire type, tagged and versioned | rest of F210 | 5, 97 | [ ] |
| 99 | WS-19 | The handshake query through the crate S1 picks (through `serde_urlencoded` if S1 is declined) | F206 | 80 or 81 | [ ] |
| 100 | EXT-12 | The WebSocket connection hooks read their parameters through `FromContext<WsContext>`; `DisconnectReason` becomes an extractor. **0060 flips to `accepted`** | A6, F171 | 82 | [ ] |
| 101 | PIPE-12 | The pre-dispatch role: `Middleware` narrowed to one anchor, reaching all four transports, typed-error contract, path scoping over the framework table, tower mounted there, the module anchor retired; gRPC anchored above tonic's router (S13). **0056 flips to `accepted`**, superseding the middleware half of ADR-0007 | F73, F118, F119, F123, F124, F126's middleware half, F127 | 88, **S13** | [ ] |
| 102 | HTTP-10 | ADR-0054: `StatusCode` and `HeaderMap`; **0054 flips to `accepted`** | F108, F113, F114 | 88 | [ ] |
| 103 | HTTP-11 | ADR-0053: problem details, one renderer, `render_error` reachable on HTTP, RPC and WebSocket (S4); 0053 amended per S6 and **flipped to `accepted`** | F65, F69 (422 half), F87, F22 | 102, **S4**, **S6** | [ ] |
| 104 | HTTP-12 | `Vary: Origin` unconditionally; a cookie surface | F109, F115 | 102 | [ ] |
| 105 | HTTP-13 | One body ceiling at 2 MiB with a knob, declared per adapter; multipart constraints from it | F112, F116, F27 | 76 | [ ] |
| 106 | HTTP-14 | `use_global_http_prefix`, applied once at table construction | — | 79 | [ ] |
| 107 | EXT-14 | `Path` reports the error it was given; `PathError::NotFound` deleted or constructed | F174 | 80 or 81 | [ ] |

### Wave 5 — RPC Phase 2 and Phase 3

Docker is up for every row here; one broker suite at a time.

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 108 | RPC-10 | `RpcData` narrows to `Json \| Binary`; `RpcData::text` becomes a constructor; redis stops tagging its envelope's `data`; `bytes_to_data`/`data_to_bytes` collapse into `ulo::rpc::wire`. **ADR** *what `RpcData` means on the wire*, pointer under ADR-0032, provisional 0063 | F216 | 78, 88 | [ ] |
| 109 | RPC-11 | The client collapse: `RpcClientTransport` becomes the carrier (`dispatch` + `inbound` + `capabilities`); core owns correlation, the pending map, the timeout, the gap deadline, `forward_stream`, the cancel notice on a `send` timeout (D3) | F221, F222, F225, F228, F229 (the queue stays) | 108 | [ ] |
| 110 | RPC-12 | The server collapse: core takes the `catch_unwind`-and-frame sequence, `drain_tasks`, the concurrency bound and the `{pid}-{nanos}` helper; the four `handle_*` functions are **not** merged wholesale; `basic_qos` on rabbitmq | F221, F224, F228 | 108 | [ ] |
| 111 | RPC-13 | `RpcMessageCallbacks` becomes a trait with one `async fn` taking `Items<RpcData, RpcError>`; every adapter passes `Items::One`; the inbound grammar reserved beside the reply grammar | D7, F234 | 109, 110 | [ ] |
| 112 | RPC-14 | A declared delivery mode: adapter default, per-pattern override, `balanced` for `#[message_pattern]`, `broadcast` for `#[event_pattern]`; nats queue group, mqtt `$share/`, rabbitmq/kafka per-instance broadcast; redis refuses `balanced` at `bind()`. **ADR** *delivery cardinality*, provisional 0064 | F218 | 111 | [ ] |
| 113 | RPC-15 | tcp's `max_frame` enforced against the declared capability, default 1 MiB, `TcpAdapter::with_max_frame` | F220 | 111 | [ ] |
| 114 | RPC-16 | Pattern-name validation against the declared rules; the reserved-name check for the four cancel channels stays in core; refused under ADR-0024 | F231 | 111 | [ ] |
| 115 | RPC-17 | One word covering three sites (S3); udp's single-reply branch writes the error frame its stream branch writes | F223, F63 | 110, **S3** | [ ] |
| 116 | RPC-18 | Per-call credit: `{"id":…,"credit":N}` in `ulo::rpc::wire`; `drive_reply_stream` waits for credit; `forward_stream` issues it; in band on tcp and udp | F229, D8 | 109, 110 | [ ] |
| 117 | RPC-19 | Credit on nats, redis and mqtt over a per-call control address; kafka and rabbitmq declare through `RpcCapabilities` that they cannot and fall back to a bounded queue with shed-and-cancel, the cancel sent before the error | F229, D8 | 116 | [ ] |
| 118 | GATE | RPC Phase 3 approval (S9) | — | 117, **S9** | [ ] |
| 119 | RPC-20 | The inbound grammar: request item frames and a request-end marker; per-transport correlation of a multi-frame request; udp declares it cannot order inbound frames. **ADR** *request-side streaming*, provisional 0065 | — | 118 | [ ] |
| 120 | RPC-21 | The handler-side extractor holding the request stream, designed after reading gRPC's `Inbound<T>` (ADR-0043); `#[patterns]` accepts it | — | 119 | [ ] |
| 121 | RPC-22 | Client-streaming: many request frames, one reply; `RpcClient` gains the verb | — | 120 | [ ] |
| 122 | RPC-23 | Bidi: a request stream and a reply stream on one correlation | — | 121 | [ ] |

### Wave 6 — GraphQL Phase 2 and Phase 3

Fetch the three protocol documents rather than recalling them (`GRAPHQL_BUILD_HANDOFF.md`).

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 123 | GQL-3 | **ADR A** *a GraphQL engine is an SPI; its bindings are ulo's* (D1, D5–D8), provisional 0066; the `ulo-graphql` crate: `GraphqlEngine`, `EngineCapabilities` (the capability ADR's shape), the neutral request/response types, `RequestErrorKind`, `ModuleRef` as the resolution handle; its `coverage_ledger.rs` row | — | 76, 102, 103 | [ ] |
| 124 | GQL-4 | The two engine implementations; `GraphQLModule<E>` with `with_schema` (Path A) and the engine on the module; the fixed token deleted | F191, F192, C20 | 123 | [ ] |
| 125 | GQL-5 | The HTTP binding: `Accept` negotiation, `application/graphql-response+json`, the §6.4 status table, `Content-Type`, `406`, `415`, `294`; §1's table as a conformance suite against both engines | F182, F183, F184, F186, F194 | 124 | [ ] |
| 126 | GQL-6 | The graphql-transport-ws binding: connection state, the five close codes, `error` frames, routing by operation type; §3's table as a suite | F187, F188, F189, F190 (gateway half) | 124, 4, 89 | [ ] |
| 127 | GQL-7 | The graphql-sse binding, both modes — the falsifier for §7: a validation error answers `200` plus `next` here and `422` on row 125, from one engine result. **ADR A flips to `accepted`** | — | 125 | [ ] |
| 128 | GQL-8 | **ADR B** *a resolver is a handler* (D3 Path B, D4), provisional 0067; the `builds_schema` capability; the dynamic-schema builder registering one closure per field into ulo's pipeline; the six probes promoted with their controls | F195 | 123, 124 | [ ] |
| 129 | GQL-9 | The declaration macros (`#[graphql_object]`, `#[query]`, `#[mutation]`, `#[subscription]`, `#[arg]`) and the type mapping; one PR or three | — | 128 | [ ] |
| 130 | GQL-10 | `ContextBuilder` deleted; both `with_auth` examples rewritten onto guards and DI. **ADR B flips to `accepted`** | — | 129 | [ ] |

### Wave 7 — follow-ons and close-out

| # | Row | What | Closes | Needs | State |
| --- | --- | --- | --- | --- | --- |
| 131 | D3-ii-HTTP | A running HTTP handler learns its caller is gone: the drop guard row 18 validated, at the route wrapper in `http/pipeline.rs`, disarmed at a clean return; detached work holding a token clone stops | gRPC D3 (ii), HTTP | 18, 88, **S2** | [ ] |
| 132 | D3-ii-RPC | The in-band cancel frame and a dropped connection fire a unary call's token, not only abort its task: the registered cancel action cancels the token, or the guard sits in the dispatch sequence RPC-12 lifts into core; the two docs that claim this already happens (`rpc/wire.rs` `Inflight`, `tcp_adapter.rs` header) become true | F245, gRPC D3 (ii), RPC | 18, 110, **S2** | [ ] |
| 133 | D3-ii-WS | A message handler still running when the socket closes learns it: the guard at the message dispatch in core's connection loop | gRPC D3 (ii), WS | 18, 82, **S2** | [ ] |
| 134 | CLOSE | Walk every handoff's verification list and every memo's closing checklist against the tree and record the result box by box (`feedback-walk-the-checklist-you-wrote`); delete every closed ledger entry clause by clause; every `CLAUDE.md` row; §18 complete; this file's state column complete | — | all above | [ ] |

The performance audit (`PERFORMANCE_AUDIT_HANDOFF.md`) runs after row 134 as its own pass and is not
a row: its first deliverable is a decision from the user on which benchmark shape to build.

---

## 4. The loop every row runs

1. **Sync.** `git fetch`; `git status` clean; `pgrep -fl "cargo test"` empty. The branch point is
   the stack tip — the branch of the highest `[p]` row — or master for row 1 and after a ceremony.
2. **Read the row.** Its handoff line; the memo section(s) that line cites; every ledger entry in
   `Closes`, clause by clause; the verification the handoff owes it. Re-check the plan's claims
   about the current tree (0.8). If anything read contradicts anything else read, that is 0.2.
3. **Branch.** `git switch -c <type>/<what-the-change-does> <stack-tip>` — named for what it does,
   not for the row.
4. **Red first.** Write the test the handoff names. Run it. It fails, for the stated reason. Where
   the handoff names a control, the control passes in the same run.
5. **Build the change.** Then the production text it falsifies: doc comments, ADRs, examples,
   READMEs, and the workspace `CLAUDE.md` in the untracked parent that a repo-wide grep never
   reaches. A doc comment describing the old shape is part of this PR, not follow-up work.
6. **Gate**, each command's exit code read on its own, never piped into `tail` or `rg`:

   ```
   cargo fmt --all
   cargo check --workspace                                              # plain, no flags
   cargo check -p <crate> --no-default-features                         # each touched crate with features
   cargo check --workspace --all-features --all-targets --locked        # CI's check job
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --lib --locked
   cargo test -p ulo --lib --locked
   cargo test -p ulo-macros --lib --locked
   cargo test -p ulo-macros --test diagnostics --locked                 # rustc 1.98.1
   cargo test -p integration-tests --locked                             # alone
   cargo test --workspace --all-features --doc --locked
   cargo test -p <crate> --locked                                       # each touched crate
   cargo test -p ulo-rpc-<broker> --features integration --test conformance --locked   # RPC rows only, one at a time
   ```

   Read every count. `0 passed` where tests exist means the gate is broken, not the code.
   `--all-targets` without `--all-features` is known not to build on master (F40, F89); the plain
   form that must pass is `cargo check --workspace`.
7. **Commit.** Split where natural (library / tests / docs). Subject `type(scope): imperative`,
   under 72 characters, `!` for a public-surface break; body only where the subject does not carry
   the why; no first person; every number from a command run after the last edit.
8. **The Law on the commit messages** (§5), in the background. Verify each finding. Amend.
9. **Push. Draft the PR title and body. The Law on both.** Then `gh pr create --base <stack-tip
   branch>`. The body cites no untracked ledger and no `F<n>`; a bare `#<digits>` is a link, so a
   count is spelled out.
10. **CI.** `gh pr checks <n> --watch`. Red: fix, re-gate, the Law again on any changed message,
    push.
11. **Do not merge.** Tick the row `[p] #n`; write the row's bookkeeping packet into §8.
12. **Review traffic.** A comment on any open PR in the stack is answered on that PR's branch; every
    branch above it is rebased, re-gated and pushed with lease before its PR is touched. A change
    that alters what a row does is 0.2, not a review fix.
13. **Next row**, branched from the new tip.

---

## 5. The Law — the brief

Run as a background agent before every push and every PR open. Surface only its findings.

> Read all four of `~/.claude/commits.md`, `~/.claude/pull-requests.md`,
> `~/.claude/prose-discipline.md` and `~/.claude/posture.md` in full before judging anything. Then
> review the draft below against them. Report violations as a list, each naming the rule and quoting
> the offending clause, and give a corrected version. A word-level scan does not discharge this: the
> failures that matter are relations, postures and audiences — a clause locating the PR in a plan or
> against another PR, a sentence narrating the authoring session or the verification, a deferral
> line arguing its own case, a general fact nothing can later correct. Judge those by reading. If the
> text is clean, say which tests were applied and that it passed them; do not report a clean scan as
> a clean text.

Add per invocation: the branch and the commit; an instruction to read the diff in full; a numbered
list of the technical claims to verify hardest (cross-transport claims first — "every transport
now does X" is a claim about seven crates); and how many prior passes in this build found factual
errors. Ask for both facts and register; do not say the register scan is reliably clean.

Rulings already settled (`feedback-the-law-review-protocol`): a PR body's opening paragraph may
repeat a clause from the commit body; an opening paragraph may carry what the PR adds and the
problem together; a reference to an ADR the change revisits survives the cross-PR test; a PR body
never cites `FRAMEWORK_GAPS.md` or any untracked ledger.

---

## 6. Halt log

One entry per halt, newest last. The build ends its turn after writing one. The user answers under
it; the answer is what the next session reads first.

### Halt 1 — row 12 (RPC-6, F230), 2026-09-26

**The row.** "Kafka's `Timeout::After(5s)` literal becomes the client timeout and the server's
remaining budget" (`BUILD_SEQUENCE.md` row 12; `RPC_BUILD_HANDOFF.md` Phase 1 PR 6: "becomes
`self.timeout` on the client and the remaining budget on the server's reply publish").

**What the plan says.** F230's candidate fix: "`Timeout::After(self.timeout)` on the client, and
on the server the remaining budget rather than a constant — [[feedback-a-timeout-is-a-promise]]
... a post-deadline wait is bounded by the configured timeout rather than by a constant added
after it." The feedback note bounds a wait by "the caller's own number", the configured timeout
the wait follows.

**What the tree has.** The client half is definite: four `producer.send(record,
Timeout::After(Duration::from_secs(5)))` sites in `kafka_client_transport.rs` (`send`,
`open_stream`, `emit`, and the cancel notice in `forward_stream`), each with `self.timeout` or
the per-frame gap deadline in scope. The server half has no budget to be the remainder of:
`KafkaAdapter` carries no timeout of any kind (`grep -n "Duration\|timeout\|drain"
kafka_adapter.rs` finds two 500 ms retry sleeps and the two literals), the caller's timeout does
not travel on the wire (D3(b), filed with a trigger), and the drain budget the phrase would name
arrives with RPC-12 (row 110), where the five brokers gain `drain_tasks` and `with_drain_timeout`
(`RPC_AUDIT.md` D5, §5). So "the server's remaining budget" names a number that does not exist
before row 110.

**Options.**
1. Build the client half now and move the server half into row 110, where the drain budget it is
   the remainder of exists. Row 12 closes F230's caller-visible clause (a call returning up to
   5 s after its timeout); F230 stays open on its server clause until row 110.
2. Build both halves now with a server knob (`KafkaAdapter::with_publish_timeout`, default 5 s).
   The feedback note argues against a second knob: the caller has no model of a publish after
   the handler returned.
3. Build both halves now with the server publish bounded by the *client's* timeout carried in a
   header — D3(b), which the memo filed with a trigger and the handoff lists under "Not in the
   sequence".

**Recommendation.** Option 1. It is a sequencing change (the server half moves to row 110), which
0.2 lists as divergence, so it is the user's to make.

**Answer (user, 2026-09-26).** Option 1. Row 12 builds the client half; the server half moves to row
110, which gives the brokers the drain budget it is the remainder of. F230 stays open on its server
clause until then.

**State.** Row 12 not started; no branch pushed, since no work exists on it. It was taken last
among the wave-1 rows that need nothing (0.3 allows the order), and rows 13–30 were built ahead
of it on the stack.

**Resumed 2026-09-26** on the answer: the client half built as #369, stacked on row 30's branch.

### Halt 2 — row 19 (GRPC-6, F237 and F241's second half), 2026-09-26

**The row.** "The metadata view: ASCII multimap plus a binary map, `headers_all(k)` and
`header_bin(k)` beside `header(k)`; `grpc-status-details-bin` written" (`BUILD_SEQUENCE.md` row
19; `GRPC_BUILD_HANDOFF.md` Phase 1 PR 6, and PR 4's row: "The real fix writes
`grpc-status-details-bin` and rides PR 6").

**What the plan says.** F241's candidate fix: "The real fix writes `grpc-status-details-bin`, which
needs a `google.rpc.Status` message and therefore a prost dependency and an encoding decision, and
should be taken with F237's metadata work since both are the same trailer surface." PR 6's
verification line covers the metadata view only ("a repeated ASCII key reaches `headers_all(k)`…
a `-bin` key reaches `header_bin(k)` with its bytes"); nothing states how the trailer is encoded
or verified.

**What the tree requires.** The metadata half is settled by the gRPC audit's D1 and buildable. The
trailer half needs the decision F241 names and no document makes:

- **The message type.** `google.rpc.Status` is not in `prost-types` (which the lock file carries);
  it comes from `tonic-types`, or a hand-written prost message in `ulo-grpc`.
- **What `details()` becomes.** `ulo::Error::details()` is a `serde_json::Value`. The spec's
  `details` is `repeated google.protobuf.Any`. A JSON value maps to one
  `google.protobuf.Struct` packed in an `Any`; mapping it onto the rich error model's typed
  details (`BadRequest`, `ErrorInfo`, …) needs a shape rule the framework does not have.
- **Who reads it.** Every gRPC client library decodes `grpc-status-details-bin` as a
  `google.rpc.Status`; a `Struct` detail is readable everywhere, typed details only by clients
  that know the types.

**Options.**
1. Split the row: build the metadata view (F237) now; move the trailer (F241's second half) to its
   own row once the encoding is decided. F241 stays open on that clause.
2. Build both now with `google.rpc.Status { code, message, details: [Any(Struct)] }` from
   `tonic-types`, the `Struct` converted from `details()`'s JSON. Adds `tonic-types` to
   `ulo-grpc`.
3. Build both now with the rich error model: map known `details()` shapes (a validation field
   list) onto `BadRequest` and fall back to a `Struct`. Needs a shape rule, and couples to the
   extraction audit's D1 (`ValidationErrors` into `details()`), which is not built yet.

**Recommendation.** Option 2 for the encoding, taken in the row as written if the user accepts
`tonic-types` as a dependency; otherwise option 1. The `Struct`-in-`Any` form carries whatever
`details()` holds without a shape rule, and every client can decode it.

**Answer (user, 2026-09-26).** Option 2: the row as written, the trailer a `google.rpc.Status` from
`tonic-types` carrying `details()` as one `google.protobuf.Struct` in an `Any`. `tonic-types` joins
`ulo-grpc`'s dependencies; core stays tonic-free.

**State.** Row 19 not started; no branch pushed, since no work exists on it. Rows 20 onward were
built ahead of it on the stack; row 20 states the limit the trailer removes.

**Resumed 2026-09-26** on the answer: built as #370, stacked on row 12's branch.

---

### Halt 3 — row 32 (DI-1, the value surface), 2026-09-26

**The row.** `Provide::{value, factory, alias}`, `T::provide()`, `.under`/`.multi` as one blanket
over `ProviderFactory`, `IntoFactory` per arity, async-only (`DI_BUILD_HANDOFF.md` PR 1; `DI_AUDIT.md`
§9.2, D8). Row 33 then deletes `provide!` and the four macros.

**What the plan says.** "One thing a macro bought: `provider_factory!` reads the closure's annotated
parameter types to emit `dependency_tokens()`. A trait implemented per arity supplies the same tokens
from the *types* … With that, nothing else in the surface needs a macro" (§9.2).

**What the tree requires.** A second thing the macros buy is not in the memo: **enhancer role
detection.** `provider_value!` and `provider_factory!` register a guard, interceptor, error handler,
middleware or gateway role by probing the produced value's concrete type with autoref probes at
expansion time (`shared::enhancer_emit::value_probe_detection`, and the type probe gated on the
closure's `-> T` for execution scope). `provider_macro_role_detection.rs` pins it: a guard provided by
`provider_value!("VALUE_GUARD", ValueGuard, ValueGuard)` or by an execution-scoped
`provider_factory!` is applied by `#[use_guards("TOKEN")]` and gates a route. `enhancers.rs:372`
uses the same path (`provider_factory!("AUTH_GUARD", |tracker| AuthGuard::new(tracker))`). A generic
function such as `Provide::value<V>(token, v)` sees `V` only through its bounds, so an autoref probe
inside it always takes the fallback, and nothing registers a role. Stable Rust has no specialization
to recover it. The row as written would delete a pinned behaviour.

**Options.**
1. **Enhancers are types.** A value or factory declaration registers no role. A guard reaches a
   token through its own declaration, `AuthGuard::provide().under("AUTH_GUARD")`, whose
   `#[injectable]` knows the type and forwards its roles as `provider_token!` does today. A guard
   built with arguments at the declaration site uses `#[use_guards(AuthGuard::new(..))]` or the
   closure spelling from ADR-0055. The role-detection test is rewritten to pin both routes, and
   `enhancers.rs:372` moves to one of them. No new API. This matches §9.2's example, where every
   enhancer declares itself.
2. **Roles named at the declaration.** `Provide::value("X", g).role::<dyn Guard<HttpContext>>()`,
   or one modifier per role, bounded so the compiler checks the impl. It keeps value and factory
   enhancers, and adds surface the audit did not decide.
3. **Keep one macro for enhancer values.** It contradicts D8's "five proc macros go".

**Recommendation.** Option 1. It adds nothing, the declaration-site spellings already cover a guard
built with arguments, and it keeps D8 whole.

**State.** Row 31 is open as #372. Row 32 is not started. Rows 33, 34, 38, 40, 41, 45, 49–52 and 54
wait on it; rows 35–37, 39, 42–44, 46–48 and 53 do not.

**Answer (user, 2026-09-26).** Enhancers are types, and a token can carry a role. Probed against
the real SPI before the answer (scratch crate on `3cc3651`, axum adapter, four requests and two
refusals):

- An `#[injectable]` enhancer registers its own roles, as today; `T::provide().under(token)`
  forwards them to the token.
- A token typed with a role trait, `Token<dyn Guard<HttpContext>>`, makes a value or factory
  provided under it take that role. The constructor's bound requires it: a non-guard under a guard
  token fails to compile (`E0277`), and a value of the wrong type under `Token<String>` fails
  (`E0308`). `APP_GUARD` and `APP_INTERCEPTOR` become role tokens.
- A plain string token, and a token typed with a data type, carry data and register no role. A
  guard provided under a string token is refused at startup by the existing not-found diagnostic.
- The use site takes a type, a value, a closure or a token (ADR-0055's three spellings, and a token
  once row 33 makes tokens values). `#[use_guards]` taking a string token resolves only a role that
  was registered under that name.
- The value and factory macros' autoref role detection is deleted with them. Nothing replaces it:
  the role comes from the enhancer's own type or from the token's.

Rejected on the way: roles named at the declaration (`.guard::<Http>()`, a modifier per role the
audit did not decide); one macro kept for enhancer values (contradicts D8); a `#[module]` DSL
parsing its own provider syntax (a macro sees text, not types, so it could not detect a role
either, and a DSL exists in `#[module]` alone while integration crates build declarations in
functions and through the builder).

**Carried by:** row 32 (role tokens on the value and factory surface; `APP_GUARD` and
`APP_INTERCEPTOR` retyped); row 33 (`#[use_guards(tokens::AUTH)]` compiles); row 38 (two
registrations under either `APP_*` token both run); row 55 (global role tokens for RPC, WebSocket
and gRPC); row 60 (`Guard<HttpContext>` respelled `Guard<Http>` in the tokens' types). ADR-0058
records it.

---

### Halt 4 — row 32 (DI-1), the `.multi` modifier, 2026-09-26

**The row.** `.under`/`.multi` as one blanket over `ProviderFactory` (`DI_AUDIT.md` §9.1, §9.2:
`A::provide().multi::<dyn Plugin>("PLUGINS")`).

**What the tree requires.** A contribution to a collection is stored as `Arc<dyn Plugin>`, so `.multi`
has to turn an `Arc<V>` into an `Arc<dyn Tr>` for a caller-chosen `Tr`. That generic coercion needs
`Unsize`, which is unstable; probed, `fn multi<Tr: ?Sized>(self, ..) -> Arc<Tr> { Arc::new(v) }` fails
with `E0308` and `E0277` (`Tr` unsized). A bare `ProviderFactory` also hides its output type, so the
modifier can sit only on declarations that know it: `Provide::value`, `Provide::factory` and
`T::provide()`. `.under` has neither problem and is built as recorded.

**Built on the branch, uncommitted:** `Provide::{value, factory, alias}`, `T::provide()`, `.under`
(forwarding roles, scope and lifecycle hooks), `IntoFactory` for arities 0–8, async-only, role
tokens per Halt 3 for guards, interceptors and error handlers on all four transports and for
middleware, and `APP_GUARD`/`APP_INTERCEPTOR` retyped. A factory takes `.per_execution()` or
`.transient()`, returning a declaration whose role bound is `HoldsPerExecution`, which only the guard
and interceptor role tokens implement, so an error handler or middleware built per execution fails to
compile rather than registering nothing. Five runtime tests and four refusal snapshots pass.

**Options for `.multi`.** Both probed; both refuse a non-plugin at compile time (`E0277`).
1. **One line per collected trait, the call as recorded.** `ulo::collection!(dyn Plugin);` once,
   written beside the trait; then `A::provide().multi::<dyn Plugin>("PLUGINS")` exactly as §9.2 has
   it. The line generates the coercion impl for that trait. A declarative macro, so it also serves a
   trait from another crate.
2. **A coercion closure at each call.** `A::provide().multi::<dyn Plugin>("PLUGINS", |p| p)`. No
   declaration per trait; the `|p| p` repeats at every contribution, and it reads as noise.

**Recommendation.** Option 1. The call keeps the recorded shape, the extra cost is one line per
trait rather than one per contribution, and all 13 multi call sites in the tree are in
`multi_providers.rs` over one trait.

**State.** Row 32 is halted with the work above uncommitted on
`feat/declare-a-provider-as-a-value`; the rows waiting on it are Halt 3's list.

**Answer (user, 2026-09-28).** Neither option. One call-site `provide!`, a proc macro, over the
value API, with multi keyed by element type in the container. Reached through an independent review
by Fable (`fable2/REPORT.md` in the session scratchpad) and probes of every form (`macroprobe/`):

- **One macro, as sugar over the value API, never beside it.** Every form expands to a public
  constructor, so `builder.provider(provide!(..))` and a `for_root` function build the same value
  (D12 holds). The letter of D8 is reversed and its reasons kept: the grammar is keyword-led, not
  casing-led; one vocabulary; a role comes from a written type, never from a probe (Halt 3). The
  proc macro parses first and then decides, and points each refusal at the user's closure or value.
- **Multi at the core.** A binding is single or many under its key; `into` declares many (D2's
  "declared multi"). `Vec<Arc<dyn Plugin>>` is looked up by element type the way `Arc<Db>` is.
  The macro writes the `Arc<V>` to `Arc<dyn Trait>` conversion at the concrete call site, so no
  per-trait line exists. A named collection is a `Many<T>` const. A single binding and an `into` on
  one key fail startup naming both (ADR-0057). Contributions cross module exports as singles do.
- **Globals.** The unnamed collection of a role type is that transport's global set:
  `into dyn Guard<HttpContext>` is the HTTP guards. `APP_GUARD`, `APP_INTERCEPTOR` and a new
  `APP_ERROR_HANDLER` name the every-transport sets; a contribution under one must implement the
  role on all four transports, and one that does not fails to compile (probed:
  `HttpOnly: Guard<RpcContext> is not satisfied`). One spelling for "every transport": the constants;
  the `every` keyword probed alongside was dropped. Per-route guards keep role tokens
  (`Token<dyn Guard<HttpContext>>`, Halt 3).
- **The grammar.** Key: a string, a `Token`/`Many` const, `dyn Trait`, or none (keyed by what the
  source builds). Source: a bare name is a type's own declaration (the `providers: [Db]` rule); an
  inline closure is a factory and any other inline expression a value, both recognised by the proc
  macro; `factory f` and `value C` for a factory or value held in a variable or const; `alias K` for
  a second name for an existing binding. `dyn Logger => ConsoleLogger` binds a trait to its
  implementation.

  ```rust
  provide!("app.port" => 3000u16)
  provide!(tokens::API_KEY => "k".to_string())
  provide!(async |cfg: Config| Db::connect(&cfg.url).await)   // keyed by Db
  provide!(Db)
  provide!(dyn Logger => ConsoleLogger)
  provide!("app.log" => alias tokens::LOGGER)
  provide!(tokens::AUTH => HeaderGuard("x-auth"))            // per-route guard token
  provide!(into APP_GUARD => Auth)                            // every transport
  provide!(into dyn Guard<HttpContext> => RateLimit(100))     // HTTP only
  provide!(into dyn Plugin => A {})
  provide!(into dyn Plugin => async |d: Dep| B(d))
  provide!(into dyn Plugin => C)
  provide!(into dyn Plugin => factory make)
  provide!(into tokens::LEGACY => A {})                       // Many<dyn Plugin>
  ```
- **Token names** follow the prefix convention (`"billing.plugins"`); collisions fail startup.

Rejected on the way: the audit's `.multi::<dyn Tr>(name)` (needs `Unsize`); a coercion closure per
call; a `ulo::tokens!` block with a marker per collection; typed consts plus `collects!` once per
trait (Fable's first recommendation; the per-trait line kept multi bolted on); a no-declaration
value API (`Arc::new` at every value, a cast in every factory, and a type's own declaration cannot
contribute).

**Carried by:** row 32 (the value API with `Contribution::{Single, Into}` and `Many<T>`, injection
by element type, and the `provide!` proc macro); row 33 (every call site onto `provide!`, the old
macros deleted, `#[inject(CONST)]` checked against the token's type); row 38 (the container's
single/many binding and the mixing refusal); row 55 (the framework reads the role collections;
`APP_*` become the every-transport sets; `use_global_*` contributes to the same collections or
goes); row 63 (error-handler role tokens and collections accept per-execution factories).

---

### Decision — a handle type injects by value, amending D14, 2026-09-28

**The user's call.** D14 has singleton and execution-scoped injectables shared as `Arc<T>` at the
injection site, with no `#[derive(Clone)]` and no quiet copies. Many types already share on clone:
`PgPool`, `reqwest::Client`, a Redis `ConnectionManager`, each an `Arc` inside, so `Arc<PgPool>` is a
double wrap. Amendment: a marker trait, working name `Shared`, meaning "cloning this shares it". A
field may be written as plain `T` only when `T: Shared`; any other singleton or execution-scoped
field is `Arc<T>`. Core implements `Shared` for `Arc<T>`, the integration crates for the handles
they register (the `ulo-db-*` pools and connections), and an application for its own handle types.
A plain struct still injects as `Arc<T>`, so the copies D14 removes do not come back. Transient
stays by value. Carried by row 50.

---

### Halt 5 — row 33 (DI-2), a `Token<T>` const in `#[use_guards]`, 2026-09-28

**The row.** "Migrate every call site; delete `provide!` and the four macros; `__ulo_provider_factory`
leaves the public surface. **Absorbs PIPE-6**" — PIPE-6 being "A `Token<T>` const is nameable in
`#[use_guards]`; the `APP_GUARD` example goes" (F50). `PIPELINE_AUDIT.md`: "recognise a const resolving
to a `Token<T>`". D4: "Accept any path in token position, emit the `IntoToken` call".

**What the tree requires.** In `#[inject(..)]` every argument is a token, so D4 applies as written: a
path is a value, `#[inject(tokens::X)]` works, and `#[inject(Type)]` goes (bare `#[inject]` already
keys by the field's type; `token_format.rs` pins the old form). `#[use_guards(..)]` is different:
ADR-0055 fixes its grammar, and a bare path there is a guard *type*, resolved by DI. `tokens::AUTH`
and `guards::AuthGuard` are both bare paths. A const lives in the value namespace and a non-unit type in
the type namespace, and no expansion compiles for both, so "recognise a const" has no mechanism short
of the casing rule D4 deleted. Halt 3's answer ("`#[use_guards(tokens::AUTH)]` compiles once row 33
makes tokens values") rests on the same assumption and is wrong as stated.

**Options.**
1. **A keyword, as `provide!` has.** `#[use_guards(token tokens::AUTH)]`: the keyword says the path is a
   key, the expansion is the `IntoToken` call, and a bare path stays a type. `#[use_guards("app.auth")]`
   keeps working. One word, consistent with `provide!`'s `value` / `factory` / `alias`.
2. **Strings only in `#[use_guards]`.** A const is written `tokens::AUTH.name()` (a method call, not a
   bare path, so it reads as a value), or not at all. F50 stays half-open.
3. **Role tokens as unit types.** `pub struct AUTH;` implementing a token trait works in both positions
   (a unit struct is a type and a value), dispatched by autoref at the use site. It needs a declaration
   form per token, which the user declined for collections.

**Recommendation.** Option 1.

**State.** Row 32 open as #373. Row 33 not started.

**Answer (user, 2026-09-28).** Option 1, with the refinements of an independent review by Fable
(approved the same day):

- **The position rule, stated once in ADR-0058.** In a *token position* (`provide!`'s left side,
  `#[inject(..)]`) a bare path is a token: an `IntoToken` value. In a *source position* (`provide!`'s
  right side, `#[use_guards]` / `#[use_interceptors]` / `#[use_error_handlers]`) a bare path is a
  type's own declaration, resolved by its type token, and a value or a token there takes its keyword.
- **`token` names a token in a source position**, the same word in all three `#[use_*]` attributes:
  `#[use_guards(token tokens::AUTH)]`. A string literal keeps working bare.
- **`value` names a value held in a `static` or `const`** in the `#[use_*]` attributes too, as in
  `provide!`: `#[use_guards(value SHARED_LIMITER)]`. Without it a bare path there reads as a type.
- **`#[inject(Type)]` is dropped.** A type is already a key (`token_of::<T>()`), and bare `#[inject]`
  keys by the field's type; a key naming any other type fails with a type mismatch, an alias has the
  same key, and `Arc<dyn T>`/`Vec<Arc<dyn T>>` are keyed by shape. A type written there gets a
  diagnostic: "a type key is the field's own; write bare `#[inject]`".
- **Refused:** `#[inject(token X)]` ("already a token; write `X`"); `provide!(k => token k2)` (the
  word there is `alias`). Error handlers take `token` and `value` but no closure, stated beside the
  closure exclusion.
- The word stays `token`, the API's own (`Token<T>`, `IntoToken`, `token_of`); the ADRs keep "key" for
  what a binding goes under, a token or a collection's element type.

**Carried by:** row 33 (the `#[use_*]` and `#[inject]` grammar, the refusals and the diagnostic, with
the migration and the macro deletion). ADR-0058 records the position rule.

---

### Halt 6 — row 34 (DI-3) and the key model, 2026-09-28

**The trigger.** Row 34's review found `exports:` parsing types against D5's "all four keys parse
expressions", and `exports: [Map<String, u32>]` failing to parse (probed). While weighing a shared
source-position parser, the user questioned the premise: the const/type ambiguity comes from copying
Nest's three key kinds (string, symbol, class), and "all of this amounts to one thing: what can serve
as a key to the container". Asked for a first-principles teardown with research.

**Research (other containers, Rust constraints) and Fable's teardown.** Guice, Dagger, Spring, .NET
keyed services and the Rust crate `more-di` key by type plus a qualifier; typemap's
`trait Key { type Value; }` makes a marker type a typed named slot; on stable Rust a key can be a type
or a string, not a string-parameterised type, and `type_name` is not const. Proposal: every key is a
type — a type's own key for the default, a marker type with `Key { type Value: ?Sized }` for a named
slot (role slots via an unsized `Value`, checked by coercion), `dyn Trait` lowered to its own slot,
`into` the one collection marker, strings exiled (a runtime-chosen name becomes a registry under one
typed key). A bare path in a key position is then always a type: the position rule, the `token`
keyword and `Token`, `Many`, `OutputKey`, `IntoToken`, `DeclarationToken`, `Holds`,
`HoldsPerExecution`, `CollectionKey` go; `Key`, `DeclaresProvider`, `DeclaresController` remain;
`value`, `factory`, `alias`, `into` stay. Costs: a marker per named slot (or a `key!` line), no
runtime-chosen keys, cross-crate slots need an imported type; #374, row 34's branch and the alias
branch are rewritten; ADR-0058's token clauses and ADR-0028's string-key clauses superseded;
`for_root_named(name, ..)` becomes `for_root_keyed::<K>(..)`.

**State.** Row 34: commit 1 staged, not committed; the alias branch (worktree `toni-rs-alias`) built
and green, uncommitted. #374 open, unchanged. Awaiting the user's decision on the key model.

**Answer (user, 2026-09-28): approved, "fully and completely".** Clarified before approving: an
`#[injectable]`, `#[controller]` or `#[websocket_gateway]` type is a key with no further writing; a
plain type needs `#[derive(Key)]` only to be written in a key position, not to be provided or
injected by its own type; a runtime-chosen set of names is one binding holding the set; a named slot
shared across crates needs the marker type imported.

**Carried by:** a new ADR (every key is a type), written first and reviewed by the user before code
moves; ADR-0058's token clauses and the position rule rewritten in place (proposed); a pointer under
ADR-0028. Then rows 32, 33 and 34 rebuilt on the model — #373's value API (`Key` for `Token`, `Many`,
`DeclarationToken`, `Holds`, `CollectionKey`), #374's migration and `#[inject]`/`#[use_*]` grammar,
row 34's module lists and builder — with the approved `alias` change folded in and the integration
crates' `for_root_named` moved to marker keys. The alias decision above stands, as this model's
reading of `alias`. Rows 38, 50 and 55 keep their intent; their texts are amended when they are
built. The record takes ADR-0059, the next free number; the extraction records that rows 64 and 68
call "provisional 0059, 0060" take the next free numbers when written, as the numbering note says.

**ADR review (user, 2026-09-28).** Read ADR-0059 and the edits to 0058, 0057, 0055, 0034 and 0028
after three Law passes: "looks good". Decided on review: string keys stay out, with no escape hatch
("we have a workaround for strings; if it comes up again we can then do it") — adding one later is
non-breaking, removing one is not. Confirmed: rescoping stays (which keys allow it is settled with
the value API's rebuild), `Key` is required only where a slot's contents are read, so a foreign type
exports and binds keyless with no impl, and the value API takes a key two ways, keeping `Of<T>`
private.

**Committed on #372 (`ad3302d9`), 2026-09-28**, after a fourth Law pass on the commit and the PR
body. The pointers under ADR-0028 and ADR-0034 were taken out before committing, with none added
under ADR-0029: an accepted record pointing at a proposal claims a refinement not yet decided, and
accepted ADR-0007 carries none to proposed ADR-0056. The change that flips ADR-0059 to `accepted`
adds the pointers under ADR-0028 and ADR-0034; the one that flips ADR-0057 adds ADR-0029's.

---

### Decision — `alias` names a binding by the source rule, 2026-09-28

**The question (the user's, after #374 opened).** "Shouldn't the alias, or rather its usage, be the
driver — whether type or not, the point is it is aliasing another key?" In #374 `alias K` read its
argument as a token value: `alias "PORT"`, `alias tokens::LOGGER` and `alias token_of::<T>()` compiled,
and `alias ConfigService` did not — an exception to ADR-0058's position rule, `alias` sitting on the
source side of `=>`. Reasoned by Fable: an alias is a second name for an existing binding, never a
value; a type is a key; the macro cannot accept a type and a const as one bare path (the namespace
argument of Halt 5).

**Answer (user, 2026-09-28): approved.** `alias`'s argument is read as `exports:` and the `#[use_*]`
attributes read theirs: a bare path is a type's binding (`alias ConfigService`, the same instance), a
string is a token (`alias "PORT"`), a token const takes `token` (`alias token tokens::LOGGER`), and
any other expression is a token value (`alias token_of::<T>()`). A bare const after `alias` reads as a
type and is refused with rustc's `expected type, found constant`. `DeclarationToken`'s note stops
sending a type to the bare spelling where that re-keys. ADR-0058's position rule and grammar table
say so.

**Carried by:** its own PR, stacked on row 34's.

---

### Decision — merging by wave, as GitHub stacks, 2026-09-26

**The user's call.** PRs merge as each wave completes, after the user reviews them; the Law pass
stays per commit and per PR. Each wave is one GitHub stack. Wave 1 (rows 1–30, #341–#370) was
linked as stack #371 with `gh stack link`, which changed no branch. §8's procedure holds inside a
stack: merge bottom-up with the merge-commit method, apply each merged row's packet, then set it to
`[m]`. A contiguous group merged at once lands as one merge commit and keeps every PR's commits.
GitHub rebases and retargets the layers above a merged one.

**Standing authorization (user, 2026-09-26).** The build merges each wave itself once the wave is
built and green, with no manual review from the user: "once wave 2 is done we stack and merge like
that… i doubt any need for me to be doing manual review since i have employed the law in critical
steps". The per-commit and per-PR Law passes are the review. Wave 1 merged as stack #371 in one
merge commit, `3cc3651`.

---

### Halt 7 — row 44 (DI-12), a startup refusal needs a channel the SPI does not have, 2026-09-29

**The row.** "A factory declaring an execution-scoped dependency is refused at startup" (F154, A27):
a `StartupError` naming both, not the panic out of `create` it is today, under the standing
constraint that no DI defect leaves `create` by panic.

**What the tree requires.** `ProviderFactory::build` answers `Injectable` and `Provider::resolve`
answers `Box<dyn Any + Send>`: neither has an error channel. `ProviderFactory` has no scope either,
so the loader cannot tell before `build` whether a factory constructs eagerly. A singleton builds in
`build`, resolving its dependencies with `Execution::None`; the execution-scoped wrapper then
panics. The case is transitive (singleton, then transient, then execution-scoped). The
`#[injectable]` singleton factory's generated check panics too.

**Options, reasoned by Fable.**
1. **An error channel (Fable's verdict).** `Provider::resolve(&self, ctx) -> Result<Box<dyn Any +
   Send>, ResolutionError>` and `ProviderFactory::build(&self, deps) -> Result<Injectable, Box<dyn
   Error + Send + Sync>>`, ADR-0045's shape; the execution-scoped wrapper answers `Execution::None`
   with `ExecutionRequired { token }`; the loader wraps a failed `build` as a `StartupError` naming
   the provider and module, the source naming the dependency. The transitive case needs no graph
   walk; downcast failures become `TypeMismatch`; the generated check and the application's scope
   pre-check are deleted, one place refusing. Cost: every implementor, macro expansions, value API,
   integration crates and hand-written test providers, touched once. Rows 45 (`Injectable` becomes
   `Registration`) and 49 (`Arc<dyn Provider>` across the SPI) touch the same signatures, so it could
   be built as its own row here or folded into 49.
2. **A scope query on `ProviderFactory`.** A default of `Singleton` refuses a hand-written lazy
   factory for a dependency it never resolves eagerly, `Alias` cannot answer before its target
   builds, and every other build panic stays.
3. **A panic net around `build`.** Catches the wrong set: nothing under `panic = "abort"`, and a
   user's own `expect` in `#[new]` turned into a startup error; the constraint would be met for every
   future defect without a refusal being designed.

**Recommendation.** Option 1. A27's subject, `provider_factory!`'s scoped factory not caching, went
with the macro in row 33; the value API's `.per_execution()` caches per execution, so A27 closes on
the next audit whichever option is chosen.

**State.** Row 44 is halted with nothing built; its branch `feat/refuse-a-factory-on-a-per-call-dependency`
is empty over row 43 (#384). Rows 45 onward wait, stacking being literal: they can go on top of row
43 if the SPI change is folded into row 49, or on top of row 44 if it is built there.

**Answer (user, 2026-09-29).** Option 1, the error channel. Placed in row 44, the row that needs it,
so rows 45 onward stack on it; row 49 keeps its own change (`Box` to `Arc`). The user also approved
the in-row departures of rows 36, 39 and 40 as reported.

**Built (2026-09-29):** #385, four commits over #384. Two in-row departures, reported to the user
for approval with the PR: `ControllerFactory::build` carries the channel too, and F277's fix (a
per-call instance's transients built in its execution), without which the refusal fired inside a
live call. Row 44's packet in §8 records both. **Approved by the user, 2026-09-29.**

### Halt 8 — row 50 (DI-19), the `Shared` amendment cannot be built as written, 2026-09-30

**The row.** D14 (an injectable is shared: `Arc<T>` for singleton and execution, by value for
transient, the field written `Arc<T>`, the macro's `#[derive(Clone)]` deleted) with the 2026-09-28
amendment: a marker `Shared` ("cloning this shares it"), a plain `T` field only when `T: Shared`,
core implementing it for `Arc<T>`, the integration crates for the handles they register.

**What the compiler refuses (probed by Fable, rustc 1.98.1, scratch crates under
`scratchpad/row50/`).**
1. *Core implements `Shared` for `Arc<T>`*: the dispatch that tells an `Arc<X>` field from a plain one
   (`impl<T: ?Sized> X for Arc<T>` beside `impl<T: Shared> X for T`) is E0119 with that impl and
   coherent without it (`Arc` is not `#[fundamental]`). The impl buys nothing once `Arc<X>` is read
   by its shape.
2. *The integration crates implement it for their handles*: `sqlx::Pool`, `DatabaseConnection`,
   `ConnectionManager`, `mongodb::Database`, the deadpool pools and the Prisma client are foreign to
   the `ulo-db-*` crates, and so is the marker: E0117 (orphan rule). Only `ulo` could write those
   impls, with six optional driver dependencies in core, so "plain `T` only when `T: Shared`" cannot
   deliver `pool: PgPool`, the amendment's own example.

**Fable's design.** Whoever hands a value out says it is a handle: a hand-written provider by
answering it by value (all 24 in the tree already do, so `pool: PgPool` keeps working), a value
declaration by `.handle()`, and an `#[injectable]` type by implementing the marker. A user's rule:
`Arc<X>` always works; plain `X` works when the binding hands out `X` (a transient or a handle).
- Marker named `Handle` (ADR-0058 already calls the `Arc` answer "shared"); `.handle()` on a value
  declaration; `Provider::shape() -> Shape { Shared, Value }` reported by providers (default `Value`).
- A factory receives every dependency as `Arc<Dep>` (`async |cfg: Arc<Config>|`, ADR-0058's own
  spelling), the only coherent generic form; a by-value handle is wrapped for the build.
- `resolve`/`get` on the application, the context and `ModuleRef` return `Arc<T>`.
- `Arc<X>` over a transient binding wraps the fresh value rather than refusing.
- Refusals: compile time for a plain field of a shared `#[injectable]` type ("write `Arc<Svc>`, or
  implement `Handle`"); `create` for a plain field over a shared value-API binding
  (`value_dependencies()` on the factories, checked by the loader); resolution as the backstop
  (`ResolutionError::Shape`).
- ADR-0058's "`K::Value` for a transient one" gains "or a handle".

**State.** Row 50 halted with nothing built, on branch base `refactor/hold-a-provider-in-one-arc`
(#391). Rows 51–54 wait.

**Answer (user, 2026-09-30): the minimal version.** `Arc<T>` for every shared binding, services and
`provide!` values included; plain `X` only where the provider hands out a value (transients, the
hand-written handles); no `Handle` trait and no `.handle()` (a later, non-breaking addition if double
wraps prove a nuisance); the refusals kept; factories take `Arc<Dep>`; `resolve`/`get` return
`Arc<T>`. Illustrated on `examples/lifecycle_hooks.rs`, `config_module.rs` and `provider_patterns.rs`
before approval. Making `#[injectable]` a derive, possible once the `Clone` derive goes, was raised and
not filed (the user: the derive-plus-helper spelling is uglier).

**The field-spelling question, closed (user, 2026-09-30).** Asked: can the field be written `T` and
read as a key into the container, the scope deciding what arrives? No. A field typed `T` owns a `T`,
and the container has three ways to fill one: move its single instance out (the second consumer gets
nothing), clone it (D14's bug unless the author's `Clone` shares, the reasoning the user wants off
every author), or have the macro rewrite the type to `Arc<T>` (a lie that surfaces in a hand-built
struct, a `#[new]` taking `T`, an error naming `Arc<T>`). The key is still `T`, read out of the
`Arc` (ADR-0059); `Arc` is the shape the answer arrives in. Settled: `Arc<T>`, no desugar, no
`Handle` marker or `#[injectable(handle)]` flag, no `Clone` requirement. The documented rule is one
line, "write `Arc<T>`; it always works", with plain `X` accepted where the provider hands out a value.

### Halt 9 — row 51 (DI-20), the container cannot answer an `Extension<T>` key it knows only as a string, 2026-09-30

**The row.** "`ExtensionFactory<T>` deleted; the container answers any `Extension<T>` token from the
execution's bag" (row 51; `DI_BUILD_HANDOFF.md` PR 20; `DI_AUDIT.md` D14: "the container answers any
`Extension<T>` token from the execution's bag, the way it already answers `Extensions`"; ADR-0058:
"the container answers any `Extension<T>` key from the execution's bag").

**What the tree requires.** The container answers a `Box<dyn Any>` that the asker downcasts to the
type it expects. For a key it knows only as a string it cannot build an `Extension<T>`, whose `T` it
never sees. Built instead: the injection site answers it. A rung of `ulo::__di::Site` keys an
`Extension<T>` site by `Extensions` and views the bag as `T`, so every `#[inject]` field and `#[new]`
parameter (injectable, controller, gateway, enhancer builder) needs no declaration, and the
handoff's verification holds. Two askers are not injection sites and are generic over the type:
a `provide!` factory parameter `Arc<Extension<T>>` (probed: `create` fails with `Dependency not
found: …Extension<CurrentUser>`) and `get`/`resolve::<Extension<T>>` (`ProviderNotFound`). Both
worked with a declaration before; with the declaration deleted, neither can. ADR-0058's clause is
corrected in place to the injection site, which narrows the decision's words.

**Options.**
- (a) Accept the limit, as built: a factory takes `Arc<Extensions>` and a lookup
  `resolve::<Extensions>`, and the docs say so.
- (b) Keep an optional declaration: restore `DeclaresProvider` for `Extension<T>` with a provider
  answering the view, for a module that wants factory or lookup access; injection sites still need
  none. Two ways to reach one view, and the row's deletion becomes partial.
- (c) Make a factory parameter an injection site: a per-parameter key trait over `IntoFactory`'s
  arguments, F255's candidate fix, which also covers trait-object parameters.

**Recommendation: (a).** `Extensions` is a direct substitute in both places, (b) re-creates the
registration the row deletes, and (c) belongs with F255.

**State.** Rows 50 and 51 built. Row 51 is pushed as a draft PR on `refactor/read-an-extension-from-the-bag`
over #392, gate green (709 integration tests), the Law applied. Rows 52-54 wait.

### Pause — the sequence stops for an end-to-end DI redesign, 2026-10-01

**The decision.** The user paused the build sequence to take up an end-to-end redesign of dependency
injection, written by an outside design pass from a list of 48 capabilities with no reference to the
current container. The design, the capability list and a README sit on
`experiment/di-redesign` in `toni-rs/` (commit `929cfcfd`, cut from master `3cc36510`, local and
unpushed), under `docs/experiments/di-redesign/`. Whether it is phased into ulo or dropped is
undecided.

**State held.** The stack #375-#393 stays open and unmerged; the merge ceremony has not run. Rows 50
and 51 are #392 and draft #393; Halt 9 is unanswered. The rebuild of #392/#393 with `Dep<T>` sites
(`di/DI_CORE_MODEL.md`) does not start, and rows 52-54 do not start. PR #386 (the ADR README) is
open for the user.

**Resume condition.** The user decides whether the redesign phases in, and how; the sequence
resumes, or is re-planned against the redesign, on that answer.

### Question for the merge ceremony — row 40 (DI-9), how far `.or_global()` reaches, 2026-09-29

**Not a halt: the row is built and its PR is open, on the reading below.** It needs the user's word
before the wave merges, because DI's D1 carries two sentences that part on one case.

**The two sentences.** D1: "`.global()` becomes `.or_global()` and reaches only exported and global
tokens", and, giving the name, "current module first, then the global registry". They agree on a
private provider (unreachable) and a global module's export (reached). They part on an import's
non-global export: the module can inject it, the old `.global()` reached it, and "exported" admits
it while "then the global registry" does not.

**Built: (a), the global registry only.** The current module, then what the global modules export.
It is D1's operational sentence, and the registry is the one set where ADR-0029 already refuses two
answers for a key, so the fallback never has to choose. ADR-0057 is accepted saying so.

**The alternative: (b), what the module could inject.** The current module, then its imports'
exports, then the global registry, with two imports exporting one key answering `AmbiguousModule`.
It fits D1's reason ("a provider a module did not export should not participate") and "exported".
A follow-up commit on row 40's branch: the import walk in `ModuleRefQuery::execute`, the ADR
sentence, and one case in `ambiguous_lookups.rs`. A third reading, any module's export, has D1's
reason behind it too, and puts back part of the reach D1 narrowed.

**State.** Rows 41 onward were built on (a); none reads `.or_global()`'s reach, and they are rebased onto (b).

**Answer (user, 2026-09-29).** (a): only what the global modules export is reachable past the
current module; an export to an importer that is not global does not count.

**Revised (user, 2026-09-29): (b).** The first answer was read from "only exported global is as far
as it can go"; asked the one case the readings part on (a non-global import's export the module can
inject), the user chose (b). Nest was checked on the way and is precedent for neither: `strict`
reads the host module alone, `strict: false` reads every module with exports ignored and the last
holder winning. Built on row 40's branch as two commits on top of its three, the log kept as a
process record; rows 41-44 rebased onto them. The name followed (user, 2026-09-29, on Fable's
reasoning): `.visible()`, the set being what the module can see; `.or_global()` named one of three
tiers and `or_` put the module outside its own set. No `strict` wording anywhere: it is Nest's.

## 7. Plan-text discrepancies — amended 2026-09-24 on the user's authorisation

Textual, not sequencing. Found by the triage, held here until the user read them, then applied:
one line or one phrase each, in the document named. None changes what a row does.

- `WS_BUILD_HANDOFF.md`: the "Read first" list says "D1–D7 settled, D8 open" and the Status
  section says D8 is settled; two headings are duplicated ("## Read first, in this order## Read
  first…", "## Verification each PR needs## Verification…"); its Phase 1 table skips a PR 6.
  `WS_AUDIT.md`'s closing line still says "D8 is open".
- `RPC_BUILD_HANDOFF.md` Phase 3 numbers the call-shape PRs 20–23; `project-rpc-request-side-
  streaming` in memory says "four PRs (18–21)", written before credit took 18 and 19.
- `GRPC_AUDIT.md` §7 item 3 scopes F240 to a diagnostic; `GRPC_BUILD_HANDOFF.md`'s PR 3 section,
  written at approval, scopes it to the structural fix and calls the diagnostic-only version a
  divergence. The handoff is followed (§2).
- `GRPC_BUILD_HANDOFF.md`'s PR 4 row says F241's real fix "rides PR 6"; the memo says it rides
  F237's trailer work, which is PR 6. Consistent; row 19 carries it.
- `RENDER_AUDIT_HANDOFF.md`'s closing note: no build handoff's "read first" list names F242–F244.
  Rows 58, 85–87 carry them.
- The workspace `CLAUDE.md` says `DOCS_AUDIT.md` §18 "is consumed and empty"; §18 carries drift
  lines since `fd9dc5e` (#278 onward). The build appends; the documentation-site audit clears.
- `HTTP_BUILD_HANDOFF.md` sends the builder to "F105–F120 for the defects", a range that holds
  F117; `HTTP_AUDIT.md` §10 leaves F117 out of the sequence. S1 resolves it.

---

## 8. The merging ceremony, and the bookkeeping owed at it

The user calls it. Until then every PR sits open on the stack, reviewed as it opened.

**The procedure.** Bottom-up in row order: `gh pr merge <n> --merge --delete-branch`; confirm the
PR above it now targets master (GitHub retargets when a base branch is deleted) and that its
checks are green on the retargeted base — if not, rebase that branch onto master, re-gate, push
with lease, wait for green; apply the merged row's bookkeeping packet below; set the row to
`[m] #n <sha>`; next. A packet is applied only after its PR has merged, so the three untracked
files never describe a state master does not have.

**Bookkeeping packets**, one per opened PR, written by step 11 of §4. Each names: the row; the PR;
the `FRAMEWORK_GAPS.md` entries to delete, each clause verified against the branch, with any
surviving clause and its residual text; the workspace `CLAUDE.md` rows the change falsifies and
their corrected text; the `DOCS_AUDIT.md` §18 line.

### Row 1 — WS-7 — PR #341 (`fix/answer-a-close-with-a-close`, base `master`)

- **`FRAMEWORK_GAPS.md`:** delete **F196** whole. Every clause verified against the branch: the
  four adapters that wrote no Close (salvo, poem, rocket, `ulo-ws-tungstenite`) now poll the read
  half once more after a Close and the echo arrives from all five, asserted per adapter in
  `integration-tests/tests/integration/ws_adapter_conformance.rs`; the axum arm the entry names
  (`Err(_) => {}`) is unchanged and now commented; the entry's own carve-out — F197 is not closed
  by this — is F197's and F197 is untouched.
- **Workspace `CLAUDE.md`:** no row falsified. The WebSocket section's "The read loop stops only
  when the client is already gone" holds, and the adapter table carries no close-handshake column.
- **`DOCS_AUDIT.md` §18:** `- **#341 — every adapter answers a peer's Close with a Close** (merged
  <date>). No page claimed the old behaviour: \`websocket/gateways.mdx\` describes the close frames
  a refusal writes and \`WsMessage::close\`, not the closing handshake a peer starts. Nothing to
  correct.`

### Row 2 — WS-9 — PR #342 (`test/a-websocket-adapter-conformance-suite`, base `fix/answer-a-close-with-a-close`)

- **`FRAMEWORK_GAPS.md`:** delete **F36** whole. Clause by clause against the branch: "no
  conformance suite" — `ws_adapter_conformance.rs` stamps five cases per adapter with a
  `macro_rules!`, five adapters plus a silent-server control; "one suite instantiated per
  implementor" — one file, five stamps; "tungstenite's separate-port-only nature written into the
  suite as the exception" — its stamp names `separate_port(..)` where the four HTTP adapters name
  `same_port(..)`, and the header says why. The candidate's four cases: upgrade on the declared
  path and a message round-trip are in every stamp, a gateway matched by port is tungstenite's
  stamp; a refused connection is not in this suite — it is pinned on one adapter by
  `ws_connect_refusal.rs`, and rows 4 and 94 add refusal frames per adapter. The entry's
  `WebSocketAdapter` name is the pre-#270 spelling of `WsAdapter`.
- **Workspace `CLAUDE.md`:** no row falsified. Nothing there enumerates the conformance files.
- **`DOCS_AUDIT.md` §18:** `- **#342 — a WebSocket wire conformance suite** (merged <date>).
  \`testing/integration.mdx:177\` describes the framework's conformance suite as the HTTP chain,
  unrouted paths and method mismatches; the WebSocket wire now has one too. Incomplete rather than
  wrong.`

### Row 3 — WS-1 — PR #343 (`docs/the-close-code-registry-has-auth-codes`, base `test/a-websocket-adapter-conformance-suite`)

- **`FRAMEWORK_GAPS.md`:** **F202**, the comment half only. The clause "the comment defending the
  choice is wrong" and "correct the comment in either case" are met: `close_code`'s doc, the
  `ws_connect_refusal.rs` comment and ADR-0036's decision bullet each name the registry's 3000,
  3003 and 3008 and no longer claim the RFC has no auth code. The mapping half — whether
  `ErrorKind` moves onto 3000/3003/3008 — is D4, row 95, and the entry stays until then with a
  note that its comment clause is spent.
- **Workspace `CLAUDE.md`:** no row falsified; the WebSocket row states the 1008/1013/1011 mapping
  without the rationale.
- **`DOCS_AUDIT.md` §18:** `- **#343 — the close-code registry has auth codes** (merged <date>).
  Two pages repeat the corrected claim: \`websocket/gateways.mdx:226\` ("RFC 6455 defines no
  auth-specific code, which is why an unauthorized connect also closes with 1008") and
  \`techniques/cors.mdx:72\` ("it has none for authentication"). Both are wrong rather than stale:
  the IANA registry RFC 6455 §11.7 creates carries 3000 Unauthorized, 3003 Forbidden and 3008
  Timeout. The mapping they describe is unchanged until row 95 moves it.`

### Row 4 — WS-2 — PR #344 (`fix/a-refused-message-closes-with-its-code`, base `docs/the-close-code-registry-has-auth-codes`)

- **`FRAMEWORK_GAPS.md`:** delete **F201** whole. Clause by clause against the branch: "sends an
  envelope and no close frame when a message handler returns it" — `to_message` renders
  `Refused` as `close_with(code, reason)`, asserted on all five adapters by
  `ws_adapter_conformance.rs`'s `a_refused_message_closes_with_its_code`; "`close_code` is never
  consulted" — the variant carries its own code and the close frame carries it; "the connect path
  works" — `refusal_frames` unchanged; "blocks F187's candidate fix" — unblocked, GQL-6 (row 126)
  may route through `Refused`. The candidate's "needs the message path to be able to answer with
  a close and end the connection" — a handler's Close already went out through the sink; the
  connection ends when the peer answers it, which the case asserts.
- **Workspace `CLAUDE.md`:** the WebSocket row's "Every message failure answers the canonical
  envelope `{"status":"error","kind":…}`: a guard rejection, an interceptor's refusal, a panic,
  an event nothing handles (`NotFound`) and a frame naming no event (`BadRequest`) alike." gains:
  "A handler's `WsError::Refused` is the exception on both paths: one close frame carrying its
  code, no envelope."
- **`DOCS_AUDIT.md` §18:** `- **#344 — a refused message closes with its code** (merged <date>).
  \`websocket/gateways.mdx:341\` ("`Refused { code, reason }` | `BadRequest` — but a refused
  *connect* sends no envelope at all, only the close") describes the envelope the message path
  no longer writes: `Refused` is one close frame on both paths.`

### Row 5 — WS-3 — PR #345 (`test/pin-the-redis-broadcast-envelope`, base `fix/a-refused-message-closes-with-its-code`)

- **`FRAMEWORK_GAPS.md`:** **F210**, the pin half only. "Nothing in the repository asserts the
  shape, so a `#[serde(rename)]` or a variant addition changes the wire format with no test
  failing" — `crates/ulo-ws-redis/src/message.rs` pins the envelope as literal JSON per target
  kind and per `WsMessage` variant, and pins that an unknown variant fails to parse; the crate's
  first unit test, recorded in `coverage_ledger.rs`. The entry's other half — "decide whether the
  broadcast format is `WsMessage`'s derived representation or a type of its own" — is D7, row 98,
  and the entry stays until then with a note that its pin clause is spent.
- **Workspace `CLAUDE.md`:** no row falsified; the `ulo-ws-redis` row describes keys and
  delivery, not the envelope.
- **`DOCS_AUDIT.md` §18:** nothing to record; no page describes the channel's bytes.

### Row 6 — WS-4 — PR #346 (`refactor/ws-message-is-non-exhaustive`, base `test/pin-the-redis-broadcast-envelope`)

- **`FRAMEWORK_GAPS.md`:** delete **F208** whole. Clause by clause against the branch: "carries
  no `#[non_exhaustive]`" — it does, with the reason in its doc. One correction the entry and
  `WS_AUDIT.md` §8 share: both cite "RFC 6455 §11.8" for the reserved opcode ranges, and §11.8
  (the opcode registry) carries no reservation language — the ranges are §5.2's ("%x3-7 are
  reserved for further non-control frames", "%xB-F ... further control frames"), repeated in §5.5
  and §5.6. The doc comment cites §5.2; the memo is frozen and is not edited; "adding a variant later is
  a breaking change for every downstream `match`" — the five outbound conversions, the only
  matches outside `crates/ulo`, carry a wildcard arm; "the same enum is the cross-process
  broadcast format, which is F210" — pinned by row 5, and D7 (row 98) gives the wire its own type.
- **Workspace `CLAUDE.md`:** the WebSocket row's "`WsMessage` variants
  `Text`/`Binary`/`Ping`/`Pong`/`Close(Option<CloseFrame>)`" gains "(`#[non_exhaustive]`; a
  `match` outside core carries a wildcard arm)".
- **`DOCS_AUDIT.md` §18:** `- **#346 — `WsMessage` is non-exhaustive** (merged <date>). No page
  matches on the enum; \`websocket/gateways.mdx\` lists its constructors only. Nothing to correct.`

### Row 7 — WS-5 — PR #347 (`fix/a-session-is-built-by-the-framework`, base `refactor/ws-message-is-non-exhaustive`)

- **`FRAMEWORK_GAPS.md`:** delete **F214** whole. Clause by clause against the branch: "derives
  `Default`, so `Session::default()` is public" — the derive is gone and a `compile_fail` doctest
  pins that it does not build; "`Session::new()` has exactly one caller" — still one,
  `ws_client.rs`, and `new` is `pub(crate)` with a second `compile_fail` doctest; "drop the
  `Default` derive and make `new()` `pub(crate)`" — done as written. The route the entry did not
  name, `WsClient::new(id).session()`, is F246, filed by this row.
- **Workspace `CLAUDE.md`:** no row falsified; the WebSocket row describes `Session`'s accessors
  and lifetime, not its construction.
- **`DOCS_AUDIT.md` §18:** nothing to record; no page constructs a `Session`.

### Row 8 — RPC-1 — PR #348 (`fix/refuse-a-binary-payload-on-tcp-and-udp`, base `fix/a-session-is-built-by-the-framework`)

- **`FRAMEWORK_GAPS.md`:** delete **F217** whole. Clause by clause against the branch: "reaches
  the handler as `Json(null)`, and nothing refuses it" — `data_to_json` on both clients answers
  `RpcClientError::Transport` for `Binary`, ahead of the lazy connect, and
  `rpc_binary_payload_is_refused.rs` asserts every verb's refusal and an untouched socket on
  both transports; "the loss recorded only in a source comment" — the comment is the refusal's
  doc; "`Transport(...)` is the variant to hand back before any I/O" — as written; "the refusal
  moves into the capability at PR 9" — RPC-9 (row 78) absorbs it.
- **Workspace `CLAUDE.md`:** the `ulo-rpc-tcp` row's "binary → null" becomes "a `Binary`
  payload is refused by the client before any I/O (`RpcClientError::Transport`); a binary stream
  item still travels under `stream_b64`". The `ulo-rpc-udp` row says "Same envelope" and carries
  no binary clause; it inherits the tcp row's.
- **`DOCS_AUDIT.md` §18:** `- **#348 — a binary payload is refused on tcp and udp** (merged
  <date>). \`microservices/tcp-adapter.mdx:143\` and \`microservices/udp-adapter.mdx:126\` say a
  request payload or single reply carrying \`RpcData::Binary\` "becomes \`null\` on the wire";
  the request half is wrong after this: the client refuses it with \`RpcClientError::Transport\`
  before connecting. The single-reply half still holds (a handler's binary reply degrades to
  \`null\`), as do the base64 advice and the \`stream_b64\` exception.`

### Row 9 — RPC-4 — PR #349 (`test/the-conformance-suite-runs-on-tcp-and-udp`, base `fix/refuse-a-binary-payload-on-tcp-and-udp`)

- **`FRAMEWORK_GAPS.md`:** **F233**, the `Broker` half. "`Broker` is implemented in five
  `tests/conformance.rs` files ... and not for `ulo-rpc-tcp` or `ulo-rpc-udp`" — both now carry
  one, and the six cases pass on both. The entry's other halves — the three probe cases promoted
  (row 10) and the CI job (row 11) — stay until those rows merge; note on the entry that the
  `Broker` clause is spent.
- **Workspace `CLAUDE.md`:** the MSRV paragraph's "`ulo-rpc-conformance` is `publish = false` — a
  dev-dependency of the five broker transports (nats, redis, rabbitmq, mqtt, kafka), never
  released. Each of the five invokes `conformance_suite!` from a `tests/conformance.rs` carrying
  `#![cfg(feature = "integration")]`" becomes "a dev-dependency of all seven RPC transports.
  The five broker transports invoke `conformance_suite!` from a `tests/conformance.rs` carrying
  `#![cfg(feature = "integration")]`; tcp and udp invoke it with no feature gate, through a proxy
  the test owns, so a plain `cargo test -p ulo-rpc-tcp` runs six cases". The sentence "a plain
  `cargo test -p ulo-rpc-nats --test conformance` reports `ok. 0 passed`" stays true.
- **`DOCS_AUDIT.md` §22 B38:** closes. B38 says the five broker crates' comments overstate "the same six
  cases every transport answers"; with tcp and udp stamping the suite, every transport does answer
  them, so the comments are true as written and the candidate fix ("every broker transport") is
  not applied.
- **`DOCS_AUDIT.md` §18:** nothing to record; no page enumerates which transports the suite covers.

### Row 10 — RPC-5 — PR #350 (`test/promote-the-wire-probes-into-the-suite`, base `test/the-conformance-suite-runs-on-tcp-and-udp`)

- **`FRAMEWORK_GAPS.md`:** **F233**, the probe half: "the three probe cases promoted, kept in the
  session scratchpad as `probe-cases.rs`" — promoted into the default list as assertions on all
  seven. With rows 9 and 11 the entry deletes whole. What each promoted case pins is the
  cross-transport invariant only; rows 108 (F216's uniform matrix) and 112 (F218's declared
  mode) tighten them, and row 108's verification line ("every row identical across all seven")
  is where the payload matrix becomes an assertion.
- **Workspace `CLAUDE.md`:** no row falsified; the MSRV paragraph names the suite and not its
  case count.
- **`DOCS_AUDIT.md` §18:** nothing to record; no page counts the suite's cases.

### Row 11 — RPC-CI — PR #351 (`ci/run-the-broker-conformance-suites-on-merge`, base `test/promote-the-wire-probes-into-the-suite`)

- **`FRAMEWORK_GAPS.md`:** **F233**, the CI half ("nothing in CI runs this, and now that the cases
  pass, nothing will notice when they stop"): the `rpc conformance (brokers)` job runs the nats,
  redis, mqtt and rabbitmq suites on every push to master; kafka has `make conformance-kafka`. With
  rows 9 and 10 the entry deletes whole. D6's "(b) a local `make` target and a line in
  `CONTRIBUTING`": the repo has no `CONTRIBUTING` file, so the Makefile's own comment and the kafka
  crate's test-location comment carry the line. The kafka figure: F233's 578 s and this build's
  147 s are both stated; the memo's "nine minutes" is the first of the two.
- **Workspace `CLAUDE.md`:** the MSRV paragraph's "CI's `check (all features, all targets)` job is
  the only thing that compiles the suite, and nothing anywhere runs it — deliberately, since it
  needs five brokers under Docker" became "CI's `rpc conformance (brokers)` job runs the nats,
  redis, mqtt and rabbitmq suites on every push to master, one crate at a time; kafka runs by hand
  through `make conformance-kafka`". The same paragraph still counted five `Broker`
  implementors after row 9 made it seven; corrected to seven, with the tcp and udp stampings named
  as ungated.
- **`DOCS_AUDIT.md` §18:** nothing to record; no page describes CI.
- **Gate:** fmt, both checks, docs, the kafka crate's own tests and `make conformance-kafka`
  (147 s). The workspace test suites were not re-run: the diff is a workflow, a Makefile and one
  `//` comment.

### Row 14 — RPC-2 — PR #352 (`docs/say-what-connect-and-close-do-on-the-client`, base `ci/run-the-broker-conformance-suites-on-merge`)

- **`FRAMEWORK_GAPS.md`:** **F225**, the doc half — with a correction appended to the entry. Its
  `connect` clause ("an automatic call that nothing makes") is wrong about the code and right about
  every registration a macro can write: `impl Provider for RpcClient` calls `connect` from
  `on_application_bootstrap` and `close` from `on_application_shutdown`, the container runs both
  for a client it holds as its own `Provider`, and the macro spelling of that registration (the
  `lifecycle` flag) does not compile — `DOCS_AUDIT.md` §17 **A8**, open, and in no row of this
  sequence. The four docs name the condition without the flag. The `close` clause stands; the docs
  say the default does nothing and one override flushes. The entry's flush half is RPC-11
  (row 109); the entry stays until then. Not a divergence: the row's deliverable — the two
  lifecycle docs true to the tree — is unchanged; what the truth was is what the finding got
  wrong, and the Law's pass on this row is what found A8 standing behind it.
- **Also in the diff:** the `RpcClient` type doc's `provide_factory!` is spelled `provider_factory!`,
  the macro's name, and its `#[inject(token = …)]` is spelled `#[inject("INVENTORY_CLIENT")]`, the
  attribute's grammar — both halves of `DOCS_AUDIT.md` §17 **B13**, which closes. Its example still
  passes the `lifecycle` flag (A8, the macro's fix).
- **The Law:** three passes on this row. The first found the `lifecycle` flag behind F225 (A8); the
  second found `connect`'s startup promise false for kafka, mqtt and udp and the `close` conditional
  false for mqtt; the third found the reachable-peer case misdescribed and the roster claim in
  `RpcClient::connect`'s doc. The third pass's corrections were applied as given and the texts were
  not put to a fourth.
- **Workspace `CLAUDE.md`:** no row falsified; the RPC section says "lazy connect — no I/O in
  constructors" and describes neither hook nor flush.
- **`DOCS_AUDIT.md` §18:** one line appended: `microservices/index.mdx` (line 101, "Call
  `connect()` in an `#[on_application_bootstrap]` hook when you would rather learn at startup")
  and `microservices/rpc-controllers.mdx` (line 548, "`close()` flushes pending messages and
  closes; call it in an `#[on_application_shutdown]` hook") state the old promises: the flush
  holds on nats only, and closing closes nothing. `microservices/nats-adapter.mdx` line 99
  ("`close()` flushes pending publishes") holds for that transport.

### Row 15 — RPC-3 — PR #353 (`docs/say-what-the-err-lane-means`, base `docs/say-what-connect-and-close-do-on-the-client`)

- **Also in the diff:** three docs that named a guard's refusal as a wire-`err` frame, stale since #310
  and pointed at by the new text — `Forbidden`'s variant doc, `GuardRejection`'s doc
  (`errors/framework.rs`) and the `ClaimsWithAnError` doc in
  `error_handler_answers_a_handlers_type.rs` — say the refusal is the `Forbidden` envelope on
  `response`; `frame_panic`'s doc names the dispatch's own panic rather than a handler's.
- **`FRAMEWORK_GAPS.md`, filed:** **F247** — `RpcError::Forbidden`'s `Display` reads "Guard rejected
  message: …" for a variant no guard builds; wire text, not this row's.
- **The Law:** two passes. The first rejected "whether a controller answered picks the lane" (a
  global `#[catch(Unrouted)]` handler's `Ok` rides `response` with no controller reached) and the
  reachability of `frame_response`'s `unsupported` arm; the second found the type doc still
  carrying the rejected rule and the three adjacent stale docs. The second pass's corrections were
  applied as given and put to a third pass (on Opus; the Fable rerun failed on credits), which found
  the construction-panic path and filed it.
- **`FRAMEWORK_GAPS.md`, filed:** **F248** — a panic while building a per-call controller or an
  execution-scoped enhancer escapes the chain and answers on `err`; row 28's `run_guards` is where
  the guard half would be recovered, and the plan does not name it, so it stays filed.

- **`FRAMEWORK_GAPS.md`:** **F232** closes whole; the entry deletes at row 134. "the module doc on
  `ulo::rpc::wire` states a lane rule that two of the same file's own branches break" — the doc
  now states the rule the `Result` picks the lane: `Ok` on `response`, `Err` on `err`. The entry's
  own candidate fix ("the `err` lane is a call that reached no controller, the `response` lane is
  a call that reached one") is wrong the same way the old doc was: a miss a global
  `#[catch(Unrouted)]` handler recovers with `Ok` rides `response` with no controller reached,
  which `unrouted_is_an_event.rs` pins. The doc names the dispatch panic the adapters frame with
  `frame_panic`, and the `"status": "unsupported"` refusal as `frame_response`'s answer to a
  stream — reached by no shipped adapter, all seven driving a stream through
  `drive_reply_stream`; that branch is F223's and stays for row 115. `RpcError`'s doc and
  `to_data`'s say the same rule, and `Forbidden`'s variant doc no longer claims a guard's refusal
  builds it (nothing in the framework does; a refusal is `AppError(GuardRejection)`).
- **Workspace `CLAUDE.md`:** the "Wire semantics (all)" paragraph gains the recovered miss beside
  the success frame: "A miss a global `#[catch(Unrouted)]` handler recovers with `Ok` answers on
  that lane too, no controller reached: the `Result` picks the frame, not the route." 
- **`DOCS_AUDIT.md` §18:** one line appended: `microservices/rpc-controllers.mdx` line 222
  ("`PatternNotFound` / `Forbidden` / `Internal` become an error end") carries the enumeration the
  in-source docs drop; the rule is the lane's meaning, and a guard's refusal is on the response
  lane.

### Row 13 — RPC-8 — PR #354 (`feat/name-a-kafka-clients-reply-topic`, base `docs/say-what-the-err-lane-means`)

- **The Law** found a defect the staged change introduced: the correlation-id prefix was the reply
  topic, so a named client repeated `name:0`, `name:1`, … across restarts, colliding in the
  server's shared cancel registry. Correlation ids now use a per-transport `client_id` whatever the
  topic is called. Not pinned by a test: the collision needs a dead client's stream still
  registered on a live server, which the reply-topic test does not stage.

- **`FRAMEWORK_GAPS.md`:** **F227**. Clause by clause: "creates a topic and a consumer group per
  process and deletes neither" — a named client reuses one topic and one group across restarts,
  pinned by `crates/ulo-rpc-kafka/tests/reply_topic.rs`; "every auto-created topic is
  single-partition and unreplicated ... no knob changes" — `with_topic_partitions` and
  `with_replication_factor` on both sides. Surviving clause: "The server does the same for its
  cancel consumer: group `ulo-rpc-cancel-{pid}-{nanos}`" — each server instance still names its
  cancel group per process (every instance must see every cancel notice, so instances cannot
  share one group, and no instance identity exists to name one stably); and "or a way to decline
  auto-creation entirely" is not built. Residual text for the entry: the server's cancel group and
  the auto-creation opt-out.
- **Workspace `CLAUDE.md`:** the `ulo-rpc-kafka` row gains "`with_reply_topic(name)` (a stable
  reply topic and group per logical client; unset, per process), `with_topic_partitions` /
  `with_replication_factor` on adapter and client (default 1 / 1)".
- **`DOCS_AUDIT.md` §18:** `- **#<n> — a named Kafka reply topic and topic-shape knobs** (merged
  <date>). \`microservices/kafka-adapter.mdx\` lists the adapter and client builders; three are
  new and not on the page.`

### Row 16 — GRPC-1 — PR #355

- **`FRAMEWORK_GAPS.md`:** **F239** closes whole (the entry deletes at row 134): `split_at_checked` at the one site, the non-ASCII
  case in the test beside it, the doc's promise now kept.
- **Workspace `CLAUDE.md`:** no row falsified.
- **`DOCS_AUDIT.md` §18:** nothing to record; no page describes the parser.

### Row 17 — GRPC-7 — PR #356 (`feat/fire-the-token-at-a-grpc-deadline`, base `fix/read-a-grpc-timeout-ending-mid-char-as-absent`)

- **The Law** found the staged timer was never disarmed: a call that answered before its deadline
  still had its token fired at the deadline, and each call held a task until then (unbounded for a
  huge `grpc-timeout`). The timer now races a sender held in the execution's extensions and exits
  without firing when the execution ends, pinned by the answered-early case and falsified against
  a timer that ignores the end. It also found the staged doc inserted between `to_status`'s doc and
  its item, which would have given `arm_deadline` both docs; moved below `to_status`. A second
  pass found a reference cycle (the installed request's extensions hold the context) that kept an
  execution alive whenever nothing took the request — a refused call's timer then fired at the
  deadline; the expansion now empties the request slot after the pipeline, pinned by the
  refused-call case, which failed red. A third pass found that release was a plain statement
  after the pipeline, skipped when tonic drops the call first; it is now a drop guard
  (`ulo::__grpc::ReleaseRequest`) bound after the install, pinned by a dropped-call case that
  checks the execution is freed and failed red. The leak itself dates from #311.
- **`FRAMEWORK_GAPS.md`, filed:** **F249** (two contradicted in-source docs), **F250** (HTTP fires
  the token for a buffered body abandoned before it drained).
- **The unary deadline case moved to row 18.** tonic drops a unary future at the deadline; the
  handler cannot observe a token that fires then, and firing it on that drop is F235's abandonment
  guard. Row 18's test carries the case.

- **`FRAMEWORK_GAPS.md`:** **F236** closes whole (the entry deletes at row 134): the token fires at the deadline from the one
  place that builds the context, unary and streaming alike, pinned by
  `grpc_deadline_fires_the_token.rs`; the "status is tonic's and is also wrong" half is the
  entry's own dependency note, not a clause of the fix. `CancellationToken`'s falsified sentence
  is rewritten (F235 names it too; F235 stays for row 18).
- **Workspace `CLAUDE.md`:** the Contexts row's "`deadline()` (gRPC reads the caller's
  `grpc-timeout`; `None` on the other three, which carry nothing to read — ulo reports it and
  enforces it nowhere, and tonic's enforcement ends when the handler returns its `Response`, so a
  streaming reply outlives its deadline without bound, F236)" becomes "`deadline()` (gRPC reads
  the caller's `grpc-timeout`; `None` on the other three, which carry nothing to read — the
  execution's token fires when it passes, whatever the answer's shape; tonic's own race ends
  when the handler returns its `Response`)". The `CancellationToken` clause in the same row
  ("buffered responses never fire it") gains "except at a gRPC deadline".
- **`DOCS_AUDIT.md` §18:** `- **#<n> — the token fires at a gRPC deadline** (merged <date>). The
  gRPC pages that describe \`deadline()\` as read-only (grep at the ceremony) gain the token.`

### Row 18 — GRPC-8 — PR #357 (`feat/fire-the-token-when-a-unary-call-is-abandoned`, base `feat/fire-the-token-at-a-grpc-deadline`)

- **Carries the unary deadline case** moved from row 17: tonic drops a unary future at the deadline, and this row's guard is what fires the token on that drop.

- **`FRAMEWORK_GAPS.md`:** **F235** closes whole (the entry deletes at row 134): the guard on
  `run_grpc_pipeline`'s future fires the token when a unary call is abandoned, pinned by the
  deadline and abandoned-call cases in `grpc_deadline_fires_the_token.rs`; the token's doc sentence the entry
  names is rewritten (row 17 rewrote it first for the deadline; this row adds abandonment and
  names the three transports where a buffered answer still fires nothing). The drop-site design
  the row was to validate: the future `run_grpc_pipeline` returns is the one tonic drops, and a
  guard local to its body fires on that drop — validated by the case, which fails without the
  guard.
- **Workspace `CLAUDE.md`:** the Contexts row's "buffered responses never fire it except at a gRPC
  deadline" becomes "buffered responses never fire it on HTTP, RPC and WebSocket; on gRPC a call the
  caller abandons before it answers — at its deadline or by going away — fires it, through a guard
  on the pipeline's future".
- **`DOCS_AUDIT.md` §18:** `- **#<n> — an abandoned unary gRPC call fires the token** (merged
  <date>). The gRPC pages describing cancellation as the streaming reply's alone (grep at the
  ceremony) gain the unary case.`

### Row 20 — GRPC-4 — PR #358 (`docs/say-that-details-do-not-travel-on-grpc`, base `feat/fire-the-token-when-a-unary-call-is-abandoned`)

- **`FRAMEWORK_GAPS.md`, filed:** **F251** — a `Validated` failure answers `Internal` on gRPC and a string `details` on HTTP.

- **`FRAMEWORK_GAPS.md`:** **F241**, the doc half: "the honest small step is to state the limit in
  `to_tonic`'s doc and in `Error::details()`'s" — stated on `to_status` (the public one; `to_tonic`
  is private and one line below it) and on `details()`. The other half, writing
  `grpc-status-details-bin`, is row 19 (GRPC-6); the entry stays until then with a note that its
  doc clause is spent.
- **Workspace `CLAUDE.md`:** the Errors row lists `details` among the trait's three methods
  without a transport clause; nothing falsified. The ulo-grpc row's "**Errors:**" clause gains
  "`details()` does not travel until `grpc-status-details-bin` is written (GRPC-6)".
- **`DOCS_AUDIT.md` §18:** `- **#<n> — details do not travel on gRPC** (merged <date>).
  \`grpc/error-handling.mdx\` maps kinds to codes and says nothing of \`details\`; a line saying it
  is dropped on this transport is owed.`

### Row 21 — GRPC-5 — PR #359 (`feat/warn-for-a-grpc-service-bypassing-the-pipeline`, base `docs/say-that-details-do-not-travel-on-grpc`)

- **Also in the diff:** `add_service`'s rustdoc, the crate doc's mixing section and the README say the warning is logged and how to silence it; the warning sits after the TLS step, the last that can fail `bind`. The level is `warn` as the row names it; whether the sanctioned health/reflection path deserves `info` or an opt-out is a question for the ceremony.

- **`FRAMEWORK_GAPS.md`:** no entry; gRPC D2's startup-warning half. Row 101 (PIPE-12) carries the
  other half, the pre-dispatch anchor above tonic's router.
- **Workspace `CLAUDE.md`:** the ulo-grpc row's "`.add_service(svc)` (non-DI, bypasses enhancers)"
  gains "; warns at `bind()` naming each such service".
- **`DOCS_AUDIT.md` §18:** `- **#<n> — a warning per add_service service** (merged <date>). The
  gRPC page that describes \`add_service\` for reflection and health gains the warning.`

### Row 22 — GQL-1 — PR #360

- **`FRAMEWORK_GAPS.md`:** **F185** closes for its content-type clause, which is the candidate fix
  (the entry deletes at row 134). Its second observation — `GET /graphql?query=…` answers the playground rather than a result
  or a refusal — is not this row's and is F184/F186 territory (row 125, the HTTP binding);
  residual text for the entry if the user keeps it: that one sentence.
- **Workspace `CLAUDE.md`:** no row falsified; the GraphQL row says `.with_playground` (default =
  debug builds) and nothing of the content type.
- **`DOCS_AUDIT.md` §18:** nothing to record; the pages tell a reader to open the playground and
  say nothing of its content type.

### Row 23 — GQL-2 — PR #361 (`docs/delete-the-juniper-crates-test-location-comment`, base `fix/serve-the-graphql-playground-as-html`)

- **`FRAMEWORK_GAPS.md`:** **F193** closes whole (the entry deletes at row 134): the comment is gone, as
  the candidate fix asks.
- **Workspace `CLAUDE.md`:** no row falsified.
- **`DOCS_AUDIT.md` §18:** nothing to record.

### Row 24 — EXT-13 — PR #362 (`feat/re-export-the-crates-the-extraction-surface-names`, base `docs/delete-the-juniper-crates-test-location-comment`)

- **Design note for the ceremony:** #291 cut the root to "one path per item" and five dependency re-exports that macro expansions name. These three are named by public signatures, not by expansions, and `ulo::multer::Field` is a second path to `ulo::http::extract::Field`; the root comment names the extract path as the one to write. The plan offered re-export or wrapping; re-export follows the root's stated reason. `DOCS_AUDIT.md` and `FRAMEWORK_GAPS.md` record the root's `pub use` count and the five re-exports; both go stale with this.

- **`FRAMEWORK_GAPS.md`:** **F172** closes whole (the entry deletes at row 134), by the first of its two
  candidate fixes
  (re-export under the root's rule); the wrapping alternative is not taken, the root's own rule
  deciding it.
- **Workspace `CLAUDE.md`:** the `ulo::prelude` row's "the crate root still re-exports the
  pre-reorganisation set" gains "plus `bytes`, `multer` and `validator`, which the extraction
  surface names".
- **`DOCS_AUDIT.md` §18:** `- **#<n> — `multer`, `validator` and `bytes` re-exported** (merged
  <date>). The extractors pages that tell a reader to add \`validator\` to their manifest (grep at
  the ceremony) can point at \`ulo::validator\` instead.`

### Row 25 — EXT-15 — PR #363 (`test/cover-every-body-arm-and-a-multipart-round-trip`, base `feat/re-export-the-crates-the-extraction-surface-names`)

- **Falsified:** the urlencoded case fails when that arm parses JSON instead.

- **`FRAMEWORK_GAPS.md`:** **F173** closes whole (the entry deletes at row 134): one test per `parse_bytes` arm and one multipart
  round trip with a file part and a text part, as the candidate fix asks.
- **Workspace `CLAUDE.md`:** no row falsified.
- **`DOCS_AUDIT.md` §18:** nothing to record.

### Row 26 — PIPE-ADR — PR #364 (`docs/propose-the-pipelines-order-and-the-pre-dispatch-role`, base `test/cover-every-body-arm-and-a-multipart-round-trip`)

- **A split in the plan, for the ceremony:** the handoff assigns D2 — including the non-singleton
  `Middleware` refusal — to ADR-0056, while BUILD_SEQUENCE row 63 (PIPE-11) builds that refusal and
  flips only 0055. The records follow the handoff: 0056 carries the middleware refusal, 0055 carries
  the error handlers' `Factory` arm (the grammar's completion, and PIPE-11's other half). Row 63
  accepting 0055 then leaves 0056's refusal clause built before 0056 is accepted at row 101.
- **`Custom { status, message, source }`:** §10 settled two fields; the third is the gRPC audit's D4
  (row 56 requires it). 0055 states the reason for the slot.

- **`FRAMEWORK_GAPS.md`:** no entry closes; the records carry the decisions rows 27–30, 55–63 and
  101 execute.
- **Workspace `CLAUDE.md`:** the file header's "ADRs 0001–0051 in `docs/adr/`" becomes "0001–0051
  accepted, 0055–0056 proposed" (0052–0054 join at row 76).
- **`DOCS_AUDIT.md` §18:** nothing to record; a proposed ADR changes no page.

### Row 27 — PIPE-1 — PR #365 (`feat/run-an-enhancer-declaration-in-the-order-written`, base `docs/propose-the-pipelines-order-and-the-pre-dispatch-role`)

- **`FRAMEWORK_GAPS.md`:** **F121** closes whole (the entry deletes at row 134): one ordered vector
  per role, pinned by `enhancer_declaration_order.rs` on all four transports and falsified by
  sorting tokens first (the seven order tests fail). ADR-0055's grammar table says a closure reaches DI "through `ctx`"; a sync closure cannot await a resolution, so `declaration.rs` says "no" — the ADR's cell to be corrected when row 63 flips 0055. **F56** closes whole: the closure spelling reaches the
  `Factory` arm from the declaration site. Error handlers take two spellings until PIPE-11 (row 63)
  gives them the `Factory` arm; the closure is refused for them by a compile error pinned in the
  diagnostics suite.
- **Workspace `CLAUDE.md`:** the Execution-order row's F121 caveat is replaced by the written-order
  rule and the three spellings (applied).
- **`DOCS_AUDIT.md` §18:** one line — the enhancer pages (`request-lifecycle/guards.mdx`,
  `api-reference/macros.mdx`, `grpc/enhancers.mdx`, `microservices/rpc-controllers.mdx`) describe
  two spellings and no order rule; the closure spelling and "entries run in the order written" are
  new.

### Row 28 — PIPE-2 — PR #366 (`feat/build-each-guard-only-after-the-one-before-admits`, base `feat/run-an-enhancer-declaration-in-the-order-written`)

- **`FRAMEWORK_GAPS.md`:** **F122** closes whole (the entry deletes at row 134): guards built one at a
  time and interceptors after them on all four transports, pinned by `guards_build_in_order.rs`
  (red before: the refused HTTP call built `guard` and `interceptor`, the gRPC one `guard`).
  F248's guard half — a panic while *building* an execution-scoped guard escapes recovery — is not
  closed: `run_guards` builds outside `catch_async`, as before; the entry stays.
- **Workspace `CLAUDE.md`:** the Execution-order row gains "each guard is built only once the one
  before it admits, and interceptors only after every guard".
- **`DOCS_AUDIT.md` §18:** nothing to record; no page describes when enhancers are constructed.

### Row 29 — PIPE-3 — PR #367 (`docs/correct-the-stale-pipeline-texts`, base `feat/build-each-guard-only-after-the-one-before-admits`)

- **`FRAMEWORK_GAPS.md`:** **F132** closes whole (the entry deletes at row 134): (1) `Interceptor`'s
  `R` doc names the four result aliases; (2) `middleware_error.rs`'s header states the matched route
  and the downcast; (3) `#[use_guards]`'s order rules hold since row 27; (4) the diagnostic carries
  each role's article, pinned by a unit test that fails with the derived article.
- **Workspace `CLAUDE.md`:** no row falsified.
- **`DOCS_AUDIT.md` §18:** nothing to record.

### Row 30 — PIPE-4 — PR #368 (`refactor/one-resolved-shape-and-one-interceptor-walk`, base `docs/correct-the-stale-pipeline-texts`)

- **`FRAMEWORK_GAPS.md`:** **F128** closes (one `Resolved<T>`, the nine-argument constructors take
  it, `ResolvedGrpcEnhancers` wraps it); **F57** closes (tiers merged at create, dispatch reads one
  slice); **F129** closes (`closure_leaf`, gRPC on `through_interceptors`, one `safe_render` taking
  the fallback); **F60** closes with F129 (the chain built once, the render recovery once); **F74**
  closes (the aliases removed, 0 remaining by grep). All delete at row 134. `AUDIT3_DISPATCH.md`
  §8's `GuardEntry`/`InterceptorEntry` box can be ticked at the close. Row 27 already left
  `enhancer_vecs` defined nowhere, which spends §8's F58 box too.
- **Workspace `CLAUDE.md`:** no row names these internals.
- **`DOCS_AUDIT.md` §18:** nothing to record.
- **Tests:** none added; a refactor with no behaviour change beyond one log line. The full suite
  is the net.

### Row 12 — RPC-6 — PR #369 (`fix/bound-a-kafka-call-by-its-timeout`, base `refactor/one-resolved-shape-and-one-interceptor-walk`)

- **Halt 1's answer applied:** the client half only; the server's reply publish keeps its literal
  and moves to row 110.
- **`FRAMEWORK_GAPS.md`:** **F230**'s client clause closes: every client publish runs under the
  caller's timeout, delivery included, and a `send` takes one deadline for its publish and reply.
  The fix goes past the entry's candidate (`Timeout::After(self.timeout)`), since rdkafka applies
  that to queueing only, so the delivery future is wrapped in the deadline too. F230 stays open on
  its server clause until row 110. **F252** filed: the first call on a client nobody called
  `connect()` on waits on topic creation under rdkafka's admin timeout, outside the call's.
- **Workspace `CLAUDE.md`:** the RPC section's shared "5s client timeout via `with_timeout`" holds,
  and no kafka row names the literal.
- **`DOCS_AUDIT.md` §18:** nothing to record.
- **Tests:** `tests/call_timeout.rs` (integration): `send`, `emit` and a stream's opening each
  answer `Timeout` in under 1.5 s against 500 ms with the broker stopped; against the base each ran
  to the 10 s give-up.

### Row 19 — GRPC-6 — PR #370 (`feat/carry-grpc-metadata-whole-and-status-details`, base `fix/bound-a-kafka-call-by-its-timeout`)

- **Halt 2's answer applied:** the trailer is a `google.rpc.Status` from `tonic-types`, with
  `details()` as one `Any`. A JSON object packs as a `google.protobuf.Struct`, as decided. A
  non-object value, which a `Struct` cannot hold, packs as a `google.protobuf.Value`: a case the
  answer did not cover, filled with the well-known type for any JSON value and stated in the PR.
- **`FRAMEWORK_GAPS.md`:** **F237** closes on its request side (every ASCII value in order,
  `-bin` keys decoded, for guards and handlers alike; `headers_all`/`header_bin`). Its reply-side
  clauses stay open: `set_header` refusing a `-bin` key and screening no `grpc-` prefix. **F241**
  closes whole: #358 stated the limit, and this row removes it. **Filed at once:** **F253** (the
  gRPC README names `grpc_code`, which does not exist).
- **Workspace `CLAUDE.md`:** in "Metadata vs headers", `` `RpcContext`/`GrpcContext`
  `headers()`/`header(k)` (read-only) `` becomes `` `RpcContext`/`GrpcContext` `headers()`/`header(k)`
  (read-only; on gRPC the last value, with `headers_all(k)` for every value of a repeated key and
  `header_bin(k)` for a `-bin` key's bytes, split on `,` before decoding; built by
  `GrpcContext::from_wire`; `deadline()` reads the first `grpc-timeout`, as tonic does) ``. In ulo-grpc's
  **Errors**, append: "An error's `details()` travels in `grpc-status-details-bin` as a
  `google.rpc.Status` (`tonic-types`) repeating code and message, the detail one `Any` — a
  `google.protobuf.Struct` for a JSON object, a `google.protobuf.Value` otherwise; no detail, no
  trailer, nor does an `Ok` code. A failing stream item goes through `to_status` too."
- **`DOCS_AUDIT.md` §18:** `- **#370 — gRPC metadata whole; details() in grpc-status-details-bin**
  (merged <date>). `grpc/error-handling.mdx` gains the trailer (supersedes #358's owed line, which
  said detail is dropped); the gRPC pages reading metadata gain `headers_all`/`header_bin`.`
- **Coverage ledger:** `ulo-grpc` gains `unit: true` (its first `#[cfg(test)]`: the comma split
  and the `Ok`-code guard).
- **Tests:** `grpc_metadata_and_status_details.rs`, six cases over a real call. With the detail
  left out of the status, the object, array and stream-item cases fail; with the `Ok` filter
  removed, its unit test fails. The metadata cases name API absent on the base, and the base macro
  dropped every `-bin` key.

### Row 31 — DI-ADR — PR #372 (`docs/propose-the-two-di-records`, base `master`, opens Wave 2's stack)

- **ADRs:** 0057 (*the container answers an ambiguous question, or refuses it*: D1–D3) and 0058 (*a
  provider declaration is a value*: D4, D8–D12), both `proposed`. 0058 also carries D5 (the module
  keys parse expressions, inseparable from D12) and D13 (no compatibility shims), beyond the
  handoff's list. On the user's ruling that a record states what the build does (2026-09-26), 0058
  also carries D14's `Extension<T>` rule (row 51) and F158's `#[default]` refusal (row 36), and 0057
  names `APP_INTERCEPTOR` a multi token beside `APP_GUARD`, the scanner registering both the same
  way; row 38 makes both multi. The pointer under ADR-0029 is added when 0057 flips at row 40.
  After Halt 4 (2026-09-28) both records were rewritten on the same PR: 0058 declares providers
  through one `provide!` over the value API with collections keyed by element type, and 0057 gains
  the refusal of a key bound both ways and the `APP_*` every-transport sets.
- **`FRAMEWORK_GAPS.md`:** nothing closes.
- **Workspace `CLAUDE.md`:** the workspace-layout line's "0055–0056 proposed" becomes "0055–0058
  proposed".
- **`DOCS_AUDIT.md` §18:** nothing to record; a proposed record changes no page.

### Row 32 — DI-1 — PR #373 (`feat/declare-a-provider-as-a-value`, base `docs/propose-the-two-di-records`)

*Rebuilt 2026-09-28 on ADR-0059 (Halt 6). This packet replaces the first build's.*

- **Halts 3, 4 and 6 applied:** every key is a type: `Key`, `key!`, `#[derive(Key)]`, and the
  reflexive impls `#[injectable]`, `#[controller]` and `#[websocket_gateway]` emit. The value API
  over them: `Provide::{value, value_key, factory, factory_key, alias, alias_key, into, into_key}`,
  `T::provide()`, and the `Declaration` modifiers `under_key`, `under_key_with`, `under_with`,
  `into_with`, `into_key_with`. The `_key` lookups on the app, the context and `ModuleRef`.
  `provide!` reading types, a marker key through the `ulo::__di` autoref probe. In-row choices, none
  moving the plan: a slot holding the declared type hands out a clone and one holding a trait object
  `Arc<dyn Trait>`, until row 50 makes a shared slot `Arc`; `PerExecution` admits the guard and
  interceptor trait objects only; the comma grammar stays until row 33; the HTTP guard and
  interceptor collections feed today's global path and other role collections are refused at
  `create` until row 55; `APP_GUARD`/`APP_INTERCEPTOR` untouched here.
- **`FRAMEWORK_GAPS.md`:** nothing closes; row 33 closes F161, F162, F143, F148 and F50. F153 (a
  controller cannot inject a collection) is confirmed from the new surface; row 42 carries it. Filed
  at once: **F254** and **F255** by the first build; **F259** (no lookup reaches a slot holding a
  trait object; trigger row 50); **F260** (a single binding and a collection under one `dyn T`
  collide; trigger row 38) and **F261** (two gateways at one path replace each other; a rebound
  gateway is two) from the Law's review. **F258** amended: the rebuild closes its keyless half.
- **ADRs corrected in place:** ADR-0059's built-without list gains `Of<T>`, and `Holds` and
  `HoldsPerExecution` are stated as what they became (the type equality or the cast; `PerExecution`).
  ADR-0058's collection paragraph says the macro passes the cast beside the declaration.
- **Workspace `CLAUDE.md`:** the ulo-macros *Exports* row gains `derive(Key)` and the new `provide!`
  grammar; the *Modules & DI* row gains `Key`/`key!`, `Provide`, the `Declaration` modifiers, the
  `_key` lookups and injection by element type; applied at the ceremony.
- **`DOCS_AUDIT.md` §18:** `- **#373 — every key a type; providers declared through
  \`provide!(key => source)\`** (merged <date>). Every page showing \`provide!\`, a
  \`provider_*!\` macro, a string key or a \`Token\` gains the typed form; row 33 deletes the old
  ones.`
- **Tests:** `provide_values.rs` (sixteen cases), seven diagnostics snapshots, four unit tests of
  the grammar; each behaviour red under a mutation (the probe always casting, no role from the slot,
  a rebound type dropping its roles or taking the slot's, a rebound singleton converted at every
  resolution, a trait-object marker binding under its `Value`, the per-execution cache taken away,
  a transient cached per execution, a rebinding forwarding no hook). Two Law passes.

### Row 33 — DI-2 — PR #374 (`refactor/declare-every-provider-through-provide`, base `feat/declare-a-provider-as-a-value`)

*Rebuilt 2026-09-28 on ADR-0059 (Halt 6). This packet replaces the first build's.*

- **Halts 4, 5 and 6 applied:** every call site on `provide!(key => source)`, a string key becoming a
  marker; `provider_value!`, `provider_factory!`, `provider_alias!`, `provider_token!` and the comma
  grammar deleted, with `TokenType`; `#[inject(K)]` reads a key type and checks what its field or
  parameter holds against `K::Value` at the key (`ulo::__di::inject_key`, bound `K: Key<Value = V>`),
  a string refused naming `key!`; the three `#[use_*]` attributes read a bare path of any length,
  generic or not (a type reading wins where it covers more of the argument), as a type,
  `value X` / `value &STATIC` for a held value, and refuse a string naming the role trait; `Token<T>`, `IntoToken`, `APP_GUARD`, `APP_INTERCEPTOR`,
  `APP_MIDDLEWARE`, the `_by_token` lookups and `DynamicModuleBuilder::export_token` deleted,
  `export::<T>()` taking `T: ?Sized`; `__ulo_provider_factory` and `__ulo_controller_factory` leave
  the public surface, `#[module]` reaching both traits. In-row choices, none moving the plan: the
  six database integrations' named constructors become `for_root_keyed::<K>` / `<backend>_keyed::<K>`
  (prisma's reading its client type off the marker), with the module identity base
  `token_of::<K>()`; `APP_GUARD` and `APP_INTERCEPTOR` go with `Token<T>`, their call sites moving to
  the HTTP collections they already equalled, and row 55 builds the every-transport markers, its table row carrying them; the
  GraphQL service is keyed by its own type, F191's candidate fix, because `#[inject("GraphQLService")]`
  no longer parses, and GQL-4's restructuring stands; `Extension<T>` implements `DeclaresProvider`,
  so row 34's `Extension<T>` clause is done; `provider_macro_role_detection.rs` goes and
  `app_token_enhancers.rs` becomes `global_collection_enhancers.rs`; the TCP and UDP client
  transports' docs attach to their structs again, a private `type Slot` having taken them.
- **`FRAMEWORK_GAPS.md`:** closes F161, F162, F148, F50, F139, F143 (every key position reads a
  type, and `Token<T>` is gone) and F133 (`APP_MIDDLEWARE` deleted); delete the seven entries.
  Filed at once: **F262** (a `#[new]` parameter of type `Vec<Arc<dyn T>>` panics out of `create`). F134
  amended: the `APP_*` route is gone, and the HTTP collections are the DI route to a global. F151(b)
  is spent with the `APP_*` consts. F191 amended: the service is keyed by its type. F256's candidate
  fix is restated against markers.
- **Workspace `CLAUDE.md`:** the ulo-macros *Exports* row loses the four `provider_*!` macros; the
  `provide!` row becomes the `provide!(key => source)` grammar over markers; *Modules & DI* loses
  `provider_token!`, `Token<T>`, `IntoToken`, `get_by_token`/`resolve_by_token` and `export_token`,
  and gains `#[inject(K)]` checked against `K::Value`; *Execution order* reads a bare path as a type
  with `value X`/`value &STATIC`; *Named connections* becomes `for_root_keyed::<K>` with
  `#[inject(K)]`; the gRPC *Client* paragraph's `provider_factory!` becomes `provide!(async || ..)`;
  the GraphQL row gains the service keyed by type.
- **`DOCS_AUDIT.md` §18:** `- **#374 — every key a type: the \`provider_*!\` macros, string keys
  and \`Token<T>\` deleted** (merged <date>). Every page showing \`provider_value!\`,
  \`provider_factory!\`, \`provider_alias!\`, \`provider_token!\`, \`provide!(TOKEN, ..)\`,
  \`#[inject("..")]\`, a \`Token\` const, \`get_by_token\`, \`export_token\`, \`APP_GUARD\`,
  \`for_root_named\` or \`#[inject("GraphQLService")]\` is rewritten with markers; §17 A8 and B13
  are spent.`
- **Tests:** `key_spellings.rs` (five cases, every accepted spelling end to end); four diagnostics
  cases (a field the key does not hold, a key that is not a `Key`, a string in `#[inject]`, a string
  in `#[use_guards]`); unit tests of the `#[use_*]` grammar; `di_providers.rs`'s value and factory
  cases read back what they declare; the migrated suites pin the keyed constructors, the marker
  lookups and the HTTP global collections. Each behaviour red under a mutation. Two Law passes.


### Row 34 — DI-3 — PR #375 (`feat/one-module-grammar-for-attribute-and-builder`, base `refactor/declare-every-provider-through-provide`)

*Built 2026-09-29 on ADR-0059 (Halt 6), the alias decision folded into row 32's grammar.*

- **ADR-0058 applied:** `controllers:` parses expressions, a bare path of any length being the type's
  own dispatch target through `DeclaresController`, which refuses a provider naming `providers:`;
  `exports:` lists key types (a generic with or without a turbofish, `dyn Trait`), a string refused
  naming `key!`; `DynamicModuleBuilder` has one `.provider(value)` and one `.controller(value)`,
  `provider_factory`, `provider::<T>()`, `controller::<T>()` and `controller_factory` deleted, every
  integration's module builder migrated. In-row choices, none moving the plan: `provide!` refuses a
  string key in the same words, so every key position does; `#[module]` gains its first doc, and
  `provide!` and `#[injectable]` carry the usage matrices the user asked for; `providers:` keeps its
  expression grammar, a generic there written with a turbofish.
- **`FRAMEWORK_GAPS.md`:** closes F144 (the four dialects; its `Extension<T>` half spent by row 33);
  delete the entry. C18 (the `controllers:` diagnostic) is answered by `DeclaresController`'s
  `on_unimplemented`.
- **Workspace `CLAUDE.md`:** *Modules & DI*'s `DynamicModule::builder(id).provider(f).export::<T>()`
  line reads `.provider(value)` / `.controller(value)`, and `#[module(..)]`'s row gains `exports:` as
  key types and `controllers:` as expressions.
- **`DOCS_AUDIT.md` §18:** `- **#375 — \`#[module]\` and the builder take the same values**
  (merged <date>). Every page showing \`.provider_factory(..)\`, \`.provider::<T>()\`,
  \`.controller::<T>()\` or \`.controller_factory(..)\` on the builder is rewritten; \`exports:\`
  gains generic, qualified and \`dyn Trait\` types.`
- **Tests:** `module_grammar.rs` (two cases), three diagnostics cases; red under a mutation (a fixed
  export key).

### Row 35 — DI-4 — PR #376 (`refactor/new-is-the-only-constructor`, base `feat/one-module-grammar-for-attribute-and-builder`)

- **As planned:** `init = "…"` deleted from `#[injectable]`, `#[controller]`'s refusal deleted with
  its parser's `init` arm, both `from_request` branches deleted; no migration error (D13). In-row:
  `ProviderStructArgs` and `ControllerStructArgs` (dead parsers of the key) deleted;
  `DependencyInfo::constructor_params` (never filled) and `DependencySource::Constructor` (never
  built) deleted with their readers; the three inline copies of the struct literal call
  `struct_instantiation`. Emitted code unchanged.
- **Verification drift:** the handoff's PR-4 test ("a provider whose init method is named
  `from_request` compiles like any other") lost its premise with the key: a `#[new]` method's name
  was never read, so the case passes at the base too (the Law probed it). The refusal case is the
  guard instead.
- **`FRAMEWORK_GAPS.md`:** closes F145, F156 and F157; delete the three entries. F159 loses three of
  its four cases (`init = "does_not_exist"`, `#[new]` beside `init`, `init = "from_request"`); amend
  it to the two-`#[new]` case, which row 36 carries. F263 filed (dead `DependencySource`/`source`/
  `unique_types`, four stale comments), pointed at rows 50 and 52.
- **Workspace `CLAUDE.md`:** *Declaration forms*' Scopes bullet drops `, init = "…"` from
  `#[injectable(scope = …)]`.
- **`DOCS_AUDIT.md` §18:** `- **#376 — \`#[new]\` is the only constructor** (merged <date>).
  \`api-reference/macros.mdx\` shows \`#[injectable(init = "create")]\` and says \`#[controller]\`
  rejects \`init = "…"\`; both attributes now report it as an unknown key.`
- **Tests:** one diagnostics case (`construction_takes_no_init`, both attributes), red when
  `#[injectable]` accepts `init` again.

### Row 36 — DI-5 — PR #377 (`feat/refuse-default-beside-new`, base `refactor/new-is-the-only-constructor`)

- **As planned:** `#[default]` beside `#[new]` fails to compile naming the field and both ways out,
  from `#[injectable]`, `#[controller]` and `#[websocket_gateway]` alike (one shared emitter, a const
  panic spanned at the attribute's path). In-row, reasoned by Fable: `#[new]` emits one fixed-name
  item, `__ULO_ONE_NEW_PER_TYPE: Option<Ctor<Self>>`, in place of the two bridge fns, so the flag the
  check reads and the constructor the factories call are one value, and a second `#[new]` fails as
  one E0592 at both attributes. ADR-0001's mechanism steps name the const. The eight test structs
  with an inert `#[default(0)]` beside `#[new]` lose it.
- **`FRAMEWORK_GAPS.md`:** closes F158 and F159 (its last case, two `#[new]`, now one error on a name
  stating the rule); delete both entries. F264 (a `#[new]` in another module is ignored) and F265
  (`#[inject]` beside `#[new]` accepted and inert, a design question) filed.
- **Workspace `CLAUDE.md`:** *Declaration forms*' code block carries `#[default(1000)] max` beside
  `#[new] pub fn new(dep: Dep)`, which now fails to compile: make the impl `impl X { … }` and add a
  bullet — construction is field injection, or `#[new] fn name(deps…) -> Self` on the impl (a bare
  `new()` is not auto-detected), which sets every field; a `#[default]` field beside it fails to
  compile, and a second `#[new]` on one type fails on `__ULO_ONE_NEW_PER_TYPE`.
- **`DOCS_AUDIT.md` §18:** `- **#377 — \`#[default]\` beside \`#[new]\` is refused** (merged
  <date>). \`api-reference/macros.mdx\`'s field-attributes section and \`### #[new]\`, and
  \`core-concepts/services.mdx\`'s "Constructor injection with \`#[new]\`", need the refusal and the
  one-\`#[new]\`-per-type rule.`
- **Tests:** two diagnostics cases (`default_beside_new` on the three struct macros, `new_twice`),
  both compiling when the const's name varies per method, the first when the check never fires;
  `owned_fields.rs` gains a controller and a gateway with `#[default]` and no `#[new]`, reading each
  default back (the controller the only one in the tree; its test reads `0` when the controller's
  field injection drops the default).

### Row 37 — DI-6 — PR #378 (`feat/refuse-a-hook-that-cannot-fire`, base `feat/refuse-default-beside-new`)

- **As planned:** a hook on an execution-scoped or transient `#[injectable]` fails to compile naming
  the hook and the scope; a per-call target's init/bootstrap `Err` fails the call. In-row, reasoned
  by Fable: the refusal reads inherent `__ULO_LC_*` flags the hook macros emit over a `LifecycleFlags`
  blanket with no `Sync` bound (row 36's const mechanism), and the unreachable `reject_lifecycle_hooks` path with the dead `LifecycleHooks`
  plumbing in the provider generators is deleted rather than made reachable; the call failure is a
  new framework event, `ulo::errors::HookFailed { target, hook, source }` (`Internal`, the message
  naming the hook alone, target and source in the fields and logged), returned by `DispatchSource::resolve` and
  `RpcControllerSource::resolve` (both now `Result`), nothing cached on failure. RPC's resolve moves
  inside the leaf's panic recovery. The shutdown hooks' docs state they never run on a per-call
  target (C17's asymmetry, kept). `#[catch]`'s doc example corrected to `value render_4xx`.
- **`FRAMEWORK_GAPS.md` / `DOCS_AUDIT.md`:** closes A9 and C17's discarded-`Err` half; C17's shutdown half is stated in the
  hook docs and stays open as F267 (DOCS_AUDIT statuses at the next audit). F266 filed (a bare `#[catch]` name fails startup with advice that does not fit; it predates
  row 33), F267 filed (a shutdown hook on an explicit `#[controller(scope = "execution")]` compiles and
  never runs), F264 amended (a hook in a child module's `impl` never runs on a singleton, predating the stack,
  and escapes the new refusal on an execution-scoped type).
- **Workspace `CLAUDE.md`:** *Execution-model DI*'s last sentence ("Lifecycle hooks on an execution-
  or transient-scoped plain provider COMPILE and are silently never called — the rejection is
  unreachable (§20 A9)…") reads that such a hook fails to compile naming the hook and the scope.
  *Declaration forms*' Scopes bullet: a per-call target's init/bootstrap `Err` fails the call as
  `HookFailed`, and its shutdown hooks never run. *Errors*: `HookFailed` joins the framework events.
  *ulo-macros* Hooks bullet: `Err` from an init hook on a per-call target fails the call.
- **`DOCS_AUDIT.md` §18:** `- **#378 — a hook that cannot fire is refused; a per-call hook \`Err\`
  fails the call** (merged <date>). \`core-concepts/lifecycle-hooks.mdx\`'s warn Callout and
  \`core-concepts/services.mdx\`'s scope Callout describe a compile error now, and
  \`api-reference/macros.mdx\`'s hook target line covers all five hooks; \`lifecycle-hooks.mdx\`'s
  per-call controller paragraph says an init \`Err\` is discarded, where it fails the call as
  \`HookFailed\`; the framework-event tables in \`request-lifecycle/error-handling.mdx\` and
  \`api-reference/core.mdx\` gain \`HookFailed\`.`
- **Tests:** `hook_on_a_scope_that_never_fires` (both scopes; compiles with the check removed),
  `a_hook_free_transient_that_is_not_sync_builds` (fails to compile once the flags carry a `Sync`
  bound, the regression the review caught); `per_call_hook_failure.rs` (six cases: HTTP unclaimed,
  claimed, refused and recovering; RPC claimed and panicking; gRPC unclaimed; four fail when the
  provider discards the hook result, the panic case when RPC resolves outside recovery).

### Row 38 — DI-7 — PR #379 (`feat/refuse-a-key-bound-twice`, base `feat/refuse-a-hook-that-cannot-fire`)

- **As planned, on the ADR-0059 model:** a second single binding under one key in a module fails
  `create` naming the key, the module and both entries by position; a key bound in any module
  and collected with `into` in any module fails `create` naming both modules (the check runs once
  the scan has every module, collections being registered by key across modules). "`APP_GUARD`
  becomes a multi token; `APP_INTERCEPTOR` with it" was met when row 33 moved their call sites onto
  the HTTP collections, which collect every contribution; this row pins two contributions running,
  in one module in declaration order and across two modules. The every-transport markers stay with
  row 55, whose table row carries them; row 33's packet line saying row 38 builds them is corrected.
  In-row: F31 closes in `ulo-db-prisma` itself, each `for_root` / `for_root_keyed` call folding a
  process-wide registration number into its module identity (`identity_hint`), so two collide on the
  global export (ADR-0029's refusal) instead of one being dropped as a repeat, and two client types
  register side by side (at the base one never registered). The scan skips a module it has already
  scanned, so a module reached through two import paths builds its imports once (the review found
  the per-call identity refusing a diamond import that starts at the base).
- **`FRAMEWORK_GAPS.md`:** closes F138's one-module half (its `APP_GUARD` half spent by row 33; the
  cross-module half moved into F142 as its fourth site), F31 and F260; delete the three entries.
  F268 filed (a collection field built before a contributing module drops its items).
- **Workspace `CLAUDE.md`:** *Modules & DI*: a second binding under one key in a module, and a key
  bound and collected, fail `create` naming both; *Database* row's Prisma sentence becomes: no check,
  no shutdown hook; each call carries its own `identity_hint`, so a second unkeyed `for_root` of one
  type, in the same module or another, fails startup naming both modules; two client types coexist;
  a second client of one type takes `for_root_keyed`.
- **`DOCS_AUDIT.md` §18:** `- **#379 — a key bound twice is refused** (merged <date>). Pages on
  providers and modules gain the two startup refusals; \`database/prisma.mdx\`'s "a name is
  required" sentence describes a startup refusal now, and its claim that two client types collapse
  into one module no longer holds; \`core-concepts/modules.mdx\`'s two-feature-modules dedup holds for
  the URL-configured integrations, not Prisma, where each \`for_root\` call is a registration and
  two are refused.`
- **Tests:** `a_key_bound_twice.rs` (five cases: two refusals of a key bound both ways, one of a key
  bound twice with the single-binding control, two guard-collection cases), Prisma's
  `two_unnamed_clients_of_one_type_are_refused`, `two_clients_under_one_marker_are_refused`,
  `two_client_types_register_side_by_side`, `a_module_reached_through_two_paths_registers_one_client`;
  red with each check removed, the registration number left out of the identity, and the scan's
  skip removed.

### Row 39 — DI-8 — PR #380 (`feat/fire-hooks-in-construction-order`, base `feat/refuse-a-key-bound-twice`)

- **As planned:** the container's module map and each module's provider, provider-instance and
  controller maps are `IndexMap<_, _, FxBuildHasher>` (`indexmap` 2 added to `ulo`, already in the
  lockfile transitively); the loader keeps build order in its instance map and records the order
  modules finish; every startup hook phase walks that order, every shutdown phase walks it back.
  In-row, from the review: controllers' hooks run after every provider's and stop before them
  (controllers are built after every provider; run with their own module's, one fired ahead of a
  global provider it injects); the scanner pushes sibling imports reversed so they register contributions and guards in declaration order,
  and build and fire in it at one import depth (F270 filed for uneven depth) (the ordered map otherwise kept the stack's last-declared-first order and
  flipped two imported guards); a module's own hooks keep their phase ahead of its providers' at
  shutdown as at startup, written into ADR-0057 (the loader builds no module).
- **`FRAMEWORK_GAPS.md`:** closes F140; delete the entry. F269 filed (a `macro_rules!` `ty` fragment
  as an injected field's type is refused), found writing the tests; F270 filed (sibling modules at
  different import depths fire out of declaration order), found in review.
- **Workspace `CLAUDE.md`:** *ulo-macros* Hooks bullet: hooks fire in construction order: a
  provider's after those of every provider it injects, providers declared in dependency order in
  declaration order, sibling imports at one import depth in declaration order, controllers after every provider; each
  shutdown phase runs the reverse; a module's own hook runs ahead of its providers' in every phase,
  shutdown included.
- **`DOCS_AUDIT.md` §18:** `- **#380 — lifecycle hooks fire in construction order** (merged
  <date>). \`core-concepts/lifecycle-hooks.mdx\` gains the order contract (controllers after every provider,
  a module's own hooks first in every phase) and the reversed shutdown.`
- **Tests:** `hook_order.rs` (six cases), `a_key_bound_twice.rs`'s two-module case asserting import
  order; red when the loader builds in token order, when the walk runs against declaration order,
  when shutdown walks forwards, when controllers run ahead of providers, and when sibling imports are
  pushed unreversed.

### Row 40 — DI-9 — PR #381 (`feat/narrow-the-widening-lookup`, base `feat/fire-hooks-in-construction-order`)

- **As planned, renamed:** `.global()` becomes `.visible()` (D1 named it `.or_global()`; the name
  was revised with the reach, 2026-09-29, reasoned by Fable), reaching the current module, then what its
  imports export, then the global registry, the order an injection is resolved in (the `ModuleRef`
  store gains each module's imported exports and the registry beside its instances; a key two
  imports export answers `AmbiguousModule`); a
  cross-module search on the application context answers `AmbiguousModule` with every holder's key
  (its message widened to cover a token). **0057 flips to `accepted`**; ADR-0029 gains the pointer.
  In-row, a narrowing recorded in the ADR: the two-imports refusal sits at the loader's injection
  site, not at the import, because the documented two-path GraphQL mount exports one service type
  from two modules into one importer that injects neither (a scan-time refusal broke it and
  `module_lookup`'s three GraphQL cases).
- **`FRAMEWORK_GAPS.md`:** closes F141, and F142's three sites (the import walk, `ModuleRef`'s
  non-strict branch, the application context's search); delete both entries. F142's fourth site
  (the application-wide role registry) moved to F271, open.
- **Workspace `CLAUDE.md`:** *Execution-model DI*: `ModuleRef` lookups read the module, and
  `.visible()` then what its imports export and then the global registry, a key two imports export
  answering `AmbiguousModule`; `AmbiguousModule` also answers a cross-module search
  whose key two modules hold; two imports exporting a key a provider or controller injects fail
  `create`; the `ProviderNotFound` parenthetical reads "`module` is the module searched, or `None`
  for a search across every module; a `.visible()` miss names its own module"; "`ModuleRef` handle
  scoped to that module (strict resolution)" reads "`ModuleRef` handle scoped to that module: its
  own providers, or everything visible to it with `.visible()`".
- **`DOCS_AUDIT.md` §18:** `- **#381 — lookups narrowed, ambiguities refused** (merged <date>).
  \`api-reference/core.mdx\` (the \`ModuleRef\` query line) and \`core-concepts/dependency-injection.mdx\`
  (the cycle-breaking paragraph) show \`.global()\` widening to exported and global providers: it is
  \`.visible()\`, reaching what the module could inject: its imports' exports, then what the global
  modules export. \`dependency-injection.mdx\`'s "\`get\`
  searches every module" and \`api-reference/core.mdx\`'s \`app.get\`/\`ctx.get\` lines omit that a
  key two modules hold answers \`AmbiguousModule\`, and the ambiguity message the first page quotes is
  the old identity-only wording.`
- **Settled:** the §6 entry "how far `.or_global()` reaches" is answered (b) (revised from (a)),
  and the method is `.visible()`.
- **Tests:** `ambiguous_lookups.rs` (six cases with controls: `.visible()` reaching a global
  export and an import's export, and refusing a key two imports export, among them), each red with
  its check removed; `module_ref.rs`'s "strict" wording and names become the current module and
  `.visible()`.

### Row 41 — RPC-7 — PR #382 (`feat/refuse-a-pattern-declared-twice`, base `feat/narrow-the-widening-lookup`)

- **As planned:** `Container::add_rpc_controller` refuses a pattern another registered controller
  declares, naming the pattern and both controllers, at `create` (ADR-0029's position), on every
  transport before any bind, per-call controllers included. In-row, from the review: `#[patterns]`
  refuses a pattern repeated inside one impl at the second attribute (the later method was
  unreachable and NATS subscribed twice, F226's residue). The `#[message_pattern]` /
  `#[event_pattern]` docs state both.
- **`FRAMEWORK_GAPS.md`:** closes F226; delete the entry. F272 filed and corrected (both attributes'
  docs show a handler shape the macros no longer take); F273 filed (a controller's token is its bare
  struct name, so same-named controllers collide and escape the refusal); F274 filed (a NATS wildcard
  beside an exact pattern it matches runs every call twice); F4 amended (four HTTP adapters accept a
  duplicate route and serve one of the two, not by declaration order; axum panics at `into_lifecycle`);
  F275 filed (a pattern attribute whose argument is not a string literal is dropped).
- **Workspace `CLAUDE.md`:** *RPC transports*: two controllers declaring one pattern fail `create`
  naming both, and a pattern repeated in one `#[patterns]` impl fails to compile. *Declaration forms*'
  routing bullet ("routes duplicate up to slashes collapse to a bind error") is false per F4's
  amendment: no adapter refuses; axum panics out of `bind`.
- **`DOCS_AUDIT.md` §18:** `- **#382 — a pattern declared twice is refused** (merged <date>). The
  RPC controller page and \`api-reference/macros.mdx\`'s \`#[message_pattern]\` / \`#[event_pattern]\`
  section gain both refusals.`
- **Tests:** `rpc_pattern_declared_twice.rs` (one module, two modules, message against event, the
  distinct-pattern control); diagnostics `rpc_pattern_repeated`; red with each check removed.

### Row 42 — DI-10 — PR #383 (`fix/resolve-a-collection-in-every-resolver`, base `feat/refuse-a-pattern-declared-twice`)

- **As planned:** the `Vec<Arc<dyn Trait>>` branch is one helper, `collection_field_resolution`,
  called by the three provider field resolvers and `#[controller]`'s. In-row: `#[new]`'s
  `resolve_param` calls it too, closing F262, whose candidate fix named this extraction; the
  `resolve_fields` doc, the helper's doc and `provide_values.rs`'s note that a controller has no
  collection branch are corrected.
- **`FRAMEWORK_GAPS.md`:** closes F153 and F262; delete both entries. F268 amended (a `#[new]`
  collection parameter on a singleton shares the partial assembly); F276 filed (an `Arc<dyn T>`
  field under a key holding only a collection panics during `create`).
- **Workspace `CLAUDE.md`:** nothing states the controller limit; no row changes.
- **`DOCS_AUDIT.md` §18:** none; no docs page states the limit.
- **Tests:** `collection_in_a_controller.rs` (provider, singleton controller, per-call controller,
  `#[new]` parameter); red with the controller's branch removed and with `#[new]`'s removed.

### Row 43 — DI-11 — PR #384 (`refactor/one-field-resolver`, base `fix/resolve-a-collection-in-every-resolver`)

- **As planned:** `generate_field_resolutions` and `generate_factory_field_resolutions` are one
  `field_resolutions(dependencies, deps, ctx)`; `generate_create_field_resolutions` and the
  controller's `resolve_fields` stay separate, as F155's measurement said.
- **`FRAMEWORK_GAPS.md`:** closes F155; delete the entry. F154 amended: its locator named
  `generate_factory_field_resolutions`, removed here (and misattributed at `654c7183`, where the
  probed `provider_factory!` resolved in `factory_provider.rs:286`); it points at the value-API
  sites its addendum names.
- **Workspace `CLAUDE.md`, `DOCS_AUDIT.md` §18:** nothing.
- **Tests:** none added; the expansion is token-identical to what the two functions emitted
  (probed by the review across nine provider shapes).

### Row 44 — DI-12 — PR #385 (`feat/refuse-a-factory-on-a-per-call-dependency`, base `refactor/one-field-resolver`)

- **As planned (Halt 7's option 1):** `Provider::resolve` answers `ResolutionError` and
  `ProviderFactory::build` a `BuildResult<T>` (ADR-0045's alias shape, exported from `ulo::spi`); an
  execution-scoped provider answers `Execution::None` with `ExecutionRequired`, the refusal passes up
  through whatever injects it, and the loader reports a failed build as `StartupError::BuildFailed
  { module, token, source }`; the generated singleton scope check and `Execution::ensure_can_build`
  are deleted; a failed downcast in generated and value-API resolution answers `TypeMismatch`.
  In-row, reported to the user: `ControllerFactory::build` answers `BuildResult` too, since a
  singleton controller reaching an execution-scoped provider through a transient is built at startup
  the same way and would otherwise still panic out of `create`; and F277's fix, the three resolvers
  that built a per-call instance's transients outside its execution, without which the refusal fired,
  and read falsely, inside a live call. A `.per_execution()` declaration reached from a singleton other
  than as a direct field, which started with an uncached instance, is now refused. A stand-in
  `Under` over an execution-scoped provider renames its refusal to the slot.
  ADR-0025 gains a pointer under its title (its *Provider construction still panics* boundary is
  crossed); the five database modules' comments that `build` "returns the instance directly" are
  reworded.
- **`FRAMEWORK_GAPS.md`:** closes F154 — the direct and transitive cases, `#[new]` parameters, a
  value-API factory, a slot factory (`FactoryBinding`), a slot type under a singleton (`Recast`), a
  collection contribution of an execution-scoped type, and a singleton over `.per_execution()`. Its
  addendum's "a transient declared type is built once and frozen" describes a contribution, which
  `Declaration::into_with` documents as built once at startup; under a slot a transient type converts
  per resolution (`Recast`). Delete the entry. Closes F277 (filed and fixed here); delete it. F278
  filed (`ulo-ws-redis` panics in `build`), F279 filed (`Request` in a non-HTTP execution panics),
  F280 filed (a controller reaching one through a transient is refused where a direct one elevates;
  design question). F276 amended: the downcast is a `BuildFailed` carrying `TypeMismatch` now, not a
  panic; the field-naming refusal stays open.
- **`DOCS_AUDIT.md` §17:** A27's subject, `provider_factory!`'s scoped factory, left with row 33, and
  `.per_execution()` caches per execution (`Maker::make`); for the next audit to clear.
- **Workspace `CLAUDE.md`:**
  - *Bootstrap*: the `StartupError` list gains `BuildFailed { module, token, source }` (a provider's or
    controller's build failed, a singleton reaching an execution-scoped provider among them);
    "`From<Box<dyn Error + Send + Sync>>` into `Setup`" reads "`From<Box<dyn Error + Send + Sync>>`
    hands a boxed `StartupError` back as itself and wraps anything else in `Setup`"; "Four aliases"
    becomes five, `BuildResult<T>` (`ProviderFactory::build` and `ControllerFactory::build`, in
    `ulo::spi`) joining the list.
  - *Naming*: "`Provider::resolve(ctx)` (no parameter vector)" reads "`Provider::resolve(ctx)` (no
    parameter vector, answering `Result<Box<dyn Any + Send>, ResolutionError>`)".
  - *Execution-model DI*: "`ExecutionRequired { token }` (a refusal, not a panic)" reads
    "`ExecutionRequired { token }` (a refusal, not a panic, answered by the execution-scoped provider
    itself: it also reaches a transient resolved outside an execution, and is the source of the
    `BuildFailed` for a provider built at startup that reaches one)".
  - *Declaration forms*' Scopes bullet: "auto-elevate to execution scope when a dep is
    execution-scoped" reads "auto-elevate to execution scope when a declared dependency is
    execution-scoped; one reached through a transient refuses the build instead (F280)".
- **`DOCS_AUDIT.md` §18:** `- **#385 — a provider built at startup that reaches an execution-scoped
  one is refused** (merged <date>). \`core-concepts/provider-patterns.mdx\`'s hand-written provider
  sample shows \`build\` answering \`Injectable\` and \`resolve\` a bare \`Box<dyn Any + Send>\`: they
  answer \`BuildResult<Injectable>\` and \`Result<_, ResolutionError>\`. \`api-reference/core.mdx\`'s
  \`StartupError\` listing gains \`BuildFailed\`, and its prose on \`create\`'s failures gains the
  refusal; its line that the container refuses a singleton injecting \`Extension\`/\`Extensions\` at
  startup now holds as a \`BuildFailed\`.`
- **Tests:** `di_validation.rs`: the two refusals move from `should_panic` to `BuildFailed`; the
  transitive, `#[new]`, value-API factory and `.per_execution()`, keyed `.per_execution()` (named by
  its key), keyed transient (naming its dependency), slot factory, slot type, collection and
  controller cases; a transient resolved in and
  outside an execution; execution over execution; three per-call builds through a transient
  (controller, `#[new]`, guard), each red before its resolver's fix. Eleven single-site mutations
  each caught.
- **Added on the user's word (2026-09-29):** the five database health indicators answer a mistyped
  connection with `TypeMismatch` instead of `.expect` (two commits); each crate's `health.rs` pins it,
  red with `.expect` restored. `DispatchSource::resolve` keeps its panic: only the framework's own
  per-call provider answers it, and its error type is the call's `HookFailed`.

### Row 45 — DI-13 — PR #387 (`refactor/rename-injectable-to-registration`, base `feat/refuse-a-factory-on-a-per-call-dependency`)

- **As planned:** `ulo::spi::Injectable` becomes `Registration` with no alias, across the SPI, the
  value API, the loader, the macro expansions and every integration crate; the loader's and the
  generated factories' locals follow. **0058 flips to `accepted`**, saying what `build` builds (it
  answers `BuildResult<Registration>` since row 44). ADR-0002's detection bullet gains a pointer to
  0058's key rule for a factory value's roles. Clauses of 0058 built by later rows, which verify
  them: row 50 (`Arc<T>` for a shared injection), row 51 (`Extension<T>` with no declaration) and
  row 55 (the every-transport sets). In-row, from the review: three clauses wrong when written are
  corrected in place (a collection gathers every module's contributions, exports ignored; `#[new]`
  is the one source of dependencies only for a struct that has one; a `provide!` form builds what a
  public constructor builds, some through a doc-hidden shim), and the declaration rewrite is scoped
  to the tree, the docs site being the docs pass's; the Clone-detection doc in
  `instance_injection.rs`, orphaned above `add_inject_fields`, moves to its function with its
  mechanism corrected.
- **`FRAMEWORK_GAPS.md`:** nothing closes. F281 filed (a collection ignores exports; design
  question); F282 filed (a provider sees only the contributions built before it; probed).
- **Workspace `CLAUDE.md`:** *Module layout*'s `ulo::spi` list reads `Registration` for `Injectable`.
  The workspace-layout line reads "ADRs 0001–0051, 0057 and 0058 accepted, and 0055, 0056 and 0059
  proposed", after rows 31 and 40.
- **`DOCS_AUDIT.md` §18:** `- **#387 — \`Injectable\` renamed \`Registration\`** (merged <date>).
  \`core-concepts/provider-patterns.mdx\`'s hand-written provider sample names \`Injectable\`: it is
  \`Registration\`.`
- **Tests:** none added; the compiler checks the rename.

### Row 46 — DI-14 — PR #388 (`refactor/close-the-app-constructor-and-key-lookups-by-token`, base `refactor/rename-injectable-to-registration`)

- **As planned:** `UloApplication::new` is `pub(crate)`, as `UloApplicationContext::new` is; the
  `unreachable_pub` gate then reports the 58 container and module items the constructor held open
  (38 in `container.rs`, 20 in `module.rs`), and they become `pub(crate)`. `ModuleRef::get` and
  `ModuleRef::resolve` take their token from `token_of`. C20's GraphQL half stays with GQL-4.
- **`FRAMEWORK_GAPS.md`:** closes F147; delete the entry.
- **`DOCS_AUDIT.md` §17:** C19 no longer holds; for the next audit to clear. C20 stands.
- **Workspace `CLAUDE.md`, `DOCS_AUDIT.md` §18:** nothing; neither names the constructor.
- **Tests:** none added; the compiler checks the visibility, and the token is the same string.

### Row 47 — DI-15 — PR #389 (`feat/fold-a-startup-check-into-module-identity`, base `refactor/close-the-app-constructor-and-key-lookups-by-token`)

- **As planned:** a `CheckedModule`'s identity folds its check beside the inner module's
  fingerprint (`StartupCheck` implements `Hash`): identical imports dedup, and two differing only in
  their check are two modules, whose global exports clash and fail `create`. In-row:
  `provider_startup_check.rs`'s header, stale since row 44 (it said `build` cannot fail), is
  rewritten; row 44's sweep missed it.
- **`FRAMEWORK_GAPS.md`:** closes F152; delete the entry.
- **Workspace `CLAUDE.md`:** *Database / GraphQL / Config / Health / CLI*'s DB-modules bullet gains,
  after the `StartupCheck` sentence: "the check is part of the module's identity, so two imports of
  one connection differing only in their check are two modules, refused where their global exports
  clash".
- **`DOCS_AUDIT.md` §18:** `- **#389 — a startup check joins the module identity** (merged <date>).
  The five \`database/*.mdx\` pages' startup-check sections omit that importing one connection with
  and without \`without_startup_check()\` (or with two checks) makes two modules, refused as a global
  clash.`
- **Tests:** `provider_startup_check.rs`: identity equal for identical imports and different for a
  dropped or changed check; identical imports start; differing ones are refused; both red with the
  check left out of the identity.

### Row 48 — DI-17 — PR #390 (`fix/report-a-module-cycle-as-its-chain`, base `feat/fold-a-startup-check-into-module-identity`)

- **As planned:** a cycle inside one module is found with `find_dependency_cycle`, started at the
  provider declared first, and rendered by the one `cycle_message` the cross-module stall now uses
  too; the loader's `max_iterations` budget and its "timed out" error are deleted, the loop being
  bounded by one pass per module.
- **`FRAMEWORK_GAPS.md`:** closes F146 and F149; delete both entries.
- **Workspace `CLAUDE.md`:** *Modules & DI*'s "Cycle diagnostics: single-module `Circular dependency
  detected for provider: {token}`; cross-module cycles name the exact provider chain" reads "Cycle
  diagnostics: a provider cycle, in one module or across several, names every provider on it with
  its module, from the one declared first in a single module".
- **`DOCS_AUDIT.md` §18:** `- **#390 — a cycle inside one module names its chain** (merged <date>).
  \`core-concepts/dependency-injection.mdx\` quotes \`Circular dependency detected for provider:
  my_app::services::ServiceA\`, the old single-token message.`
- **Tests:** `circular_dependency.rs`: a cycle inside one module, red with the old message and with
  the chain started from the other provider.

### Row 49 — DI-16 — PR #391 (`refactor/hold-a-provider-in-one-arc`, base `fix/report-a-module-cycle-as-its-chain`)

- **As planned:** `Arc<dyn Provider>` and `Arc<dyn Gateway>` across the SPI (`Registration::instance`,
  the provider maps a controller factory takes, `ProviderRole::Gateway`), the container, the
  `ModuleRef` store, `DispatchSource::PerCall`, the macro expansions and the integration crates;
  a factory coerces at construction rather than boxing first (64 spellings).
- **`FRAMEWORK_GAPS.md`:** closes F150; delete the entry.
- **Workspace `CLAUDE.md`:** nothing names the nesting.
- **`DOCS_AUDIT.md` §18:** `- **#391 — a provider is held in one \`Arc\`** (merged <date>).
  \`core-concepts/provider-patterns.mdx\`'s hand-written provider sample builds
  \`Arc::new(Box::new(..))\`: it is \`Arc::new(..)\`.`
- **Tests:** none added; the compiler checks the types.

### Row 50 — DI-19 — PR #392 (`feat/share-an-injectable`, base `refactor/hold-a-provider-in-one-arc`)

- **As planned, on Halt 8's answer:** a singleton or execution-scoped `#[injectable]` or
  `#[websocket_gateway]`, a `provide!` value or factory and a `.per_execution()` factory answer one
  `Arc`; a transient answers its value; the macros' `#[derive(Clone)]` is deleted; a dependency on a
  shared binding is written `Arc<T>` (keyed by `T`, wrapping a by-value answer), a plain `T` reads a
  transient or a hand-written by-value provider; a factory takes `Arc<Dep>`; `get`/`resolve` and the
  `_key` forms return `Arc<T>`. No `Handle` marker, no `.handle()`, no desugar. The three refusals:
  compile time (an inherent `__ULO_SHARED` flag over the blanket `ulo::__di::SharedFlag`, read in a
  `const` at each plain field and inside `#[new]`'s tokens fn), `create` (`value_dependencies()` on
  `ProviderFactory` and `ControllerFactory`, `Ctor::values`, the loader reading `Provider::shape`),
  resolution (`take_value` answering `ResolutionError::SharedByValue { token, wrote }`). In-row, from
  the red check: fields over one token are grouped by written shape (commit A), and a plain field
  takes its own answer so its type needs no `Clone` (commit D). ADR-0058's injection clause corrected
  in place; ADR-0002 and ADR-0005 gain pointers to 0058 at their `Clone` clauses. From the Law:
  commit A is a break (`!`): an `Arc<T>` field no longer reads a slot bound to an `Arc<T>` value,
  and `#[inject(K)]` checks an `Arc<V>` field against `K::Value = V`; two doc comments claiming a
  trait-object slot and the type's own key hand out one object were corrected (`Recast` builds its
  own instance). On the user's word (2026-09-30), F283 is fixed in the row: a site's key, answer,
  value read and `#[inject(K)]` check are methods on `ulo::__di::Site<F>`, picked by autoref over the
  resolved type; a `Vec<Arc<T>>` whose `T` is sized, `str` or a slice is read by value, as before;
  and fields no longer group into one resolution by their written token, each resolving on its own
  (a shared binding hands every field the same `Arc`).
- **`FRAMEWORK_GAPS.md`:** closes **F283** (a site's key, answer, value read and `#[inject(K)]`
  check follow its type through `ulo::__di::Site`, so an alias or a renamed `Arc` reads its
  binding); delete the entry. **F263**: its `Clone`-detection clause is spent (the doc
  and `add_clone_and_inject_fields` are deleted); the rest stays for row 52. **F259** stays open, amended:
  the lookups return `Arc<K::Value>` since row 50 and keep the `Sized` bound, because only a sized
  answer handed out by value can be wrapped; a trait-object slot is still read by injection alone.
  **F256** stays open, amended: since row 50 a plugin injected directly and the same plugin reached
  through a collection built from its type are two instances, observably. **F255** stays open, amended: its alias clause (`type Log = Arc<dyn Logger>` keying `Arc<dyn
  Logger>`) is closed with F283;
  a factory parameter is `Arc<A>` keyed by `A`, and `Arc<dyn T>` is refused at compile time by the
  implicit `A: Sized` where it failed `create`, under the `IntoFactory` label "each an `Arc`", which
  the parameter already is (probed on the branch). **F7** (closed): the docs
  page it names carries the `Clone`-bound error, which no longer occurs; injecting a dispatch target
  fails `create` with `Dependency not found` (probed on the branch). Filed at once: **F283** (closed in
  the same PR, above) **F284** (the compile-time check reads the type, so a
  plain field under a marker bound to a transient is refused although it would resolve) and **F285**
  (a collection of a sized element type can be contributed and not injected; the collection rung
  was kept to trait objects, as the macros read it before).
- **Workspace `CLAUDE.md`:** *Declaration forms*' code block: `#[inject] dep: Arc<Dep>,` and
  `#[new] pub fn new(dep: Arc<Dep>) -> Self`, with a bullet: a singleton or execution-scoped
  provider hands out one `Arc`, which every holder shares, and a transient a fresh value; write
  `Arc<T>` for a dependency, which reads any binding, or a plain `T` for a transient or a handle a
  hand-written provider answers by value (`ConfigService`, `ModuleRef`, `Extension<T>`, the pools);
  a plain field of a singleton or execution-scoped `#[injectable]` type fails to compile, and one
  over a `provide!` value or factory fails `create` with `ResolutionError::SharedByValue`; the
  macros derive no `Clone`. *Execution-model DI*: `resolve::<T>` returns `Arc<T>`, and the
  `ExecutionCache` clause becomes "one instance per execution, handed out as one `Arc`
  (`insert` keeps the first)"; the `ResolutionError` list gains `SharedByValue { token, wrote }`.
  *ulo-macros*' `provide!` bullet: a factory closure takes `Arc<Dep>` parameters. *ulo-grpc*'s
  **Client**: the client is injected as `Arc<OrdersClient<Channel>>` and cloned out of the `Arc`
  for the `&mut` a call takes.
- **`DOCS_AUDIT.md` §18:** `- **#392 — an injectable is shared as one \`Arc\`** (merged <date>).
  Every page showing an \`#[inject]\` field or \`#[new]\` parameter of a service or a \`provide!\`
  value writes it plain: it is \`Arc<T>\`, plain only for a transient or a hand-written handle
  (\`ConfigService\`, \`ModuleRef\`, \`Extension<T>\`, the database pools). Factory closures take
  \`Arc<Dep>\`. \`get\`/\`resolve\` return \`Arc<T>\`. \`core-concepts/services.mdx\`'s
  \`Clone\`-bound error for an injected dispatch target no longer occurs (\`create\` reports
  \`Dependency not found\`), and any page describing execution-scoped instances as shared by clone,
  or the macro adding \`Clone\`, is stale. The refusals (\`SharedByValue\`, the compile-time
  message) are undocumented.`
- **Tests:** `shared_instances.rs` (eight: D14's three assertions in one, lookups, a factory
  parameter, one instance per execution from a `.per_execution()` factory, a hook's effect reaching
  a holder built before it, a trait-object slot handing out the instance its hook ran on, an
  injectable's own `impl Clone`, and the handoff's fourth: an injectable whose field is not
  `Clone`); the scope, lookup, per-execution, hook and trait-object cases
  fail against commit C's copying providers. `shared_by_value.rs` (four): the three `create` cases
  pass `create` at commit D, before the loader's check; the resolution-time case fails with the
  `Arc` check removed from `take_value`. `inject_a_shared_type_plain` (diagnostics) compiles at
  commit D, before the flag.
  `site_types.rs` (one): aliases and a renamed `Arc` read their bindings; fails with `Dependency
  not found: alloc::sync::Arc<…Store>` against the spelling-read macros (and to compile on the
  `#[inject(Limit)]` alias).

### Row 51 — DI-20 — PR #393 (`refactor/read-an-extension-from-the-bag`, base `feat/share-an-injectable`)

- **As planned:** `ExtensionFactory<T>` deleted, with `Extension<T>`'s own `Provider` impl and its
  `DeclaresProvider` impl; a site of `Extension<T>` reads the execution's bag through a rung of
  `ulo::__di::Site` (keyed by `Extensions`, the global slot, and viewed as `T`), so no module
  declares a view. In-row: the mechanism is the injection site rather than the container answering
  an `Extension<T>` key, which leaves nothing registered under `Extension<T>`; a lookup reads the bag
  with `resolve::<Extensions>`, and a singleton holding a view is refused naming `Extensions`.
  ADR-0058's clause ("the container answers any `Extension<T>` key") corrected in place to the
  injection site. A `providers: [Extension::<T>]` entry fails to compile on `DeclaresProvider`.
  **Halt 9:** a `provide!` factory parameter `Arc<Extension<T>>` and `resolve::<Extension<T>>` are
  not injection sites and fail (probed); built as option (a), awaiting the user's word.
- **`FRAMEWORK_GAPS.md`:** closes **F181**; delete the entry. **F160**: its `ExtensionFactory<T>`
  half is spent (the type is deleted); the entry stays for row 52's `ExtensionsFactory` rename,
  reworded to that one name. **F144** was closed by row 34's packet; nothing here reopens it.
- **Workspace `CLAUDE.md`:** *Execution-model DI*: "`Extension<T>` = execution-scoped DI view of the
  extension bag (HTTP; register per payload type)" becomes "`Extension<T>` = a view of the
  execution's extension bag, read by injection with no declaration; code outside the graph reads the
  bag through `resolve::<Extensions>`".
- **`DOCS_AUDIT.md` §18:** `- **#393 — an \`Extension<T>\` needs no declaration** (merged <date>).
  Any page registering \`providers: [Extension::<T>]\` (the execution-scoped-context and
  guards/extension pages) is stale: the entry fails to compile, and a view reads the bag with no
  declaration. A lookup of \`Extension<T>\` finds nothing; \`resolve::<Extensions>\` reads the bag.`
- **Tests:** `extension_di.rs` and `module_grammar.rs` lose their `Extension::<T>` declarations and
  pass (each fails at the parent with `Dependency not found: …Extension<…>`);
  `a_singleton_holding_a_view_is_refused_at_create` (fails when the bag's provider answers an empty
  bag outside an execution). The unit test pinning the registration token is deleted with the
  registration.
