# Handoff — building what the gRPC audit decided

For the session that implements it. The plan is `GRPC_AUDIT.md` §7; nothing here restates it.

## Status

**Approved 2026-09-23.** F240 was named explicitly by the user; the rest of §7 and all four decisions
were approved with it. `GRPC_AUDIT.md` §8 records D1-D4 with the alternatives rejected and why.
**They are not open.** A recommendation kept beside a decision is there as its argument, not as a
choice still to make.

Two of the seven PRs are **not this build's to execute** and are listed only so the sequence is
readable: D4's error-vocabulary change is the pipeline build's PR 7, and D2's pre-dispatch anchor is
its PR 12. Do not build either here.

## Read first, in this order

1. `GRPC_AUDIT.md` §8 — the four decisions.
2. `GRPC_AUDIT.md` §1 and §3 — the seam against tonic, and the cancellation defect. Several rows
   below only make sense against them.
3. `GRPC_AUDIT.md` §6a — gRPC against the six settled audits. It is what makes three of these PRs
   wait on other builds rather than race them.
4. `FRAMEWORK_GAPS.md` **F235-F241** for what this closes, and **F63**, **F75**, **F129**, **F130**,
   **F16**, **F1** for the older entries it touches or is touched by.
5. `artifacts/grpc_audit_probe.rs` and `artifacts/grpc_spelling_probe.rs` — the
   evidence. The second **does not compile by design**; it is F240's reproduction.

## The sequence

Two phases. Phase 1 is free-standing and can start today in any order. Phase 2 waits on decisions
executed elsewhere.

### Phase 1 — independent

| # | PR | Closes | Notes |
| --- | --- | --- | --- |
| 1 | `split_at_checked` in `parse_grpc_timeout`, plus a non-ASCII case in the test beside it | F239 | One line. The function's doc already promises the behaviour the fix gives it |
| 2 | A guard's refusal renders `Error::message()`, as the other three transports do | F238 | Keep the index on the event and in the `tracing::debug!`. The panic branch one above renders `Internal` and logs the payload |
| 3 | **F240 — the return type stops being read by name** | F240 | The large one. Its own section below |
| 4 | State in `to_tonic`'s doc and `Error::details()`'s that detail does not travel on gRPC | F241 (half) | The real fix writes `grpc-status-details-bin` and rides PR 6 |
| 5 | A startup `warn` per service registered through `add_service`, naming the bypass | D2's half | Worth having whichever way the pipeline build anchors the pre-dispatch role |
| 6 | The metadata view: an ASCII multimap plus a binary map, `headers_all(k)` and `header_bin(k)` beside `header(k)` | F237 | D1. `header(k)` keeps today's shape, so existing callers do not move |
| 7 | Fire the execution's token when the deadline passes | F236 | Delete `CancellationToken`'s falsified sentence in the same change |

### Phase 2 — after the drop-site design is proved

| # | PR | Closes | Gate |
| --- | --- | --- | --- |
| 8 | A drop guard on the unary path, disarmed at a clean return | F235 | PR 7. Partly covered by it; size after it merges |

PR 7 before PR 8 because the deadline case is the one with a timer to hang the test on, and because
§8's D3 makes gRPC the place the drop-site design is validated **before** the other three transports
are changed to match it. If PR 7 shows the design is wrong, it is wrong here and cheaply.

## PR 3 — F240, in the detail the rest do not need

**What is wrong.** `answer_type` treats a return type as fallible when its last path segment is the
identifier `Result` (`crates/ulo-macros/src/grpc_macro/grpc_methods.rs:996-1009`), and
`response_inner_type` unwraps a reply when the last segment is `Response` (`:960-976`). An alias for
either fails to compile with a diagnostic pointing at the attribute.

**Where the reply type should come from instead — established, not proposed.** `ulo-build` already
writes a `MethodShape` marker per method carrying `type Arg`, read off the request argument of the
trait tonic generated (`crates/ulo-build/src/lib.rs`, `companion_for`). The same trait carries the
reply, and it is readable there in both shapes:

