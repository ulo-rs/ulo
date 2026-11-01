# Salvage: the root build sequence (`BUILD_SEQUENCE.md`)

**Subject.** `BUILD_SEQUENCE.md` as a document: the control surface written by the 2026-09-24 triage over
the eight September Smile audits against master `654c7183`. It carries the standing orders (§0), the
sign-offs S0-S13 (§1), the ordering calls (§2), 134 numbered rows in seven waves (§3), the per-row
loop (§4), the Law's brief (§5), the halt log (§6), the plan-text amendments (§7) and the merge
ceremony with one bookkeeping packet per opened PR (§8). The per-row *content* (what each fix does,
its probes, its value) belongs to the subject records (`di`, `rpc`, `ws`, `grpc`, `http`, `pipeline`,
`extraction`, `graphql`); this record gives each row a one-line disposition and owns the sequence's
own items.

**Sources read in full.** `salvage/source/BUILD_SEQUENCE.md` (28,685 words, 2,264 lines);
`salvage/source/README.md` (2,654 words) for the cross-subject couplings. Consulted, not read whole:
`FRAMEWORK_GAPS.md` (entry presence and tails), `DESIGN.md`, `transports/DESIGN.md`,
`BUILD_PLAN.md`, `SWITCH_INVENTORY.md`, `git log master`, the crates at HEAD.

**Tip and date.** `experiment/di-redesign` at `331c35b9`, 2026-11-01. Master at `861e9840`.

**Where the rows stand in git.** The numbers below are read from `git log master` and
`git merge-base HEAD master`.

| Rows | PRs | On master | In HEAD's tree |
| --- | --- | --- | --- |
| 1-30 (wave 1) | #341-#370 | merged as stack #371 in one merge commit `3cc36510` (2026-09-26) | `3cc36510` is the merge base of HEAD and master; every crate it touched has since been rewritten, so the code is in HEAD's history and not in its tree |
| 31-51 (wave 2, rows 31-51) | #372-#385, #387-#393; #386 (the ADR README) rides the stack and is not a row | merged 2026-11-01, `6199313d` (#372) through `861e9840` (#393); the SHAs per row are in the table in §3 | absent: HEAD was cut from `3cc36510` |
| 52-134 | none | never started | n/a |

The §3 state column in the source still reads `[p]` for rows 31-51 and `[p] #393 (draft, Halt 9)` for
row 51; the merge on master post-dates the document's last edit (the pause of 2026-10-01).

---

## 1. Summary

| Status | Rows (§3 below) | Sequence-level items (§4-§9) |
| --- | --- | --- |
| built (rows: incl. 6 built with a difference) | 63 | 30 |
| superseded | 56 | 35 |
| open | 9 | 7 |
| dropped (incl. spent) | 3 | 20 |
| UNVERIFIED | 3 | 2 |
| **total** | **134** | **94** |

Sequence-level items: 13 orders and method items (§2), 14 sign-offs (§3), 12 ordering calls (§4),
14 halt-log entries (§5), 7 plan-text discrepancies (§6), 8 ceremony items (§7), 26 couplings (§8).

**The five most valuable open items.**

1. **The ledger and the workspace `CLAUDE.md` do not describe master** (§8, item B-8). Master merged
   #372-#393 on 2026-11-01 without the §8 ceremony: no bookkeeping packet was applied. `FRAMEWORK_GAPS.md`
   still carries F138, F141, F146, F150, F153, F154, F155, F181 and F283 with no closing note, each of
   which a merged packet says to delete (rows 38, 40, 48, 49, 42, 44, 43, 51, 50). The wave-1 entries
   marked "deletes at row 134" (F121, F122, F128, F172, F173, F185, F193, F232, F235, F236, F239) are
   also still present. Not in the ledger as a finding.
2. **Per-call credit (rows 116-117, RPC D8)** is deferred at HEAD: `credit` is a reserved, unsent frame
   (`crates/ulo-rpc/src/frame.rs:15`; transports DESIGN §5.2 "Flow control", decision 15). The plan's
   design, which the rebuild does not carry, is in §3 row 116.
3. **graphql-sse (row 127, GQL-7)** has no binding at HEAD (no `text/event-stream` or `graphql-sse` in
   `crates/ulo-graphql*`). The row was the plan's falsifier for "one engine result, two bindings".
4. **A resolver is a handler (rows 128-129, GraphQL ADR B)** is not in the rebuild: resolvers are the
   engine's own, and the context is an `#[injectable(execution)]` `GqlContext` (transports DESIGN §7).
   Guards and interceptors do not reach a resolver.
5. **WebSocket `Origin` verification (row 92, F207)** has no built-in check at HEAD: `ulo-ws`'s
   handshake decision (`GatewayTable::handshake`, transports DESIGN X32) lists path, RFC 6455 §4.2.1
   checks, subprotocol, drain and the connection phase, and no `Origin` comparison
   (`grep -i origin crates/ulo-ws/src` finds only `original`). A connect guard reading
   `UpgradeHead` is the available route.

**Live defects at HEAD not held by the ledger.** None found in code by this record's checks. The
bookkeeping divergence in item 1 is a ledger defect, and it is not in the ledger.

---

## 2. The standing orders (§0) and the method (§4, §5)

