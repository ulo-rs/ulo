# Handoff — building what the RPC wire audit found

For the session that implements it. The plan is `RPC_AUDIT.md`; nothing here restates it.

## Status

**D1–D8 are settled**, answered by the user on 2026-09-23. `RPC_AUDIT.md` §14 records each with the
alternatives rejected and why. **They are not open**, and a recommendation kept beside one is its
argument rather than a choice still to make.

Three of them moved after they were first written, and the movement decides this sequence:

- **D1 gained a mechanism the options did not carry.** It is `(b)` — `RpcData` narrows to
  `Json | Binary` — **plus `(d)`, a capability declaration** the user proposed: transports say what
  they can carry. The two are complementary. A capability cannot recover a tag the wire never wrote,
  so the type still narrows; and the narrowing needs redis to stop tagging its envelope's `data`
  field, or the matrix stays non-uniform. §15 carries the shape.
- **D4 folded into D1's (d).** `max_frame` is a declared capability, not a bespoke tcp builder. The
  default is **1 MiB**, approved 2026-09-23 and reasoned in the memo; it is one constant.
- **D8 was answered against this memo's own recommendation, by a probe.** Shedding past a
  high-water mark was the memo's first answer; the measurement shows the producer runs at full speed
  regardless of the consumer (~38 MiB/s unbounded), which makes a mark a timeout in disguise. Credit
  is **scheduled as PRs 18 and 19**, not filed with a trigger.
- **D7 is new and was raised by the user**: request-side streaming rides at the **SPI and grammar**
  level and not as a feature. The callbacks signature takes `Items<RpcData, RpcError>` and every
  adapter passes `Items::One` on the day it merges. The two call shapes are a follow-on build.

**Two rows in an earlier revision of this sequence no longer exist as written.** PR 8's bespoke
`with_max_frame` and the standalone payload change are both absorbed by the capability SPI. Build
the SPI first.

**One sequencing claim made before D1 was settled is now wrong.** An earlier revision of
`PROJECT_SEPTEMBER_SMILE/README.md` said this build blocks on nothing in the set. Phase 1 still
does not. **Phase 2 rides the HTTP build's PRs 1-9**, because D1's (d) is `AdapterCapabilities` on a
third transport and must be co-designed with it rather than invented twice.

## Read first, in this order

1. `RPC_AUDIT.md` §14 — eight decisions, all settled, each with its rejected alternatives. **Read D8's probe table before PR 18** — it is the reason credit is scheduled rather than a bounded queue, and it refutes this memo's own first recommendation.
2. `RPC_AUDIT.md` §15 **before writing any code** — what both SPIs look like after the collapse, and
   the measurement that says core owns the *sequence* while the adapter keeps the publish. The four
   per-message dispatch functions measure 43-77 units apart, at or past §4's split line, so a
   collapse that tries to merge them wholesale is fighting the numbers.
3. `RPC_AUDIT.md` §1, §2 and §6 — the three spines: the payload that is three types, who receives a
   message, and the cancel notice that never fires for a single call. Most of the eighteen findings
   reduce to those three plus the copies in §12.
4. `RPC_AUDIT.md` §11 — the four broker conventions against their own documents, and why every gap
   found is on an axis none of them covers.
5. `FRAMEWORK_GAPS.md` **F216–F234** for what this closes, plus **F63** (whose count F223 corrects),
   **F46** (the string-match dispatch F226 is a second consequence of) and **F1** (the extraction
   failure that renders `Internal`).
6. `DOCS_AUDIT.md` §17 **A5** and §22 — the broker adapters that panic inside the serve future after
   `bind()` reported success. This audit confirms the wider reading and counts the sites: nats 3,
   redis 2, rabbitmq 8, kafka 4, mqtt 0. It is not refiled here, and F231's third failure mode is the
   same defect reached through a pattern name.
7. ADR-0032 — the grammar this build extends. Read its **Consequences** section: it is where the
   client-side buffering question (F229) is absent rather than answered.