- **unary** — `async fn create(&self, request: Request<super::CreateOrderRequest>) -> Result<Response<super::CreateOrderResponse>, Status>`.
  The reply type is nameable.
- **streaming** — `type WatchProgressStream: Stream<Item = Result<super::ProgressEvent, Status>> + Send + 'static;`
  and the method answers `Response<Self::WatchProgressStream>`. **The response type is an associated
  type of the implementor and cannot be named from the marker**, but the **item** type is readable
  from the bound.

So the marker gains a reply shape split the way the request side already is — a message for a unary
method, an item for a streaming one. Verified against the generated
`$OUT_DIR/ulo_test.orders.rs` this session.

**The wall to expect, named so it is not rediscovered.** Once the signature comes from the marker,
the macro still has to accept what a handler returns — a bare reply, `Result<Reply, E>`,
`tonic::Response<Reply>`, `Result<Response<Reply>, E>`. A trait with a blanket impl over the bare
case plus an impl for `Result` is **E0119**, which is the same wall ADR-0050 hit on the other three
transports. Two things follow:

- The conversion trait belongs in **`ulo-grpc`**, not `ulo`: `impl IntoOutput<Grpc> for tonic::Response<T>`
  is an orphan violation from either crate, and `ulo-grpc` is where `ReplyEnvelope` and
  `RequestCarrier` already live for exactly this reason.
- **Read `project-marker-vs-autoref-disambiguation` in memory before choosing the mechanism.** It
  records which gets past E0119 — autoref depth for a rank, a marker type parameter for disjoint
  shapes — and `ulo::__rpc::answer` is the worked example on the RPC side.

**Scope, decided.** The *cheap half* — a diagnostic naming the rule, spanned at the return type — is
worth landing even if the structural half turns out larger than it looks, because today's failure
names neither the alias nor the rule and is reported twice. **Do not ship only the diagnostic
without saying so**: the audit records F240 as the name matching, and a PR that documents the
constraint rather than removing it is a divergence, not a smaller version of the same PR.

**F75 rides the same surface, and stays parked.** Its design puts `type Wants = Unary<GreetRequest>`
on this same marker so the macro can assert a *parameter* against what the method carries. This PR
extends the marker for the *reply*. **Whoever writes the codegen should read F75's design first** —
`project-grpc-parked-followups` holds it — and shape the extension so F75 is a later addition rather
than a rewrite. Building F75 is not approved; making it cheap is free here.

## Verification each PR needs

Promoted from the audit's probes, which are in `artifacts/` and assert nothing. Each becomes an
assertion.

- **PR 1** — a `grpc-timeout` whose last byte is not a char boundary reads as absent rather than
  panicking. Falsify by reverting the fix and seeing the test panic.
- **PR 2** — a refused caller receives `PermissionDenied` with message `Forbidden`, and the guard
  index appears in neither the message nor any other wire field. The existing test at
  `rpc_grpc_macros.rs:879-887` asserts the code only and gains the message.
- **PR 3** — a handler returning an alias for `Result` compiles and behaves as fallible; a handler
  returning an alias for `tonic::Response` compiles. `grpc_spelling_probe.rs` is the red test:
  **it must compile after this PR**, which is the inversion that proves it.
- **PR 5** — a service registered through `add_service` produces the warning; one registered through
  DI does not.
- **PR 6** — a repeated ASCII key reaches `headers_all(k)` with both values in order, `header(k)`
  still answers the last, and a `-bin` key reaches `header_bin(k)` with its bytes. Falsify against a
  call carrying neither.
- **PR 7** — a handler whose caller set a 300 ms deadline sees its token fire, and detached work
  holding a clone of it stops. `grpc_audit_probe.rs`'s PROBE 1 and PROBE 7 are the two cases, and
  **PROBE 7 is the one that currently delivers 400 items past the deadline** — it is the sharper
  test.
- **PR 8** — a client that drops a unary call mid-flight fires the token; detached work stops.
  Falsify against a call that completes normally, which must **not** fire it.