Each order is the user's (§0 preamble: 2026-09-20, 2026-09-21/22, 2026-09-24). The question per order:
does anything of it survive in how the rebuild is run?

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| 0.1 | The plan is frozen; the build writes only code, tests, ADRs as instructed, the two ledgers, falsified `CLAUDE.md` rows, `DOCS_AUDIT.md` §18 and this file | superseded | `BUILD_PLAN.md` "Rules" 4-5: the spine's contracts and public signatures are frozen, and `DESIGN.md` is the spec with `RESPONSE.md` breaking ties | The split between a frozen spec and a mutable state file survives. The rule that status lives in one state column and never in the memos (0.1's last sentence) is worth keeping: it is what let §3 stay authoritative while eight memos stayed untouched. |
| 0.2 | Divergence halts, nothing else does; on divergence write a §6 entry with the plan's words, what the tree requires, options, a recommendation; push a draft PR; end the turn | built (transformed) | `BUILD_PLAN.md` Rules 3 and 6: every divergence is logged in `divergences/<area>.md`, "what `DESIGN.md` says (or that it is silent), what was written, and why"; `docs/experiments/di-redesign/divergences/` holds the logs | The enumerated divergence classes are the reusable part (below). The rebuild logs and continues; the old order stopped. The halt log shows why stopping paid: Halts 3, 5, 6 and 8 each found a plan clause the compiler refuses (autoref inside a generic, `Unsize`, the const/type namespace split, E0117/E0119), and building around any of them would have shipped a different design under the plan's name. |
| 0.3 | Not divergence: naming, file placement, test names, the order of two independent rows, splitting a row | superseded | Rebuild rule 6: "Internal-only choices need no entry unless they shape the public API" | The two lists together form a complete test: 0.2 says what stops, 0.3 says what never does. |
| 0.4 | No per-row go-ahead; a row whose `Needs` and `S` items are satisfied is authorised | superseded | The rebuild is run in races and batches under a coordinator (`BUILD_PLAN.md`, `BUILD_PLAN_2A.md`, `transports/BUILD_PLAN_2B.md`) | none beyond 0.11's resume test |
| 0.5 | The Law runs before every push and every PR open | dropped | No rebuild document names the Law (`grep -rlw Law docs/experiments/di-redesign` outside `salvage/` is empty); the rebuild's coordinator commits (`BUILD_PLAN.md` rule 7) | The brief (§5, row M-2 below) and the rulings it settled. The halt log records the Law finding defects, not only register: row 13's correlation-id collision across restarts, row 17's undisarmed timer, a reference cycle and a skipped release (three passes), row 14's `lifecycle` flag behind F225 (A8). |
| 0.6 | Stacking, literally; every row on the previous row's tip; a review fix on a lower PR rebases everything above and re-gates | superseded | Replaced on 2026-09-26 by "merging by wave" (§6, H-12); the rebuild is not PR-stacked | "A gate result belongs to the tree it ran on" (from `feedback-verify-the-tree-before-reading-it`) is the transferable rule. |
| 0.7 | Merge bottom-up with `--merge --delete-branch`, never `--squash`; confirm GitHub's retarget and the checks before the next | built on master | `git log master`: #372-#393 each landed as its own merge commit on 2026-11-01 | none |
| 0.8 | Verification: red first; plainest form; one `cargo test` at a time; every checker asserted against a known violation; counts from a command run after the last commit; rustc 1.98.1 before snapshots; re-check the plan's claims against master before a row starts | built (in part) | The red-first and known-violation discipline appears in 22 rebuild documents (`grep -rl 'known violation\|[Rr]ed first'`); `BUILD_PLAN.md` rule 1 forbade `cargo` during the spine race, so the rest applied later | The last clause is the one that caught the most: rows 35 and 43 found their handoff's test premise false at the base (row 35's `from_request` case passing before the change; row 43's locator misattributed at `654c7183`). |
| 0.9 | Bookkeeping: the untracked files describe master; each opened PR writes a packet into §8; the packet is applied after its PR merges | open | Packets for rows 31-51 were written (§8) and not applied when master merged them (§1 item 1) | The rule's own rationale ("so the three untracked files never describe a state master does not have") is the failure that occurred. The packets are a ready worklist: §8 rows 31-51 name each entry to delete, each surviving clause and its residual text, and each `CLAUDE.md` row's corrected text. |
| 0.10 | No `Co-Authored-By` or AI attribution | built | Global `CLAUDE.md` carries it | none |
| 0.11 | Resume protocol: read the file, fetch, reconcile §3 with open PRs, read §6, take the first ready row | dropped | The sequence is paused (§6 "Pause", 2026-10-01) and its stack is merged | The reconciliation step (§3 against `gh pr list`) is what would have caught item 1 at the next resume. |
| M-1 | The loop (§4): sync, read the row and every cited clause, branch named for the change, red first, build plus falsified production text, a twelve-command gate with every exit code read alone, split commits, the Law on commits, push, the Law on the PR, CI, tick and packet, review traffic | superseded | The rebuild's gate is CI's job list (`.github/workflows/ci.yml`: check, runtime levels, docs, test, smol, msrv matrix, macro diagnostics, generated code lints, public reach, miri, broker integration) | Two gate rules stand alone and hold for any build: "Read every count. `0 passed` where tests exist means the gate is broken, not the code"; and "`--all-targets` without `--all-features` is known not to build on master (F40, F89); the plain form that must pass is `cargo check --workspace`." Step 9: a PR body cites no untracked ledger and no `F<n>`, and spells out a count, because a bare `#<digits>` renders as a link. |
| M-2 | The Law's brief (§5): read the four rule files in full; report each violation with the rule and the clause and a correction; a word scan does not discharge it; judge relations, postures and audiences; a clean text says which tests it passed | dropped (method worth keeping) | Not used by the rebuild (0.5) | The brief as written (`BUILD_SEQUENCE.md` §5) is reusable verbatim. Per invocation it adds the branch and commit, an order to read the diff in full, a numbered list of the technical claims to verify hardest with cross-transport claims first ("every transport now does X" is a claim about seven crates), and the count of prior passes that found factual errors. Settled rulings: a PR body's opening may repeat a commit-body clause; an opening may carry what the PR adds and the problem together; a reference to an ADR the change revisits survives the cross-PR test; a PR body never cites an untracked ledger. Row 15 records a pass on Opus after a Fable rerun failed on credits, and that pass found F248. |

The divergence classes of 0.2, kept because they read across any plan-driven build: a type or
signature differing from what a memo or ADR writes; an alternative a memo rejected turning out to be
necessary; a variant, parameter or capability the design does not name; a finding that reorders,
removes or adds a row; a verification that cannot be written as described; a decision whose rejected
alternatives were not the only ones; one plan text contradicting another; and named tripwires (an
adapter whose upgrade does not work at the dispatch point; a capability type RPC cannot use; a socket
type losing `Send + 'static`; graphql-sse unable to answer both bindings from one taxonomy; gRPC's PR 3
needing a `ulo-build` API change or shipping only a diagnostic; a conformance case failing for a reason
unrelated to the PR).

---

## 3. The sign-offs S0-S13 (§1)

All fourteen were answered by the user on 2026-09-24 (§1 preamble).

