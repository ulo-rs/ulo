# Handoff — building what the WebSocket audit found

For the session that implements it. The plan is `WS_AUDIT.md`; nothing here restates it.

## Status

**D1–D7 are settled.** `WS_AUDIT.md` §13 records them, each with the alternatives rejected and why.
They were put to the user and answered on 2026-09-23. **They are not open**, and a recommendation
kept beside one is its argument rather than a choice still to make.

**D8 is settled too**, after a spike that passed on all five adapters: core owns the WebSocket
connection loop, and it is designed **with** the HTTP build's `Dispatched` rather than after it.
`Dispatched::Upgrade` carries a `WsUpgrade` — naming the selected subprotocol and accepting the
transport — instead of `Arc<WsConnectionCallbacks>`, which is what makes D3's remaining half and D8
one change rather than two.

**Two rows in an earlier revision of this sequence no longer exist.** The `catch_unwind` stop-gap and
the callback widening were the fix while D8 was open; under D8 both are absorbed by the trait that
replaces `WsConnectionCallbacks`. Do not build them.

Two of the settled decisions moved after they were first written, and the movement matters to this
build:

- **D3 shrank.** The pre-upgrade seam it asked for is already built by `HTTP_AUDIT.md` §1's
  `dispatch(req) -> Dispatched`. F207 and F205 become cheap consequences rather than PRs of their
  own, and what remains is a subprotocol slot on `Dispatched::Upgrade` plus `accept_hdr_async` on the
  separate-port path.
- **D7 widened.** It was a question about a serde envelope and is now the coherence of the whole
  broadcast surface, F213 included.

## Read first, in this order

1. `WS_AUDIT.md` §13 — the decisions, D1–D8 settled.
2. `WS_AUDIT.md` §1, §2 and §11 — the three spines: the unflushed close frame, the pre-upgrade seam
   the HTTP build supplies, and the connection loop written five times. Most of the twenty findings
   reduce to those three.
3. `WS_AUDIT.md` §12 — what the probe measured and the trap it was rebuilt to avoid.
4. `FRAMEWORK_GAPS.md` F196–F215 for what this closes, plus F36 (the conformance suite), F66 (the
   control-frame split) and F190 (the gateway half of the subprotocol finding).
5. `HTTP_AUDIT.md` §1 **before Phase 2** — `Dispatched` and `AdapterCapabilities` are what PRs 13,
   15, 16 and 20 mount on, and its spike is what proved the upgrade decision can be taken from the
   ulo request. Not optional reading: the first version of D3 costed a new SPI for a seam that
   section already builds.
6. `project-the-spec-conformance-seam` in memory — the three-category test. Most of what this build
   decides is category two: the protocol leaves it open and ulo has to choose once.

**Fetch RFC 6455 rather than recalling it**, and fetch the IANA registry separately. The close codes
D4 is about are **not in the RFC** — §11.7 creates the registry and the codes were added to it in
2021, 2023 and 2024. An implementer working from the RFC text alone will confirm the comment this
build is correcting.

```
curl -sS https://www.rfc-editor.org/rfc/rfc6455.txt
curl -sS https://www.iana.org/assignments/websocket/websocket.xml
```

## The probe is the suite

`ws_wire_probe.rs` is in `artifacts/` beside this handoff (copied out of the session scratchpad on
2026-09-23, which is session-scoped and under `/private/tmp`) — 1 056 lines, a hand-rolled raw-frame client and
twelve cases: nine across all five adapters, two across the four serving a same-port upgrade, and one
that needs no server. It is written as `println!` reporting, not as assertions, because
an audit measures rather than gates.

**It is also the answer to F36**, which has wanted a `WsAdapter` conformance suite since 2026-09-12
and specifies the shape: *"one suite instantiated per implementor, not one suite per implementor"*,
per `docs/explainers/testing-and-examples.md:84`. Promoting it is a `macro_rules!` stamped five
times, with two exceptions written into the suite the way rocket's listener refusal already is —
tungstenite serves a separate port only, and rocket implements no `WsAdapter`.

Promote it in the PR that first needs it to gate rather than in one of its own. Each case becomes the
assertion for a finding, and the cases that already pass — fragmentation, the interleaved control
frame, life after a handler's Close, Ping/Pong — become the regression floor.