8. `project-the-spec-conformance-seam` in memory — the three-category test. §11 places every gap
   this build closes in category two: the protocol leaves it open and ulo has to choose once.

**Fetch the specs rather than recalling them.** AMQP's own class definition is the one that decides
whether §11 is a conformance question at all, and it says *"no formal behaviour"* — an implementer
working from RabbitMQ's tutorial alone will read a convention as a requirement.

```
curl -sS https://docs.oasis-open.org/mqtt/mqtt/v5.0/os/mqtt-v5.0-os.html
curl -sS https://www.rabbitmq.com/resources/specs/amqp0-9-1.xml
```

## The probe is the suite

Two files in `artifacts/` beside this handoff, both promotable as they stand. They were copied
out of the session scratchpad on 2026-09-23 — that directory is session-scoped and under
`/private/tmp`, so anything a build needs lives here instead:

- **`probe-cases.rs`** — three cases written against `ulo_rpc_conformance::Broker` and stamped
  across all five broker suites through the existing `conformance_suite!(B, cases: [...])` form.
  They produced §1's, §2's and §3's tables. Promoting them is appending them to
  `crates/ulo-rpc-conformance/src/lib.rs` and adding their names to the macro's default case list.
- **`rpc_wire_probe.rs`** — six cases for tcp and udp, which need no broker. It lived at
  `integration-tests/tests/integration/rpc_wire_probe.rs` with one line in `main.rs`.

`artifacts/probe-output-brokers.txt` beside them is the run §1, §2 and §3's tables were read off.

A third file, **`artifacts/rpc_backpressure_probe.rs`** with `artifacts/probe-output-backpressure.txt`,
is D8's: two cases over tcp, in-process, no broker. It is the verification for PRs 18 and 19 and it
carries its own control — a consumer reading flat out holds a gap of 2 while a slow one grows
linearly, so a run where both grow has broken the harness rather than found a regression.

**The suite passes and nothing runs it.** All five brokers passed all six existing cases this
session, first time, against master `654c7183`. That is the baseline every PR here is measured
against, and D6 is the decision about whether anything will notice when it stops being true.

## The sequence

### Phase 1 — free-standing, nothing to wait for

Eight PRs, independent of each other and of every other build in the set. They close eight findings
without touching either SPI, so they can run while the capability type is being co-designed.

| PR | Closes | What |
| --- | --- | --- |
| 1 | F217 | `data_to_json` refuses a `Binary` payload on tcp and udp instead of sending `Value::Null`. Two call sites, `RpcClientError::Transport` before any I/O. The refusal moves into the capability at PR 9; shipping it here means the data loss stops now rather than behind a co-design. |
| 2 | F225 | The two client-lifecycle docs. `RpcClientTransport::connect`'s stops claiming an automatic call; `RpcClient::close`'s stops claiming a flush six transports do not perform. Doc-only; the flush is PR 11. |
| 3 | F232 | `ulo::rpc::wire`'s and `RpcError`'s lane doc states what the lane *means* — a call that reached no controller — rather than enumerating three variants that travel it. |
| 4 | F233 (half) | `Broker` for `ulo-rpc-tcp` and `ulo-rpc-udp`: `start()` binds a port, `disrupt()` drops the connection. The suite then claims seven transports and tests seven. |
| 5 | F233 (half) | The three probe cases promoted into `conformance_suite!`'s default list, as assertions rather than `println!`. **Run PR 4 first** — they must pass on tcp and udp before they become a gate. |
| 6 | F230 | Kafka's `Timeout::After(Duration::from_secs(5))` becomes `self.timeout` on the client and the remaining budget on the server's reply publish. |
| 7 | F226 | `create` refuses two controllers declaring one pattern, naming both and the pattern. The refusal goes where the module-identity clash goes (ADR-0029), not at `bind`. |
| 8 | F227 | A stable Kafka reply topic per logical client rather than per process, plus partition and replication knobs on `ensure_topics`. |