| ID | Question | Decided | Still relevant to the rebuild? | Evidence at HEAD / value |
| --- | --- | --- | --- | --- |
| S0 | Does the build merge its own PRs with no per-PR review? | No: stack literally, the user reviews each PR as it opens and calls the ceremony. Reversed in practice on 2026-09-26 (H-12) | dropped | Process only. |
| S1 | The query-parsing crate | `serde_html_form` 0.4.1 (MSRV 1.63, a `serde_urlencoded` fork with sequence fields, built on `form_urlencoded`); six call sites in four files; one behaviour change, `n=` reads as `None` for an `Option` | superseded | HEAD writes its own deserializer for `Path<T>`, `Query<T>` and `Form<T>` over name/value pairs (`crates/ulo-http/src/extract/de.rs:1`); no manifest names `serde_html_form`, `serde_urlencoded` or `serde_qs`. Value: the five-site inventory (README "four query parsers is five"), and the `n=` behaviour, which a test of HEAD's deserializer can pin: does an empty numeric value read as `None` for `Option<u32>` or fail? |
| S2 | gRPC D3 (ii) beyond gRPC | Approved: three rows (131-133), one per transport; the RPC half filed as F245 | built | Transports DESIGN §2.6: `CancelReason::{Disconnected, ClientCancelled, Deadline, Drain, Explicit}` fired by every transport from its wire. |
| S3 | The `ErrorKind` word for three non-kind statuses | Approved: `overloaded` → `TooManyRequests` (gRPC parity: `RESOURCE_EXHAUSTED`), `unsupported` → `Unimplemented`, `error` → `Internal` | built differently | HEAD answers a shed load as `unavailable` with `RetryAfter` on RPC and 503/`UNAVAILABLE` elsewhere (transports DESIGN §12 "Over a load-shedding limit"). F63's own note had named `Unavailable` as the better fit; S3 chose the other. See §10 Q1. |
| S4 | HTTP-11's reach: `render_error` reachable on HTTP only, or HTTP, RPC and WebSocket | Approved: all three in one PR | superseded | One `CallError` rendered by every transport (transports DESIGN §2.4); there is no per-transport private renderer to expose. |
| S5 | F242's fix | Approved: `PanicRecovered::message()` answers the generic literal; the payload stays on the public field | built | `crates/ulo-transport/src/error.rs:135` (`PanicRecovered` → `Internal`, "with a generic message"), `:290`. |
| S6 | Amendments to ADRs 0052/0053 while proposed | Approved | dropped | ADRs 0052-0054 lived on `docs/http-audit-adrs` (`9d3fb9c2`, still in the object store); the branch is gone and neither master nor HEAD carries them. The amendment "an adapter never authors an error body" (F243, F244) is the rule HEAD keeps: hosts embed `ulo-http`'s service and do not render (transports DESIGN §3.3: "no middleware renders its own errors"; §3.7-§3.8). |
| S7 | WS-9's position | WS-9 directly after WS-7, before the cluster; WS-8 extends the suite after WS-21 | spent | Rows 1-2 merged in that order. |
| S8 | `HTTP-3` and `WS-21` as two PRs, one permitted | Approved | dropped | Neither row ran; the rebuild owns the WS loop in `ulo-ws` (`connection.rs`) and the hand-off in `ulo-http` (`upgrade.rs`). |
| S9 | RPC Phase 3 approval | Signed | spent | Phase 3's call shapes are built at HEAD (rows 119-122). |
| S10 | One capability ADR across HTTP, WS, RPC and the GraphQL engine SPI | Approved | superseded | No such record; each SPI declares its own: an HTTP host's `upgrades` and `host_extensions` (transports DESIGN X32, §3.8), an RPC link's `binary`, `max_frame` and delivery (`Competing`) (§5.2-§5.3), the GraphQL `Engine` (§7). Value: the README's coupling that one `#[non_exhaustive]` shape should serve four SPIs; HEAD has four shapes. See §10 Q6. |
| S11 | RPC D6 has no PR | Approved: row 11 | built | `.github/workflows/ci.yml:300-347`, the `broker integration` job, now runs all five broker suites on push, Kafka included; `Makefile` keeps `conformance-kafka`. |
| S12 | F120 (`app.routes()` as a read) | Approved: HTTP-2 ships it | UNVERIFIED | Not checked whether HEAD exposes the route table as a read. |
| S13 | PIPE-12's gRPC anchor | Above tonic's router | superseded | HEAD has `crates/ulo-grpc/src/pre_dispatch.rs` and no `add_service` bypass to cover; the router-anchor argument (GRPC_AUDIT §8 D2: a guard reading the decoded message cannot run above the codec, so the two-tier split is structural) belongs to the grpc and pipeline records. |

---

## 4. The ordering calls (§2) and their reasons

| ID | Call | Reason recorded | Status | Value to extract |
| --- | --- | --- | --- | --- |
| O-1 | Free-standing rows first (wave 1) | Each marked independent by its handoff; they close MUST-class and data-loss defects before design risk | superseded | Executed: 30 rows, 30 PRs, merged in one day (H-12). |
| O-2 | DI before the pipeline | "everything sits on it"; pipeline PR 5 needs DI D2 and PR 11 edits role registration by scope | superseded | The rebuild inverted the dependency by designing DI first and end to end (`DESIGN.md`), which is the same call taken further. |
| O-3 | `PIPE-6` absorbed by `DI-2` | Both handoffs said whichever ran first | superseded | Row 33's packet records the absorption and Halt 5. |
| O-4 | `DI-14` drops its C20 half to GQL-4 | F191 and GraphQL D6 must not both write a fix | superseded | HEAD binds the engine under `dyn Engine` with no fixed token (transports DESIGN §7). |
| O-5 | `RPC-7` beside `DI-9` | F226 is the DI ambiguity finding at a fourth site | built | HEAD refuses a pattern two handlers declare, naming both (`crates/ulo-rpc/src/server.rs:164-169`). |
| O-6 | `GRPC-3` after `PIPE-9`, full scope (structural, not diagnostic only) | The handoff written at approval supersedes the memo's §7 | built | `ulo-grpc-macros` matches no `"Result"` name (`grep` empty); gRPC replies go through the shared reply surface (transports DESIGN §2.3). |
| O-7 | `GRPC-2` and `REN-1` beside `PIPE-8` | "one rule about what a framework event tells a caller" | built | `GuardRejected` → `Forbidden`, `PanicRecovered` → generic `Internal`, in one table (`crates/ulo-transport/src/error.rs:134-135`). |
| O-8 | GraphQL Phases 2-3 after HTTP-10 and HTTP-11 | The new crate written once against `StatusCode`, `HeaderMap` and problem details | superseded | HEAD's `ulo-graphql-http` is written over `http` types (`controller.rs:7`). |
| O-9 | Adapter migration order poem, salvo, rocket, axum, actix | The spike covered poem and salvo; rocket ships the shape; actix last "so a divergence there stops with the most done"; F244 rides actix | dropped | Adapters became hosts of one service (transports DESIGN §3.7-§3.8). The ordering heuristic (least-evidenced target last) is reusable. |
| O-10 | Capability rows directly after HTTP-1 | Settle the shared type while fresh | dropped | S10. |
| O-11 | ADR numbers provisional, by write order | The number is whatever is free | spent | Master holds 0055-0059 (0055/0056 and 0059 `proposed`, 0057/0058 `accepted`); HEAD holds 0055/0056 only. |
| O-12 | What is not in the sequence stays out: F5, F6, F11, F16, F34, F35, F38-F40, F43-F48, F52, F55, F58, F59, F61, F64, F66, F70-F72, F75, F76, F79, F80, F82, F85, F86, F88-F90, F96-F102, F110 and the audits' residuals; F53/F54 spent by extraction D6 | Taking any is a scope change and a halt | dropped | The list is the inventory of master-era entries no audit placed. Each entry is still present in `FRAMEWORK_GAPS.md`; whether it applies to the rebuilt code is the ledger's question, not this record's. |

---

## 5. The halt log (§6)