## The sequence

### Phase 1 — free-standing

| # | PR | Closes | After |
| --- | --- | --- | --- |
| 1 | Correct the `close_code` doc comment, which asserts RFC 6455 has no auth-specific code | half of F202 | — |
| 2 | `WsError::Refused` answers a close frame on the message path, as its own doc comment describes | F201 | — |
| 3 | Pin the `ulo-ws-redis` broadcast envelope in a test | part of F210 | — |
| 4 | `#[non_exhaustive]` on `WsMessage` | F208 | 3 |
| 5 | Drop `Session`'s `Default` derive; `new()` becomes `pub(crate)` | F214 | — |

PR 2 **blocks the GraphQL build's PR 6**, which routes graphql-transport-ws's five closures through
`WsError::Refused` — until this merges that fix writes envelopes where it means to write closes.

PR 4 is ordered after PR 3: the pin is what makes the format change visible, and a new variant is the
change it exists to catch.

F212 is **not** a Phase 1 row. The `catch_unwind` stop-gap was the fix while D8 was open; under D8
the cleanup lives in core and the wrapper disappears from all five. It closes with PR 21, and
the number 6 stays free for that reason.

### Phase 2 — the settled decisions

| # | PR | Closes | Needs | After |
| --- | --- | --- | --- | --- |
| 7 | Poll the read half once more after a Close, four adapters | F196 | D1 | — |
| 8 | A normalised read error on the WS transport trait, mapped to a §7.4.1 close code | F197 | D1, D8 | 21 |
| 9 | Promote the probe to a `WsAdapter` conformance suite | F36 | — | 7, 8 |
| 10 | `close_with` returns a `Result`, refusing `1005`, `1006`, `1015` and anything under `1000` | F200 | D5 | — |
| 11 | `ErrorKind` maps to `3000` / `3003` / `3008` where the registry has them | rest of F202 | D4 | 1, 10 |
| 12 | A WebSocket frame ceiling on the registration surface, default 16 MiB, `1009` on breach | F203 | D6 | 8 |
| 13 | `Gateway` declares its subprotocols; `WsUpgrade` carries the selected one; `accept_hdr_async` on the separate-port path | F198, F199, F190's adapter half | D2, D3, D8 | 21 |
| 14 | `event_field` becomes `route(&self, msg) -> Option<String>` | F211 | D2 | — |
| 15 | Origin verification as a `Dispatched::Respond(403)` | F207 | D3 | HTTP build PR 9 |
| 16 | The peer address into the request extensions and out to `WsHandshake` | F205 | D3 | HTTP build PR 9 |
| 17 | `trait Broadcast`, async throughout, `send -> Result<(), _>`; both services behind it | F213 | D7 | — |
| 18 | `BroadcastFrame` as `ulo-ws-redis`'s own wire type, tagged and versioned | rest of F210 | D7 | 3, 17 |
| 19 | The handshake query through the crate ADR-0052 picks | F206 | — | — |
| 20 | `negotiates_extensions: false` in `AdapterCapabilities` | F209 | — | HTTP build PR 1 |

**PR 7 is the only thing that must not wait for PR 21.** F196 is the pass's one MUST and its remedy
is one extra poll: `tungstenite` queues the close reply and flushes it at the top of the next
`read()`, and four adapters break before that read. axum passes today because its arm is
`Err(_) => {}` and it loops. Ship it against the present SPI; PR 21 then replaces the poll with an
explicit `close(frame)` on the transport trait, which is the difference between relying on an
undocumented library internal and asking the library for what is wanted.

**PR 8 moved behind PR 21.** F197 is a close frame nothing composes — no protocol error queues
anything, so there is no flush to reach — and writing it needs each library's read error mapped to a
§7.4.1 code across five error types. That mapping is the transport trait's normalised error, so it is
PR 21's surface rather than a bolt-on to a `dyn Fn`. **F204 leaves this row entirely**: the disconnect
reason is a parameter on a trait method once `WsConnectionCallbacks` is one, and it closes with PR 21.

**Verify PR 7 against axum.** It passes today, so a change that breaks axum has broken the mechanism
rather than fixed the other four.

**PR 9 gates everything after it.** Five adapters diverging with nothing to diverge against is what
#258 found on the RPC side, and this build changes five adapters at once.