### Phase 2 — the SPI, co-designed with the HTTP build

**PR 9 is the gate for everything below it.** It introduces `RpcCapabilities` and it is the same
`#[non_exhaustive]` shape as the HTTP build's `AdapterCapabilities` — one type, one ADR, three
transports. Do not write a second capability type; if the HTTP build's PRs 1-9 have not settled
theirs, stop and co-design rather than guessing.

| PR | Closes | What |
| --- | --- | --- |
| 9 | D1(d), part of F220, F231 | `RpcCapabilities` on both SPIs, read at `bind()`: payload support, `max_frame`, delivery modes, pattern-name rules, streaming support. Enforcement in core, declaration in the adapter. Owes the ADR, written with HTTP's. |
| 10 | F216 | `RpcData` narrows to `Json \| Binary`; `RpcData::text` becomes a constructor producing `Json(Value::String)`; **redis stops tagging its envelope's `data` field** while keeping the envelope for `reply_to`. `bytes_to_data`/`data_to_bytes` collapse into `ulo::rpc::wire` in the same PR — they are 0 units apart and they are what this changes. |
| 11 | F221, F222, F225, F228, F229 | **The client collapse.** `RpcClientTransport` becomes the carrier of §15 — `dispatch` plus `inbound` plus `capabilities` — and core takes correlation, the pending map, the timeout, the gap deadline, `forward_stream`, **the cancel notice on a `send` timeout (D3)**, and the queue that D8's credit replaces (F229 — leave the queue as it is here; PRs 18 and 19 bound it). Seven `forward_stream` copies become two, and NATS keeps its own. |
| 12 | F221, F224, F228 | **The server collapse.** Core takes the `catch_unwind`-and-frame sequence, `drain_tasks`, the concurrency bound and the `{pid}-{nanos}` helper; each adapter keeps its publish. Read §15's measurement first: the four `handle_*` functions are 43-77 units apart and are **not** to be merged wholesale. `basic_qos` on rabbitmq is set from the same bound. |
| 13 | D7, **F234** | `RpcMessageCallbacks` becomes a **trait** with one `async fn`, taking `Items<RpcData, RpcError>`; every adapter passes `Items::One`. The inbound frame grammar is reserved in `ulo::rpc::wire` beside the reply grammar. Nothing changes on the wire and no call shape appears. **This is the PR that stops the SPI being settled twice** — it is cheap only if it rides with 11 and 12. |
| 14 | F218 | **D2.** A declared delivery mode, adapter default with a per-pattern override, `balanced` for `#[message_pattern]` and `broadcast` for `#[event_pattern]`. nats gains a queue group, mqtt a `$share/{group}/` prefix, rabbitmq and kafka a per-instance queue or group for broadcast. Redis refuses `balanced` as a capability mismatch at `bind()`, naming the transport — that refusal is the feature. Touches `ulo-macros`, which nothing else here does. |
| 15 | F220 | tcp's `max_frame` enforced against the declared capability, default **1 MiB**, `TcpAdapter::with_max_frame` overriding. Every other transport declares the ceiling it already inherits. |
| 16 | F231 | Pattern-name validation against the declared rules, plus the reserved-name check for the four cancel channels, which stays in core. Surfaces as `StartupError::Adapter` under ADR-0024's refuse-or-none bind. |
| 17 | F223, F63 | One word covering three sites, plus the `ErrorKind` question an oversize payload raises. `udp_adapter.rs`'s single-reply branch starts writing the error frame its stream branch already writes. **Run after PR 12**, which decides what a shed load on a broker answers. |

| 18 | F229, D8 | **Per-call credit, core and the two socket transports.** A `{"id":…,"credit":N}` control frame in `ulo::rpc::wire`; `drive_reply_stream` waits for credit before sending an item; `forward_stream` issues more as the caller consumes. tcp and udp carry it in band on the flow that already carries the cancel notice. **Run after 11 and 12** — both halves are one copy each only once those land. |
| 19 | F229, D8 | **Credit on the brokers, and the declared fallback.** nats, redis and mqtt get a per-call control address (subject, channel, topic — all free in their namespaces). **kafka and rabbitmq declare through `RpcCapabilities` that they cannot**, a per-call topic or queue being a durable object and F227's disease, and fall back to a bounded queue with shed-and-cancel. **The cancel goes before the error**, or `ended = true` suppresses it (§14 D8). |