| ID | Entry | Answered | Status at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| H-1 | Halt 1, row 12 (RPC-6, F230): "the server's remaining budget" names a number that does not exist before row 110 | Option 1 (client half now, server half to row 110), 2026-09-26 | superseded | HEAD's core client and server own the deadline on every link (`ulo-rpc-conformance` `deadlines.rs`: `timeout_is_the_server_s_deadline`, `stream_deadline_is_the_server_s`, among 19 cases). Kafka keeps a 5 s queue-full retry window (`crates/ulo-rpc-kafka/src/link.rs:65,1414`), not a call bound. Measured on master: three verbs answered `Timeout` in under 1.5 s against a 500 ms bound with the broker stopped; the base ran to its 10 s give-up. rdkafka's `Timeout::After` bounds queueing only, so the delivery future needs the deadline as well (row 12 packet). F252: the first call on an unconnected Kafka client waits on topic creation under rdkafka's admin timeout. |
| H-2 | Halt 2, row 19 (GRPC-6): `grpc-status-details-bin` needs a message type and an encoding | Option 2: `google.rpc.Status` from `tonic-types`, `details()` as one `Struct` in an `Any` | built | `crates/ulo-grpc/Cargo.toml:38` (`tonic-types`); `status.rs`, `dispatch.rs` write the trailer; HEAD's `Details` maps one-to-one onto `google.rpc` detail types (transports DESIGN §2.4), the rich model Halt 2's option 3 described. A non-object JSON value cannot be a `Struct` and packed as `google.protobuf.Value` on master (row 19 packet). |
| H-3 | Halt 3, row 32: deleting the value/factory macros deletes enhancer role detection; an autoref probe inside a generic function always takes the fallback | "Enhancers are types, and a token can carry a role" | superseded | HEAD: "a role comes only from a trait implementation", decided by `TypeId` at freeze (DESIGN §3.7, §7). Value: the stable-Rust fact itself, that a generic `fn f<V>(v: V)` sees `V` only through its bounds, so no autoref probe in it can detect a role; rejected alternatives were roles named at the declaration and one macro kept for enhancer values. |
| H-4 | Halt 4, row 32: `.multi::<dyn Tr>()` needs `Unsize` | Neither option: one call-site `provide!` proc macro; multi keyed by element type | superseded | HEAD: `Many<T, Q>` reads contributions by key (`crates/ulo/src/dependency/many.rs:17`); `into K: [..]` lists lower to `contribute` (DESIGN §4, §7). Probed fact: `fn multi<Tr: ?Sized>(self) -> Arc<Tr> { Arc::new(v) }` fails with E0308 and E0277. Rejected: a per-trait `collection!` line, a coercion closure per call, a `tokens!` block, typed consts with `collects!`. |
| H-5 | Halt 5, row 33: a const and a type are both bare paths, in two namespaces | Option 1, a `token` keyword in source positions, refined by Fable | superseded | Keys are types at HEAD (DESIGN §0 principle 5, §3.1), so the namespace collision does not arise. The namespace argument (a const lives in the value namespace, a non-unit type in the type namespace, no expansion compiles for both) is the reason any future string or const key reintroduces a keyword. |
| H-6 | Halt 6, row 34: every key is a type | Approved "fully and completely", 2026-09-28; ADR-0059 | built | HEAD `Key::of::<T, Q>()` with a qualifier defaulting to `()` (DESIGN §2, §3.1). The research list (Guice, Dagger, Spring, .NET keyed services, `more-di`; typemap's `trait Key { type Value; }`; `type_name` not const) and the decision "no string keys, no escape hatch; adding one later is non-breaking, removing one is not" belong with the di record. |
| H-7 | Halt 7, row 44: a startup refusal needs an error channel the SPI lacks | Option 1, `resolve -> Result<_, ResolutionError>`, `build -> BuildResult` | built differently | HEAD: wiring reports scope violations with the path before anything is built (DESIGN §10.1 step 5); construction fails as `ConstructError` (`crates/ulo/src/error/mod.rs:18`). Rejected on master: a scope query on `ProviderFactory` (an `Alias` cannot answer before its target builds); a panic net around `build` (catches nothing under `panic = "abort"`, and turns a user's `expect` into a startup error). |
| H-8 | Halt 8, row 50: the `Shared` marker is E0119 for `Arc<T>` and E0117 for foreign handles | The minimal version: `Arc<T>` for every shared binding, no `Handle` marker | built | `Dep<T, Q>` holds `Arc<T>` (`crates/ulo/src/dependency/dep.rs:17-19`). The field-spelling argument (a field typed `T` owns a `T`; the container can move, clone or rewrite the type, and each is wrong) is the di record's. |
| H-9 | Halt 9, row 51: `Arc<Extension<T>>` in a factory and `resolve::<Extension<T>>` fail once the declaration is gone | Unanswered in the document; #393 merged on master 2026-11-01 as option (a) | superseded | HEAD's `Ext<T>` is a `FromContainer` injection point (`crates/ulo/src/dependency/ext.rs:14`), readable in a `with` closure taking `u: Ext<CurrentUser>` (DESIGN §7), so the limit (a) accepted does not exist at HEAD. |
| H-10 | Decision: a handle type injects by value (amending D14), 2026-09-28 | Superseded by H-8's answer two days later | superseded | The amendment's own counterexample is the value: `PgPool`, `reqwest::Client` and `ConnectionManager` already share on clone, so `Arc<PgPool>` is a double wrap. HEAD accepts the double wrap. |
| H-11 | Decision: `alias` names a binding by the source rule | Approved 2026-09-28 | superseded | HEAD's alias is a recipe kind on a binding keyed by type (DESIGN §2). |
| H-12 | Decision: merging by wave, and standing authorisation to merge each green wave without manual review | 2026-09-26 | built (process) | Wave 1 merged as stack #371 (`3cc36510`); wave 2 rows 31-51 merged 2026-11-01. The authorisation's premise ("the law in critical steps") is a recorded user ruling: per-commit and per-PR review agents replace manual review. |
| H-13 | Pause for the end-to-end DI redesign, 2026-10-01 | Resume condition: the user decides whether the redesign phases in | superseded | The redesign became the rebuild. The pause records the cut point (`3cc36510`) and the 48-capability brief. |
| H-14 | Question for the ceremony, row 40: how far `.or_global()` reaches | (a) then revised to (b), 2026-09-29; named `.visible()` | built | DESIGN §8.2: a module sees its own bindings, its direct imports' exports and global modules' exports, which is (b). Nest is precedent for neither reading (`strict` reads the host module alone; `strict: false` reads every module with exports ignored, last holder winning). |

---

## 6. Plan-text discrepancies (§7)

| ID | Discrepancy | Status | Value |
| --- | --- | --- | --- |
| P-1 | `WS_BUILD_HANDOFF.md`: D8 "open" vs settled; two duplicated headings; Phase 1 skips PR 6; `WS_AUDIT.md`'s closing line says D8 open | dropped | Textual; the source copy is frozen. A ws reader reading those files needs the correction. |
| P-2 | `RPC_BUILD_HANDOFF.md` numbers Phase 3 PRs 20-23; memory says 18-21 | dropped | Same. |
| P-3 | `GRPC_AUDIT.md` §7 scopes F240 to a diagnostic; the handoff to the structural fix | dropped | O-6 records the resolution. |
| P-4 | F241 "rides PR 6" in both | dropped | Consistent. |
| P-5 | No handoff's read-first list names F242-F244 | dropped | Rows 58, 85-87 carried them. |
| P-6 | The workspace `CLAUDE.md` called `DOCS_AUDIT.md` §18 consumed and empty | dropped | Docs-site matter. |
| P-7 | `HTTP_BUILD_HANDOFF.md`'s F105-F120 range holds F117, which `HTTP_AUDIT.md` §10 leaves out | dropped | S1 resolved it. |

---

## 7. The ceremony and the bookkeeping packets (§8)

| ID | Item | Status | Evidence | Value to extract |
| --- | --- | --- | --- | --- |
| B-1 | The procedure: merge bottom-up, confirm retarget and green checks, apply the packet, mark `[m]` | built (merge half) / open (packet half) | `git log master` 2026-11-01 | B-8. |
| B-2 | Packets rows 1-30 | open: the whole-entry deletions were applied, the ones deferred to row 134 were not | F196, F201, F208, F214, F217, F233, F241 are absent from the ledger; F121, F122, F128, F172, F173, F185, F193, F232, F235, F236, F239 present ("deletes at row 134") | The deferred deletions are still owed against master. |
| B-3 | Packets rows 31-51 | open | F138, F141, F146, F150, F153, F154, F155, F181, F283 present with no closure note (`FRAMEWORK_GAPS.md`); F154's entry ends "Verified against master `654c7183`. Filed 2026-09-22." | The packets carry the exact text. |
| B-4 | In-row filings, all on master: F245-F285 (F246, F247, F248, F249, F250, F251, F252, F253, F254-F285) | present in the ledger | `FRAMEWORK_GAPS.md` | Each describes master's code. The subject readers own whether each applies at HEAD. |
| B-5 | Row 21: whether the sanctioned health/reflection `add_service` path warrants `info` or an opt-out | dropped | HEAD has no `add_service` and serves reflection through `Server::reflection` (`crates/ulo-grpc/src/server.rs:158`) and health through `ulo-health-grpc` | none |
| B-6 | Row 24: re-exporting `multer`, `validator`, `bytes` gives `ulo::multer::Field` a second path beside `ulo::http::extract::Field` | superseded | DESIGN §0 principle 7: a public signature names only what its crate reaches; a transport crate re-exports the transport-neutral items | The second-path cost is the counter-argument any re-export rule has to answer. |
| B-7 | Row 26: ADR-0056 carries the non-singleton middleware refusal that row 63 builds and row 101 accepts; row 27: ADR-0055's grammar cell says a closure reaches DI "through `ctx`", which a sync closure cannot | open | ADR-0055 and ADR-0056 are present at HEAD (`docs/adr/0055-the-pipelines-order.md`, `0056-the-pre-dispatch-role.md`), `proposed`, describing master's pipeline | HEAD's tree carries two proposed ADRs that its code does not implement. See §10 Q5. |
| B-8 | The ledger does not describe master | open, not in the ledger | B-2, B-3 | §1 item 1. |

---

## 8. Cross-subject couplings

The README's couplings (the triage's input) and the sequence's own. Each one's status says whether
the coupling still binds the rebuild.

| ID | Coupling | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| C-1 | DI D2 (single-bound unless multi) inside pipeline PR 5's globals surface | built | DESIGN §10.1 step 2 (duplicate singles, single/collection mixes); globals are contributions under role keys (§7). |
| C-2 | DI F73 is pipeline PR 12; pipeline Phase 2 changes signatures the DI macros emit | superseded | One design for both (DESIGN §3.7, transports §3.3). |
| C-3 | Extraction PR 4 (415) needs pipeline PR 7's `Custom { status, message }` | superseded | `ExtractError::UnsupportedMediaType` (transports DESIGN §2.2); 415 in `crates/ulo-http/src/extract/body.rs`. |
| C-4 | Extraction PR 14 and the query crate touch one function (`Path`'s round trip) | superseded | One deserializer (`extract/de.rs`). |
| C-5 | F180/F181 filed by extraction, built by DI behind D14 | built | `Ext<T>` (ext.rs:14); `RequestHead` is an execution input read as `Dep<RequestHead>` (DESIGN §10.1 sample). |
| C-6 | GraphQL F182/F184 and extraction F163: one argument on two surfaces, blocked differently | built | 415, 406 and the GraphQL-over-HTTP status table in `crates/ulo-graphql-http/src/lib.rs:29-33`. |
| C-7 | GraphQL F191 is the DI ambiguity finding; D6 removes the token | built | Engine under `dyn Engine` (transports DESIGN §7). |
| C-8 | GraphQL D7 rides HTTP PR 1's capability shape | superseded | S10. |
| C-9 | GraphQL F190's adapter half is WS F198/F199 (subprotocol lists) | built | Subprotocol choice "offers over two header lines read as one list" (transports DESIGN, WS conformance handshake group); `ulo-ws-conformance` `subprotocol` case. |
| C-10 | WS Phase 2 consumes the HTTP build | superseded | One hand-off: `ulo-http/src/upgrade.rs` and `ulo-ws`'s `GatewayTable::handshake` (X32). |
| C-11 | WS D8 and HTTP `Dispatched::Upgrade` decide one enum | built | Core-owned WS loop (`crates/ulo-ws/src/connection.rs`), `Switch::serve` over any upgraded `AsyncRead + AsyncWrite` (X32). |
| C-12 | GraphQL PR 6 blocked by WS PR 2 (`Refused` writing an envelope, F201) | superseded | `ConnectRefused::code(4406, ..)` and the `own_code` WS conformance case. |
| C-13 | Four query parsers is five (WS handshake, F206) | UNVERIFIED | Whether `UpgradeHead`'s query is percent-decoded through `extract/de.rs` was not checked. |
| C-14 | Four parsers, three owners (F117, F180) | superseded | C-4. |
| C-15 | RPC F226 is the DI ambiguity at a fourth site | built | O-5. |
| C-16 | RPC F219 and extraction F163 rhyme and must not be merged | superseded | HEAD answers an unhandled RPC pattern `Unavailable` with `reason: "pattern_unhandled"` (`crates/ulo-rpc/src/dispatch.rs:275,291`); whether every link can raise it is the rpc record's. |
| C-17 | RPC Phase 2 rides HTTP PRs 1-9; one capability type across HTTP, WS, RPC, GraphQL | superseded | S10; four shapes at HEAD. |
| C-18 | RPC PR 13 reserves the inbound signature; read gRPC's `Inbound<T>` before inventing a second | built | Client-streaming and bidi cases in `ulo-rpc-conformance` (`client_stream`, `client_stream_in_order`, `bidi_stream`); `Shape::ClientStreaming`/`Bidi` (`crates/ulo-rpc/src/server.rs`). |
| C-19 | gRPC F235 and RPC F222: one abandonment question on two transports | built | Transports DESIGN §2.6. |
| C-20 | gRPC D4 is pipeline PR 7: `Custom { status, message, source }` on all four | superseded | One `CallError { kind, message, details, source }` (transports DESIGN §2.4). The capability D4 protected, a typed domain error kept behind an unusual code so `#[catch(MyError)]` stays reachable, is `CallError::source_as`. A code outside the eleven kinds has no slot in `CallError`; whether HEAD lets a handler name one (gRPC's `GrpcStatus` escape) is the grpc record's. |
| C-21 | gRPC F240 is ADR-0050 reaching three transports | built | O-6. |
| C-22 | gRPC F241 is HTTP D6 from the fourth transport | built | H-2. |
| C-23 | Pipeline PR 12 decides whether gRPC's `add_service` bypass shrinks | superseded | No bypass at HEAD (B-5). |
| C-24 | ADR-0053's framework-wide clause inside an HTTP record | superseded | S4. |
| C-25 | Render pass: F242 and F243 both belong to the HTTP build | built | S5, S6. |
| C-26 | D13: no compatibility retained pre-release; delete rather than deprecate | built | The rebuild itself. |

---

## 9. The rows (§3), one line each

Status at HEAD. "Master" gives the PR and, for rows 31-51, the merge commit. Evidence is the HEAD
location; content and value belong to the named subject record.

### Wave 1 (rows 1-30; master `3cc36510`, in HEAD's history, code rewritten)

| # | Row | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 1 | WS-7 | built | One connection loop answers a Close (`crates/ulo-ws/src/connection.rs:1321-1333`; flush after reading, `:1177`). |
| 2 | WS-9 | built | `crates/ulo-ws-conformance`, 31 scenarios in seven groups, run by `ulo-ws-hyper`, the five HTTP hosts and the pipe servers. |
| 3 | WS-1 | superseded | The doc it corrected is deleted; HEAD's close-code text cites the registry and maps auth to 1008 (`gateway.rs:322,435`). See row 95. |
| 4 | WS-2 | superseded | Refusal is `ConnectRefused` on connect (`own_code` case); a handler closes through `Connection::close(code, reason)` (`connection.rs:130`). |
| 5 | WS-3 | superseded | The Redis wire is `Target` as JSON then the frame bytes (`crates/ulo-ws-redis/src/lib.rs:109-118`), not `WsMessage`'s derived form. |
| 6 | WS-4 | superseded | `WsMessage` is gone; the envelope is `Frame` (`crates/ulo-ws/src/envelope.rs:33`). |
| 7 | WS-5 | built | `Session<T>` built by the framework through `SessionFactory`, fields `pub(crate)` (`session.rs:46-49,97`). |
| 8 | RPC-1 | superseded | Payloads are typed and encoded by the link's `Codec` (`Json`, `Cbor`; `crates/ulo-rpc/src/codec.rs:30-33`); `payloads::binary` case. |
| 9 | RPC-4 | built | `crates/ulo-rpc-tcp/tests/conformance.rs`, `crates/ulo-rpc-udp/tests`. |
| 10 | RPC-5 | built | 19-file case list in `crates/ulo-rpc-conformance/src/cases`. |
| 11 | RPC-CI | built | `ci.yml:300-347`, Kafka included. |
| 12 | RPC-6 | superseded | H-1. |
| 13 | RPC-8 | built | `reply_topic`, `topic_partitions`, `replication_factor` (`crates/ulo-rpc-kafka/src/link.rs:176-196`). |
| 14 | RPC-2 | superseded | Client rewritten (`crates/ulo-rpc/src/client.rs`, `client_module.rs`). |
| 15 | RPC-3 | superseded | One wire grammar (transports DESIGN §5.2). |
| 16 | GRPC-1 | superseded | The parsing site is rewritten; HEAD writes `grpc-timeout` client-side (`crates/ulo-grpc/src/client.rs:105-149`). Server parsing not re-read. |
| 17 | GRPC-7 | built | `CancelReason::Deadline` (transports DESIGN §2.6). |
| 18 | GRPC-8 | built | `ClientCancelled` for a call future dropped before it answered (§2.6). |
| 19 | GRPC-6 | built | `GrpcMetadata::map()` exposes tonic's `MetadataMap` (`crates/ulo-grpc/src/transport.rs:165-177`); H-2. |
| 20 | GRPC-4 | superseded | Details travel (H-2). |
| 21 | GRPC-5 | superseded | No `add_service` (B-5). |
| 22 | GQL-1 | built | `text/html; charset=utf-8` (`crates/ulo-graphql-http/src/playground.rs:23`). |
| 23 | GQL-2 | superseded | Crate rewritten. |
| 24 | EXT-13 | built | DESIGN §0 principle 7. |
| 25 | EXT-15 | built | `Multipart` (`crates/ulo-http/src/extract/body.rs`) and tests naming it (`crates/ulo-http/tests/extract.rs`); one-test-per-arm not counted. |
| 26 | PIPE-ADR | superseded | B-7. |
| 27 | PIPE-1 | built | Type, `value =` and `with =` closure spellings (DESIGN §7). |
| 28 | PIPE-2 | UNVERIFIED | Whether HEAD builds each guard only after the previous admits, and interceptors after every guard, was not found in DESIGN §7 or read in `ulo` dispatch. |
| 29 | PIPE-3 | superseded | Texts deleted. |
| 30 | PIPE-4 | superseded | Internal refactor of deleted code. |

### Wave 2 (rows 31-54; rows 31-51 merged on master 2026-11-01, absent from HEAD)

| # | Row | Master | Status | Evidence at HEAD |
| --- | --- | --- | --- | --- |
| 31 | DI-ADR | #372 `6199313d` | superseded | ADRs 0057-0059 on master only; `DESIGN.md` is the record. |
| 32 | DI-1 | #373 `163c3295` | superseded | Value API, `ModuleDef`, `contribute` (DESIGN §4, §13). |
| 33 | DI-2 | #374 `9b9551ab` | superseded | Keys by type plus qualifier (§3.1). |
| 34 | DI-3 | #375 `7f3aaf99` | superseded | One grammar, macros as sugar (§0 principle 2). |
| 35 | DI-4 | #376 `480b9a7d` | superseded | `#[construct]` / `Construct` (§3.4). |
| 36 | DI-5 | #377 `beac6370` | superseded | Construction model rewritten (§3.4). |
| 37 | DI-6 | #378 `043ec5a4` | superseded | Wiring reports hooks on per-execution types (§10.1 step 5). |
| 38 | DI-7 | #379 `b6e02075` | built | §10.1 step 2. |
| 39 | DI-8 | #380 `8b33117f` | built, differs | §9.2: stable topological order, reverse at close; "a module's own hooks run after the hooks of its providers", where #380 ran a module's own hooks ahead of its providers' in every phase. §10 Q2. |
| 40 | DI-9 | #381 `0323edcb` | built | §8.2 (H-14 (b)). |
| 41 | RPC-7 | #382 `8fad80da` | built | `crates/ulo-rpc/src/server.rs:164-169`. |
| 42 | DI-10 | #383 `013841cd` | superseded | `Many<T, Q>` injection point (`many.rs:17`). |
| 43 | DI-11 | #384 `384444ff` | superseded | Resolver rewritten (`crates/ulo/src/resolver.rs`). |
| 44 | DI-12 | #385 `257d00fb` | built | §10.1 step 5 with the introducing path. |
| 45 | DI-13 | #387 `d6011c2f` | superseded | "Binding" is HEAD's word (§2). |
| 46 | DI-14 | #388 `ad796ca4` | superseded | App typestates `Wired → Connected → Bound` (§2). |
| 47 | DI-15 | #389 `686ce8f1` | superseded | Readiness checks (§9.3); keyed identity (§8.3). |
| 48 | DI-17 | #390 `bc90dd34` | built | §10.1 step 4, full path with each step's module. |
| 49 | DI-16 | #391 `75b8b3c5` | superseded | SPI rewritten. |
| 50 | DI-19 | #392 `2858cbf2` | built | `Dep<T>` holds `Arc<T>` (`dep.rs:17-19`). |
| 51 | DI-20 | #393 `861e9840` | built | `Ext<T>` (`ext.rs:14`); H-9. |
| 52 | DI-18 | never started | superseded | Its targets (`ExtensionsFactory`, `_get_controller_by_token`, `ProviderContext`) do not exist at HEAD. |
| 53 | DI-21 | never started | dropped | `ulo::http::Request` and its maps do not exist. |
| 54 | DI-22 | never started | superseded | `RequestHead` as an execution input (DESIGN §10.1 sample). |

### Wave 3 (rows 55-75; never started)

| # | Row | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 55 | PIPE-5 | built | Globals as contributions under `AnyGuard<T>` etc. (DESIGN §7). |
| 56 | PIPE-7 | superseded | One `CallError` (transports §2.4); C-20. |
| 57 | PIPE-8 | built | `GuardRejected` → `Forbidden` on every transport (`ulo-transport/src/error.rs:134,293`). |
| 58 | REN-1 | built | S5. |
| 59 | GRPC-2 | superseded | Same table as row 57. |
| 60 | PIPE-9 | built | `Transport`, `Next<'_, T>`, roles generic over one `T` (DESIGN §3.7). |
| 61 | GRPC-3 | built | O-6. |
| 62 | PIPE-10 | built, differs | `can_activate -> Result<bool, BoxError>` (§3.7), not `Result<(), E>`. |
| 63 | PIPE-11 | UNVERIFIED | Whether an error handler takes a per-execution (`with =`) entry at HEAD was not found. |
| 64 | EXT-ADR | superseded | Transports §2.2. |
| 65 | EXT-1 | superseded | `FromCall` fails with `ExtractError`, which implements `Classify` (§2.2). |
| 66 | EXT-2 | built | `ExtractError` reaches the error handlers as a `CallError` (§2.2). |
| 67 | EXT-3 | built | `BadRequest`, `Unprocessable` for `Invalid`, `Internal` for `HostMissing` (§2.2). |
| 68 | EXT-4 | built | `UnsupportedMediaType`; `+json` in `crates/ulo-http/src/extract/body.rs`. |
| 69 | EXT-5 | built | `Invalid { violations }`, `Detail::FieldViolations` (§2.2, §2.4). |
| 70 | EXT-6 | superseded | No `#[body]`/`#[param]`/`#[query]` markers; extractors are types. |
| 71 | EXT-7 | superseded | Same. |
| 72 | EXT-8 | superseded | Same. |
| 73 | EXT-9 | superseded | `Param<T, M>` with `ViaCall`/`ViaContainer` (§2.2). |
| 74 | EXT-10 | superseded | A destructuring pattern is named by its tokens (§2.2). |
| 75 | EXT-11 | superseded | Context is `T::Cx`, read through `FromCall` (§2.2). |

### Wave 4 (rows 76-107; never started)

| # | Row | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 76 | HTTP-1 | superseded | S6, S10. |
| 77 | WS-20 | built | `permessage-deflate` answered with no `Sec-WebSocket-Extensions` (`connection.rs:761`). |
| 78 | RPC-9 | built | Link declarations: `binary`, `max_frame`, `Competing` (transports §5.2-§5.3). |
| 79 | HTTP-2 | built | Core router (`crates/ulo-http/src/router`); `routing` conformance. Not `matchit`. |
| 80 | HTTP-Q | superseded | S1. |
| 81 | HTTP-3 | built | `routing` cases `not_found`, `method_not_allowed`, `options`, `head`, `percent_decoded`, `trailing_slash`. |
| 82 | WS-21 | built | C-11. |
| 83 | HTTP-5 | superseded | Hosts embed one service (§3.7-§3.8). |
| 84 | HTTP-6 | superseded | Same. |
| 85 | HTTP-8 | superseded | Same. |
| 86 | HTTP-4 | superseded | Same. |
| 87 | HTTP-7 | superseded | Same; actix now runs WS conformance (`crates/ulo-http-actix/Cargo.toml`). |
| 88 | HTTP-9 | superseded | No `register_route` SPI. |
| 89 | WS-13 | built | `subprotocols = [..]` (§4.1); handshake `subprotocol` case. |
| 90 | WS-8 | built | `read_failure`: 1002, 1007, 1009 (`connection.rs:1415-1423`). |
| 91 | WS-12 | built | `FRAME_LIMIT` 16 MiB, `DEFAULT_MESSAGE_LIMIT` 64 MiB (`gateway.rs:607-609,635`); 1009 on breach. |
| 92 | WS-15 | open | No `Origin` check in `ulo-ws` (§1 item 5). Ledger: F207 is a master-era entry. |
| 93 | WS-16 | built | `UpgradeHead` carries the peer (X32 `handshake(head, peer)`). |
| 94 | WS-10 | built | `ConnectRefused::code -> Result<Self, CloseCodeError>` refuses 1004-1006, 1015, out of range, reason over 123 bytes (transports DESIGN §4.2). |
| 95 | WS-11 | open | `Unauthorized`/`Forbidden` close 1008 (`gateway.rs:435`), not 3000/3003. §10 Q3. |
| 96 | WS-14 | superseded | `event = ".."` on the attribute; a hand-written `Gateway::on_message` reads frames raw (§4.1). |
| 97 | WS-17 | built, differs | `BroadcastAdapter::publish -> Result<(), BoxError>` (`broadcast.rs:26-58`). |
| 98 | WS-18 | superseded | Own wire (`Target` + bytes); carries no version field. |
| 99 | WS-19 | UNVERIFIED | C-13. |
| 100 | EXT-12 | superseded | `OnConnect`/`OnDisconnect` traits with `DisconnectReason` (§4.1). |
| 101 | PIPE-12 | superseded | Middleware is HTTP-only, not a core role (transports §3.3); gRPC has its own `pre_dispatch.rs`. §10 Q4. |
| 102 | HTTP-10 | built | `http` crate types throughout `ulo-http`. |
| 103 | HTTP-11 | built | `problem+json` in `crates/ulo-http/src/render.rs`. |
| 104 | HTTP-12 | open | `Vary: Origin` built (`cors.rs`); no cookie surface (`grep -ri cookie crates/ulo-http/src` empty). |
| 105 | HTTP-13 | built | `.body_limit(2 * MB)`, per-route `BodyLimit` (transports §3.1). |
| 106 | HTTP-14 | open | No application-wide prefix; per-controller `.at(prefix)` (`crates/ulo/src/transport/handler.rs:134`). |
| 107 | EXT-14 | superseded | `Path` fails with `ExtractError`. |

### Wave 5 (rows 108-122; never started)

| # | Row | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 108 | RPC-10 | superseded | Row 8. |
| 109 | RPC-11 | built | Core client owns deadlines, gap bounds and cancel (`deadlines.rs`, `cancel.rs` cases). |
| 110 | RPC-12 | built | Core server dispatch and drain (`crates/ulo-rpc/src/server.rs`, `dispatch.rs`; `drain` case). |
| 111 | RPC-13 | built | C-18. |
| 112 | RPC-14 | built | `Competing` via NATS queue group and MQTT `$share/<group>/` (transports §5.2); `delivery::two_instances`. |
| 113 | RPC-15 | built, differs | `DEFAULT_MAX_FRAME` 4 MiB (`crates/ulo-rpc-tcp-core/src/link.rs:27`), plan 1 MiB; `max_frame(..)` (`ulo-rpc-tcp/src/link.rs:87`). |
| 114 | RPC-16 | built, differs | `check_pattern`: non-empty, no whitespace or control character (`server.rs:399`); no per-link declared rules. |
| 115 | RPC-17 | built, differs | S3. |
| 116 | RPC-18 | open | Credit reserved, unsent (DESIGN decision 15). |
| 117 | RPC-19 | open | Same. |
| 118 | GATE | dropped | S9 spent. |
| 119 | RPC-20 | built | Inbound grammar and correlation (`client_stream_in_order` case). |
| 120 | RPC-21 | built | Handler-side request stream (`client_stream`). |
| 121 | RPC-22 | built | `client_stream`. |
| 122 | RPC-23 | built | `bidi_stream`. |

**Row 116-117's design, which HEAD does not carry.** Per-call credit as a wire frame
`{"id":…,"credit":N}` in the shared grammar; the server's reply driver waits for credit and the client's
stream forwarder issues it; in band on tcp and udp; on NATS, Redis and MQTT over a per-call control
address; Kafka and RabbitMQ declare through the capability that they cannot, and fall back to a
bounded queue with shed-and-cancel, the cancel sent before the error (`BUILD_SEQUENCE.md` §3 rows
116-117; `RPC_BUILD_HANDOFF.md` D8). HEAD's reason for deferring: a window on NATS and Redis needs
both ends to implement it (transports DESIGN §5.2).

### Wave 6 (rows 123-130; never started)

| # | Row | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 123 | GQL-3 | built | `Engine` SPI (`crates/ulo-graphql/src/lib.rs`; transports §7). |
| 124 | GQL-4 | built | C-7. |
| 125 | GQL-5 | built | C-6. |
| 126 | GQL-6 | built | `crates/ulo-graphql-ws`. |
| 127 | GQL-7 | open | No graphql-sse binding. |
| 128 | GQL-8 | open | No resolver-as-handler. |
| 129 | GQL-9 | open | No `#[graphql_object]`/`#[query]`/`#[mutation]` declaration macros. |
| 130 | GQL-10 | superseded | `GqlContext` as `#[injectable(execution)]` replaces `ContextBuilder` (§7). |

**Row 127's falsifier.** A validation error answers `200` plus a `next` event on graphql-sse and `422`
on GraphQL-over-HTTP, from one engine result. HEAD's HTTP binding exists, so the case can be written
the day a graphql-sse binding is.

### Wave 7 (rows 131-134; never started)

| # | Row | Status | Evidence at HEAD |
| --- | --- | --- | --- |
| 131 | D3-ii-HTTP | built | `Disconnected` on a peer close before the response ends (transports §3.6 "Cancellation"); `disconnect::abandoned`. |
| 132 | D3-ii-RPC | built | A link losing a caller delivers a `cancel` per call in flight, read as `Disconnected` (§2.6). |
| 133 | D3-ii-WS | built | §2.6, WebSocket `cancel` frame `ClientCancelled`. Not re-checked for a message handler running when the socket closes. |
| 134 | CLOSE | dropped | The walk of every handoff's verification list is moot against rewritten code; the ledger half survives as B-8. |

---

## 10. Worth discussing with the designer

1. **The load-shed word (S3, row 115).** The user signed `overloaded` → `TooManyRequests` on the gRPC
   argument (`statuscodes.md` folds a shed load into `RESOURCE_EXHAUSTED`, which `code_for` mapped
   from `TooManyRequests`). HEAD answers `Unavailable` with `RetryAfter` and 503/`UNAVAILABLE`.
   Recommendation: keep HEAD's choice and record S3 as superseded; `TooManyRequests` names a caller
   over its own quota, a shed load is the server's state, and 503 with `Retry-After` is RFC 9110's
   answer for it.
2. **A module's own hooks before or after its providers' (row 39).** Master (#380, ADR-0057 on master)
   runs them ahead in every phase; HEAD's DESIGN §9.2 runs them after at startup and reverses at
   close. Recommendation: HEAD's order (a module hook may use its providers); the designer confirms
   the reversal at close, which puts the module's destroy hook first, ahead of its providers'.
3. **Close codes for auth refusals (row 95, F202).** WS D4 settled moving `Unauthorized`/`Forbidden` to
   the registry's 3000/3003 (and `Timeout` to 3008); HEAD kept 1008. Recommendation: decide
   explicitly; 1008 is what browsers and proxies treat as policy, 3000-range codes are registry-valid
   and more specific. The rejected-alternative argument is in `ws/WS_AUDIT.md` D4.
4. **The pre-dispatch role's reach (row 101, pipeline D2, S13).** The pipeline audit settled one
   pre-dispatch role on all four transports; HEAD makes middleware HTTP-only and gives gRPC its own
   pre-dispatch. Recommendation: ask the pipeline reader whether D2's motivating cases (auth before
   routing on RPC and WebSocket) are covered by guards at HEAD; if they are, record D2 as superseded.
5. **Two proposed ADRs in HEAD's tree (B-7).** `docs/adr/0055` and `0056` describe master's pipeline.
   Recommendation: mark them superseded by `DESIGN.md` or remove them on the switch, and decide
   which master ADRs (0057-0059 merged 2026-11-01) the switch carries.
6. **One capability shape or four (S10, C-17).** The triage wanted one `#[non_exhaustive]` declaration
   type across HTTP, WS, RPC and the GraphQL engine; HEAD has four. Recommendation: accept four
   unless a consumer reads capabilities generically; nothing at HEAD does.
7. **Master is ahead of HEAD's base by 21 merged rows.** The switch base and what happens to master's
   #372-#393 (and their ADRs and ledger entries) is undecided in any document read here.
   Recommendation: decide before the ledger is reconciled (B-8), since reconciling against a master
   that is about to be replaced is wasted work.
8. **RPC frame ceiling (row 113).** 4 MiB at HEAD against the plan's 1 MiB. Recommendation: keep 4 MiB
   only if a reason is recorded; the plan's number had none recorded in this file either.

---

## 11. Not verifiable here

| Item | What would decide it |
| --- | --- |
| Row 28 (PIPE-2), guard construction order | A test at HEAD with two execution-scoped guards, the first refusing, asserting the second and every interceptor were never built. |
| Row 63 (PIPE-11), error handler per-execution entries | Compiling an `#[error_handlers(..)]` entry written with `with =` and a closure at HEAD. |
| Row 99 / C-13, handshake query decoding | A WS handshake with `?a=%20b` read through the gateway's query accessor. |
| Row 133, a running message handler when the socket closes | A WS conformance case asserting the message's execution observes `Disconnected` or `ClientCancelled`. |
| S1's `n=` behaviour on HEAD's deserializer | A `Query<Option<u32>>` test with `n=`. |
| S12 (F120), `app.routes()` as a read | Reading `ulo-http`'s public surface for a route-table accessor. |