**Self-test every one of these before believing it.** A first clean run is not evidence — the rule is
`feedback-self-test-the-checker`, and this audit's own verification pass caught wrong citations twice.
PR 3's inversion is the strongest instrument here: a probe that must go from failing to compiling.

## Not this build's

- **D4 — the error vocabulary.** Settled in `GRPC_AUDIT.md` §8 and executed by the **pipeline build's
  PR 7**: `Status { kind, message }` / `Custom { status, message, source }` / `AppError` on all four
  transports, with `GrpcCode` where HTTP has its status. Building it here would write that function
  twice and would change three other transports from a gRPC branch.
- **D2's anchor — where the pre-dispatch role sits on gRPC.** The **pipeline build's PR 12**. This
  build takes only the startup warning (PR 5), which is worth having either way.
- **F129** — gRPC keeping a second interceptor chain. The pipeline build's PR 4.
- **F1's gRPC half** — an extraction failure rendering `Internal`. The extraction build's D1, which
  changes `FromContext::Error` to `ulo::Error` on all four transports. `RequestError` gaining a
  `kind()` belongs there, not here.
- **F16** — one struct serving one proto service. Unrelated and unscheduled.
- **F75** — parked. See PR 3.

## The branch

None exists. Nothing was committed by the audit and master is clean at `654c7183`. The two probes
live in `artifacts/` beside this handoff and are not in the repo.

## The divergence gate — stop and ask

Approval of this build is approval of **what was read and understood**. If the build has to depart
from that, it pauses and reports rather than proceeding on judgement.

Divergence: a signature that differs from what §8 describes; an alternative §8 records as rejected
turning out to be necessary; a finding that reorders the sequence, removes a PR or adds one; a
verification that cannot be written as above; **PR 3 turning out to need a change to `ulo-build`'s
public API**, which is a build-script surface two crates in the tree already call; **PR 3 shipping
the diagnostic without the name matching removed.**

Not divergence: naming, file placement, test names, the order of two PRs inside Phase 1.

On divergence: state what the memo said, what the tree requires, and what the options are. Wait.

## The Law

Per `feedback-the-law-review-protocol` in memory: before every commit message and every PR title and
body, a dedicated agent reviews the text against `~/.claude/commits.md`, `~/.claude/pull-requests.md`,
`~/.claude/prose-discipline.md` and `~/.claude/posture.md`. Run it in the background and surface only
its findings. Give it the branch and commit, an instruction to read the diff in full, a numbered list
of the technical claims to verify hardest, and how many prior passes found factual errors. Ask for
both facts and register; do not tell it the register scan is reliably clean.

A finding from the Law is a claim, not a verdict.

## Per-PR checklist

Walk it. The recorded failure mode is a checklist written and not walked — `feedback-walk-the-checklist-you-wrote`.
The two most often missed here:

- **Production text this change falsifies** — doc comments, the ADRs, and the workspace `CLAUDE.md`
  in the untracked parent, where a repo-wide grep misses it. This audit already corrected three of
  its rows; PRs 3, 6 and 7 each touch another. `CancellationToken`'s doc carries a sentence PR 7
  deletes.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs
  (`feedback-all-features-hides-cfg-bugs`), and `ulo-grpc`'s TLS features are exactly that shape.

One gRPC-specific item: **`cargo test -p integration-tests` one at a time.** The gRPC tests bind
ephemeral ports and several are `#[serial]`, and `feedback-one-cargo-test-at-a-time` records that
`#[serial]` locks only within a process.

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest there (**F241** as of
2026-09-23). Do not move `Last synced HEAD` or the `Docs status` section in `CLAUDE.md` — a
documentation-site audit moves those. Append one line per merged change to `DOCS_AUDIT.md` §18 naming
the pages it invalidates; PRs 3, 6 and 7 each invalidate a gRPC page.

Findings about the pipeline rather than the gRPC seam belong to the pipeline build unless this one
surfaces them, in which case they are this session's and file here.