**Phase 2 order is 9 → 10 → {11, 12} → 13 → {14, 15, 16, 17} → 18 → 19.** PRs 11 and 12 are the two SPIs and
are independent of each other. PR 13 wants both landed so the signature change touches collapsed
code rather than seven copies, and PRs 18 and 19 want them landed for the same reason. **Credit
is built before Phase 3**: a client streaming a request needs the server to bound it, which is the same
mechanism in the other direction, so the call shapes reuse it rather than inventing a second
answer.

**Why the trait conversion belongs inside PR 13 and nowhere else (F234).** `RpcMessageCallbacks` is
a boxed closure behind a struct — the same shape as `WsConnectionCallbacks`, which is why F215's
*"odd one out"* wording is half right: the four adapters are traits, the two dispatch callbacks are
not. The cost of that shape is growth, and PR 13 is the growth: it edits the field type, the
constructor bound, the method and seven call sites for D7 anyway. Converting there costs nothing
extra; converting anywhere else costs a second pass over the same sites.

**Do not borrow the rest of the WebSocket answer.** D8 removed WS's callbacks from the SPI because
core took the connection loop, and that worked because those five loops measure 1-40 units apart.
The seven RPC receive loops measure **83-257** (tcp/udp 83, kafka/nats 257, at `654c7183`) against
§4's 35-45 split line. The receive loop stays with the adapter; only the dispatch call becomes a
trait. §15 has the reasoning, and the client-side inversion in PR 11 is where the D8 shape *does*
apply, its loops having measured 0-23.


### Phase 3 — the two call shapes

**In the sequence rather than filed beside it**, by the user's call on 2026-09-23: D7 reserves the
signature in PR 13 and the shapes themselves are what make it worth reserving. Phase 3 is a separate
approval — it adds a feature where Phases 1 and 2 close defects — and it does not start until
Phase 2 has landed, because every one of these PRs writes into collapsed code.

| PR | What |
| --- | --- |
| 20 | **The inbound grammar.** Request item frames and a request-end marker in `ulo::rpc::wire`, beside the reply grammar PR 13 reserved the space next to, plus per-transport correlation of a multi-frame request. `RpcCapabilities::streaming` is enforced here: **udp declares it cannot order inbound frames** and the refusal is a capability mismatch at `bind()`, not a runtime surprise. |
| 21 | **The handler-side extractor.** A `FromContext<RpcContext>` holding the request stream. **Read gRPC's `Inbound<T>` (ADR-0043) before designing it** — that is the same question already answered on the one transport that has request streaming, and a second shape for RPC would be answering it twice. Macro surface: `#[patterns]` has to accept it the way `#[grpc_methods]` accepts `Inbound<T>`. |
| 22 | **Client-streaming.** Many request frames, one reply. `RpcClient` gains the verb; the carrier's `dispatch` takes a stream where PR 11 left room for one. |
| 23 | **Bidi.** A request stream and a reply stream on one correlation. Depends on 22 and on nothing else. |

**Owes one ADR**, covering both shapes and the inbound grammar. PR 13 owes none, which is what makes
it cheap to ship first.

**What this is most at risk of.** Not being built — the reason it moved into the sequence. It is
recorded in three places (`RPC_AUDIT.md` §14 D7, this table, and
`project-rpc-request-side-streaming` in memory), and the trigger to start is Phase 2 merging.

### Not in the sequence

- **Carrying the caller's deadline on the wire.** F222's second half. A grammar change on seven
  transports, and gRPC already does it differently. File it with a trigger; do not build it because
  PR 11 is nearby.