**PRs 13, 15 and 16 ride `Dispatched`, which PR 21 co-designs.** `Dispatched` is the HTTP build's PRs
1 through 9, and D8 changes what its `Upgrade` variant carries. Shipping that variant as
`Arc<WsConnectionCallbacks>` and then replacing `WsConnectionCallbacks` breaks one enum twice in a
release, so PR 21's type is decided in the same conversation as the HTTP build's, not after it.

**PR 17 before PR 18.** The trait settles what `send` answers; the wire type is then written once
behind it rather than twice.

### Phase 3 — the SPI, co-designed with the HTTP build

| # | PR | Closes | Needs |
| --- | --- | --- | --- |
| 21 | Core owns the connection loop: `WsConnectionCallbacks` becomes a transport trait, `Dispatched::Upgrade` carries `WsUpgrade`, `MessageCallbackResult` collapses into `Items` plus a task handle, reply streams merge into core's loop, the loop is written once | F215, F212, F204 | D8, and the HTTP build's `Dispatched` |

**This is not the last PR in the sequence, it is the one the others are shaped by.** Numbered 21 for
reading order; scheduled alongside the HTTP build's PR 1–9, because the two decide one enum. Rows 8
and 13 sit behind it and row 7 does not.

**The bound is proven and the probe is kept.** `spike-d8-transport-bounds.patch` in the session
scratchpad asserts `Send + 'static` over `SplitStream<T>` and `SplitSink<T, M>` for all five socket
types; all five pass, and the same assertion over `std::rc::Rc<()>` fails to compile, so the bound is
a result rather than a vacuous one. Re-run it as the first commit of this PR before writing anything
that depends on it.

**What disappears, as the measure of whether it worked.** Five copies of `run_ws_connection`; five
copies of `TokioSender`, four of them `pub`; `stream_tasks` / `JoinHandle` / `.abort()` from five
files; 16 `tokio::spawn` calls on the WebSocket path, leaving core spawning nothing as it does today.

### Not in the sequence

**`permessage-deflate` (F209) is PR 20 and nothing more.** ulo cannot implement it above the library:
`tungstenite` rejects any frame with a reserved bit set and offers no negotiated-extension knob, so
compression has to live in the frame codec, and on four of the five adapters the host framework owns
that codec. Upstream, `snapview/tungstenite-rs` issue #2 has been open since 2017 with three
implementations — two closed unmerged, one open 28 months — and a maintainer objection that
compression is not WebSocket-specific. The work exists; acceptance does not. PR 20 declares the
position so a reader can tell a decision from an oversight, and the trigger to revisit is that PR
being accepted, which reaches all five adapters at a version bump provided PR 13's negotiation point
is built.

**F210's type split is PR 18 and its timing is settled.** D7 takes it now rather than deferring to
the first new variant, because PR 4 adds `#[non_exhaustive]` and the whole point of the split is that
the wire stops moving when the enum does.

## Verification each PR needs

Each row's verification is the probe case that produced its finding, promoted from `println!` to an
assertion. The mapping:

| PR | Case in `ws_wire_probe.rs` | Asserts |
| --- | --- | --- |
| 2 | `probe_refusal_frames`, the `WsError::Refused 4406` row | a close frame carrying `4406`, not an envelope |
| 3 | `probe_wsmessage_serde_shape` | the four literal JSON shapes |
| 5 | a new unit test | `Session::default()` does not compile |
| 7 | `probe_close_handshake` | the echo's bytes on all five, axum included as the control |
| 8 | `probe_raw_bytes_after_error` | a status code on each of the five error paths |
| 21 | `spike-d8-transport-bounds.patch`, then the whole suite | the bound holds; every case in §12 passes unchanged on all five |
| 10 | `probe_refusal_frames`, the `1005` and `7` rows | the constructor refuses, so no such frame reaches the wire |
| 11 | a new case | the code each `ErrorKind` produces |
| 12 | `probe_payload_limits` | the ceiling is the configured number, and breach answers `1009` |
| 13 | `probe_subprotocol`, all four header shapes | one token selected from the client's list, on all five |
| 14 | a new case | a binary frame reaches a handler through a gateway's own `route` |
| 15 | `probe_origin` | a disallowed origin is answered `403` before the upgrade |
| 16 | `probe_after_close_and_handshake`, the handshake row | `remote_addr` is `Some` |
| 17 | a new case | one `dyn Broadcast` call site compiles against both services |
| 19 | `probe_after_close_and_handshake`, the handshake row | `room=front desk`, `tag=a+b` |