- **F219's answer.** Three transports structurally cannot raise `Unrouted`, and no code change makes
  them. What is owed is honesty in `CLAUDE.md` and the `Unrouted` doc about which transports can —
  which is a documentation change this build should make and not a PR of its own.
- **A5's panics.** Already filed in `DOCS_AUDIT.md` §17, already known to be wider than filed. A PR
  that fixes them is welcome and belongs to whoever takes A5, not to this sequence.
- **JSON-RPC 2.0.** `STANDARDS_AUDIT.md` §5 and F65 hold it. Nothing here bears on it except to
  confirm that the three things the grammar carries which JSON-RPC does not — stream items, end
  markers, cancel notices — are all load-bearing.

## Verification each PR needs

Each row's verification is the probe case that produced its finding, promoted to an assertion.

| PR | Case | Asserts |
| --- | --- | --- |
| 1 | `rpc_wire_probe.rs`, the `Binary` rows | `send` returns `Err`, and no frame reaches the socket |
| 4, 5 | the whole suite on seven transports | the six existing cases pass on tcp and udp unchanged |
| 6 | a new case | a call with `with_timeout(500ms)` returns inside its budget when the broker refuses writes |
| 7 | `probe_duplicate_pattern` | `create` returns `Err` naming both controllers |
| 8 | a new case | a client restarted twice reuses one reply topic |
| 9 | a new case | a capability mismatch fails `bind()` naming the transport and the thing it cannot do |
| 10 | `probe_payload_variant` | one column per transport, every row identical across all seven |
| 11 | a new case | the server's cancellation token fires when a `send` times out; a reply produced before `close()` reaches the caller |
| 12 | a new case | the drain completes on all five brokers; `basic_qos` visible on the rabbitmq channel |
| 13 | the whole suite | every case passes unchanged — the signature moved and the wire did not; and an adapter outside the workspace compiles against the trait |
| 14 | `probe_two_instances_one_emit` | the declared mode on all five, and redis refused at `bind()` |
| 15 | `probe_tcp_unterminated_line` | the connection is refused at the configured byte, not at 64 MiB |
| 16 | a new case | a Kafka-illegal pattern fails `bind()` rather than the serve task |
| 17 | a new case | every status the wire writes is an `ErrorKind::name()` |
| 18 | `artifacts/rpc_backpressure_probe.rs`, promoted | the slow-consumer gap **plateaus** instead of growing linearly, and the producer's rate drops below the control's — the control's flat gap of 2 is what proves the harness still measures buffering |
| 19 | the same probe on all five brokers | plateau on nats, redis and mqtt; on kafka and rabbitmq a bounded gap and a shed that carries its cancel **before** its error |

**Assert against a known violation first.** `probe_payload_variant` reports what it sees rather than
what it expects, so a run where every column agrees may mean the wire was fixed or may mean the
handler stopped reporting the variant. Break one transport deliberately before believing a clean
matrix — `feedback-self-test-the-checker`.

`probe_two_instances_one_emit` has its control in the result: rabbitmq and kafka answer 1 today and
nats, redis and mqtt answer 2, through the same counter. A run where all five agree before PR 14
has broken the harness.

## The branch

No branch exists. This audit changed nothing; `git status` is clean at `654c7183`.

**ADRs owed, three of them.**

- **The capability SPI (PR 9).** Written *with* the HTTP build's, not after it: one
  `#[non_exhaustive]` type across HTTP, WebSocket and RPC. If that ADR already exists when this
  build starts, PR 9 extends it rather than adding a second.
- **What `RpcData` means on the wire (PR 10).** It supersedes part of ADR-0032's payload
  assumptions. ADR-0032 is accepted and is not retro-edited — a later ADR gets a pointer under its
  title, per `project-adr-refinement-convention`.
- **Delivery cardinality (PR 14).** A new contract, and it changes a macro surface.

D3, D4, D5, D6 are changes inside decided surfaces and owe none. **D7 owes none from this build** —
PR 13 reserves a signature and decides nothing a caller can see; the follow-on that adds the call
shapes owes one.

Number them when written: 0052, 0053 and 0054 are reserved by the HTTP build and the GraphQL build
takes the next free pair.

## The divergence gate — stop and ask

Approval of this build is approval of **what was read and understood**. If the build has to depart
from that, it pauses and reports rather than proceeding on judgement.

What counts as divergence:

- A decision in §14 turning out to be unbuildable as written.
- **The HTTP build's `AdapterCapabilities` settling on a shape RPC cannot use.** PR 9 is a
  co-design, so this is the likeliest divergence in the build and the one that must not be resolved
  by inventing a second type.
- Redis turning out to have a competing-consumer mode after all, which changes D2's shape.
- A broker whose client library will not carry the tagged envelope without a second allocation per
  call, which changes D1's cost.
- A seventh or eighth transport behaviour the probe did not measure — the nine cases are nine, not
  exhaustive, and they ran against one broker version each.
- `ulo-rpc-conformance` cases starting to fail for a reason unrelated to the PR, which means the
  baseline in this file is stale and has to be re-established before anything is believed.
- A finding that reorders the sequence, makes a PR unnecessary, or adds one.

What does not: naming details, file placement, test names, the order of two independent PRs in
Phase 1.

On divergence: stop, state what the design said, what the tree requires, and what the options are.
Wait.

## The Law

Per `feedback-the-law-review-protocol` in memory: **before every commit message and every PR title
and body**, a dedicated agent reviews the text against `~/.claude/commits.md`,
`~/.claude/pull-requests.md`, `~/.claude/prose-discipline.md` and `~/.claude/posture.md`.

Run it **in the background** and surface only its findings. Add per invocation: the branch and
commit, an instruction to read the diff in full, a numbered list of the technical claims to verify
hardest, and how many prior passes found factual errors. Ask for both facts and register; do not
tell it the register scan is reliably clean.

The claims worth verifying hardest in this build are the cross-transport ones. A PR body saying
"every transport now does X" is a claim about seven crates, and this audit found three such claims
already in the tree and wrong — the lane doc (F232), `connect`'s doc (F225) and the conformance
crate's own opening line (F233).

## Per-PR checklist

Walk it — the failure mode on record is a checklist written and not walked. It lives in
`feedback-the-law-review-protocol`. The four items most often missed here:

- **Production text this change falsifies** — doc comments, ADRs, and the workspace `CLAUDE.md`,
  which sits in the untracked parent where a repo-wide grep misses it. Its **RPC transports** table
  records per-transport behaviour and PRs 8, 10, 11, 15 and 16 each falsify a row; its **Wire
  semantics** paragraph is what PR 14 changes.
- **ADR-0032** is the record PRs 9, 12 and 13 reason against. It is accepted and is not retro-edited
  — `project-adr-refinement-convention`. A later ADR gets a pointer under its title.
- **One check in its plainest form, with no flags.** `--all-features` hides `cfg` bugs, and the five
  broker suites compile only under `--all-features` or an explicit `--features integration`, so a
  plain `cargo check --workspace` proves nothing about them —
  `feedback-all-features-hides-cfg-bugs`.
- **One `cargo test` at a time**, and one broker suite at a time. Each case starts its own
  container; five suites in parallel is five brokers per case —
  `feedback-one-cargo-test-at-a-time`.

Merge with `--merge`, never `--squash`.

## Bookkeeping

New findings go to `FRAMEWORK_GAPS.md` as `F<n>`, continuing from the highest number there. Do not
move `Last synced HEAD` or the `Docs status` section in `CLAUDE.md` — those are a documentation-site
audit's to move. Append one line per merged change to `DOCS_AUDIT.md` §18 naming the pages it
invalidates; the microservices pages describe per-transport behaviour and most of Phase 2 touches
one.

Findings about the pipeline rather than the wire belong to the pipeline audit's pass, not this one,
unless the build surfaces them — in which case they are this session's and file here.