PR 7's assertion has its control built in: axum writes the echo bytes today and the other four do not,
through the same reader. A run where all five report empty has broken the reader, not found a
regression. **Assert against a known violation before believing a clean run** —
`feedback-self-test-the-checker`, and §12 of the memo records the case where this probe's first
close-echo result conflated a timeout with an EOF.

## The branch

No branch exists. This audit changed nothing; `git status` is clean at `654c7183`.

No ADR is owed by Phase 1 or Phase 2 — D3 rides `HTTP_AUDIT.md`'s ADRs rather than adding one, and
D4, D5, D6 and D7 are changes inside decided surfaces. **D8 owes one**: it replaces the WebSocket
adapter SPI, and it should be written beside the HTTP build's rather than after them, since the two
decide `Dispatched` together. Number it when written, since 0052, 0053 and 0054 are reserved by the
HTTP build and the GraphQL build takes the next free pair.

## The divergence gate — stop and ask

Approval of this build is approval of **what was read and understood**. If the build has to depart
from that, it pauses and reports rather than proceeding on judgement.

What counts as divergence:

- A decision in §13 turning out to be unbuildable as written.
- A socket type whose split halves lose `Send + 'static` under a dependency bump, which falsifies
  D8's spike and stops PR 21.
- A sixth adapter behaviour the probe did not measure — the twelve cases are twelve, not exhaustive.
- A library where the extra poll in PR 7 does not flush the queued reply, or where a protocol error
  does queue one — either falsifies the mechanism §1 rests on.
- `Dispatched::Upgrade` turning out not to accept a subprotocol slot, which is D3's remaining half.
- A finding that reorders the sequence, makes a PR unnecessary, or adds one.
- `permessage-deflate` turning out to be reachable after all, which changes F209 from a record to a
  PR.

What does not: naming details, file placement, test names, the order of two independent PRs.

On divergence: stop, state what the design said, what the tree requires, and what the options are.
Wait.

## The Law

Per `feedback-the-law-review-protocol` in memory: **before every commit message and every PR title
and body**, a dedicated agent reviews the text against `~/.claude/commits.md`,
`~/.claude/pull-requests.md`, `~/.claude/prose-discipline.md` and `~/.claude/posture.md`.

Run it **in the background** and surface only its findings. Add per invocation: the branch and
commit, an instruction to read the diff in full, a numbered list of the technical claims to verify
hardest, and how many prior passes found factual errors. Ask for both facts and register; do not tell
it the register scan is reliably clean.

The claims worth verifying hardest in this build are the spec citations. A PR body asserting what RFC
6455 requires is a claim about a document the reviewer has to fetch, and this audit found one such
claim already in the tree and wrong.

## Per-PR checklist

Walk it — the failure mode on record is a checklist written and not walked. It lives in
`feedback-the-law-review-protocol`. The three items most often missed here:

- **Production text this change falsifies** — doc comments, ADRs, and the workspace `CLAUDE.md`,
  which sits in the untracked parent where a repo-wide grep misses it. The adapter table there
  records WebSocket support per adapter and says nothing about subprotocols; PR 11 falsifies it.
  **ADR-0036** reasons from the post-upgrade constraint PR 10 removes.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs, and clippy
  cannot pass workspace-wide (F80).
- **One `cargo test` at a time.** The separate-port WebSocket tests bind hardcoded ports and
  `#[serial]` locks only within a process — `feedback-one-cargo-test-at-a-time`. The probe's
  tungstenite cases bind `19771`.

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest number there. Do not
move `Last synced HEAD` or the `Docs status` section in `CLAUDE.md` — those are a documentation-site
audit's to move. Append one line per merged change to `DOCS_AUDIT.md` §18 naming the pages it
invalidates.

Findings about the pipeline rather than the wire belong to the pipeline audit's pass, not this one,
unless the build surfaces them — in which case they are this session's and file here.
