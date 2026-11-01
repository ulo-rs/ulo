# Project September Smile

Every outstanding audit of the framework, grouped so one triage pass can read them together and
decide a build order across the set rather than per audit.

## Layout

One folder per subject, holding that subject's handoff (the brief it was run under), its memo (the
plan) and its build handoff (the PRs). A folder with only a handoff has not been run yet.

**Each subject folder has its own `artifacts/`, holding what that pass produced that is not prose** —
probes, spikes, measurements, probe output. `<subject>/artifacts/`, never a shared one at this level:
an artifact is evidence for one memo, and the folder sits beside the memo that cites it. Added
2026-09-23, because until then six handoffs pointed at files in the session scratchpad, which is
session-scoped and under `/private/tmp`, so a build session would have had to `find` for them and a
reboot would have ended them. A pass that produces a probe copies it into its own subject's
`artifacts/` before it finishes.

| Folder | Handoff | Memo | Build handoff | State |
| --- | --- | --- | --- | --- |
| `di/` | yes | yes | yes | audited 2026-09-21/22 at `654c7183`; D1–D14 settled (D14 added 2026-09-22) |
| `pipeline/` | yes | yes | yes | audited 2026-09-21 at `654c7183`; D1–D5 settled |
| `http/` | — | yes | yes | audited 2026-09-20 |
| `extraction/` | yes | yes | yes | audited 2026-09-22 at `654c7183`; D1–D7 settled |
| `render/` | yes | — | — | **run and spent 2026-09-23 at `654c7183`**; no structural question, so no memo by decision. F242 and F243 filed, F22/F64/F69/F87 appended, A24 and C22 found already closed. Its handoff carries the dispositions |
| `performance/` | yes | — | — | not run; runs last by its own brief |
| `graphql/` | yes | yes | yes | audited 2026-09-22 at `654c7183`; F182–F195 filed, D1–D8 settled; ten PRs in three phases |
| `grpc/` | yes | yes | yes | audited 2026-09-23 at `654c7183` in three passes; F235-F241 filed, F63 and F238 corrected, **D1-D4 settled**; **build approved 2026-09-23** — eight PRs in two phases, F240 named explicitly by the user. D4 and D2's anchor are executed by the pipeline build. §6a measures gRPC against the other six audits' settled decisions |
| `rpc/` | yes | yes | yes | audited 2026-09-23 at `654c7183`; F216–F234 filed, F63's count corrected, **D1–D8 settled**; 23 PRs in three phases; Phase 2 co-designed with the HTTP build and ending in per-call credit, Phase 3 the request-streaming call shapes |
| `ws/` | yes | yes | yes | audited 2026-09-22/23 at `654c7183`; F196–F215 filed, F67 closed, D1–D8 settled (D8 spiked on all five adapters) |

## What stays outside this folder, and why

- **`FRAMEWORK_GAPS.md`** and **`DOCS_AUDIT.md`** — the two ledgers. Every audit here files into one
  of them and every build handoff cites them by number, so they belong to no single subject.
- **`CLAUDE.md`** — the framework reference, read every session.
- **`AUDIT3_DISPATCH.md`** and **`STANDARDS_AUDIT.md`** — spent audits kept for method rather than for
  pending work. `AUDIT3_DISPATCH.md` §4's measurement rule is cited by the build handoffs here and
  has now decided three questions, most recently which of four field resolvers are copies of each
  other.

Files in this folder reference each other and the files above by bare filename, not by path, so
nothing here depends on the layout staying as it is.

## What is owed

**The triage pass ran on 2026-09-24. Its output is `BUILD_SEQUENCE.md` beside this file** — the
root order across the eight builds, the state of every row, the sign-offs the build waits on, the
loop each row follows, and the halt log. It is the one control surface for the build; the memos
and handoffs are frozen inputs to it. The couplings below were the triage's input and are carried
there row by row.

The brief that pass ran under, kept as written: it reads every memo and build handoff here, decides
the order the builds run in and which can run together, and records the couplings that cross
subjects. Eighteen are already known, and none is resolvable inside one folder:

- The DI audit's **D2** — a token is single-bound unless declared multi, and a clash is refused —
  belongs in the pipeline build's **PR 5**, which replaces the globals registration surface. Shipped
  without it, that PR reopens F138.
- The DI audit's **F73** is the pipeline build's **PR 12**, and the pipeline build's Phase 2 changes
  trait signatures the DI macros emit.
- The extraction build's **PR 4** needs the pipeline build's **PR 7**. 415 is reachable only through
  the `Custom { status, message }` variant F136 introduces, and that variant does not exist yet.
- The extraction build's **PR 14** and ADR-0052 touch one function. `Path`'s struct branch round-trips
  through whichever query-parsing crate that build picks (`extract/path.rs:70-72`), so F117's swap
  decides F174's rewrite.
- **F180 and F181 were filed by the extraction audit and are built by the DI build**, in its group F
  behind **D14**. Deleting `ulo::http::Request` needs injectables to be shared first, so the
  extraction build does not wait on them and they do not wait on it. D14 itself was raised by the
  extraction pass and lives in `DI_AUDIT.md`, because it is a statement about what a provider hands
  out rather than about extraction.
- The GraphQL audit's **F182/F184** and the extraction audit's **F163** are the same argument on
  two surfaces — a status seam answering one code where a standard gives three — and should be read
  together. They are **not** blocked alike: F163 waits on **F136**'s `Custom { status, message }`
  because the status is a literal inside a macro, while a GraphQL controller builds its
  `HttpResponse` by hand over a bare `u16`. An earlier revision of this file said F184 queued behind
  the pipeline build's **PR 7**; it does not, and only the extraction build's **PR 4** waits there.
- The GraphQL audit's **F191** is the DI audit's ambiguity finding seen from outside. A fixed token
  string makes "first import wins over an `FxHashSet`, reporting nothing" certain rather than
  possible. Its **D6** puts the engine on the module and drops three type parameters, so the token in
  its present form stops existing — the two builds should not both write a fix for it.
- The GraphQL audit's **D7** rides the HTTP build's **PR 1**. That PR introduces
  `AdapterCapabilities`; the GraphQL engine SPI takes the same `#[non_exhaustive]` shape rather than
  inventing a second one, and should follow whatever that PR settles on.
- The GraphQL audit's **F190** has an adapter half, and the `ws/` pass filed it as **F198** and
  **F199**: three HTTP adapters read a list-valued `Sec-WebSocket-Protocol` as one token and agree to
  any single token offered, and rocket and tungstenite negotiate no subprotocol at all. The gateway
  half is one comparison and the adapter half is that pass's **D2**, which is open. They are one fix
  and should be sequenced together.
- **The `ws/` build's Phase 2 consumes the HTTP build, and must not precede it.** `Dispatched` and
  `AdapterCapabilities` are the HTTP build's PRs 1–9, and the WebSocket PRs 13, 15, 16 and 20 mount
  on them. The WebSocket pass's D3 was first written as a new adapter SPI costing five crates,
  because that pass had not read `HTTP_AUDIT.md` §1; it is a subprotocol slot on
  `Dispatched::Upgrade` plus `accept_hdr_async` on the separate-port path. Sequencing the two the
  other way designs one seam twice.
- **The `ws/` pass's D8 and the HTTP build decide one enum, so they are co-designed.** D8 is settled:
  core owns the WebSocket connection loop, and `Dispatched::Upgrade` carries a `WsUpgrade` — naming
  the selected subprotocol and accepting the transport — rather than `Arc<WsConnectionCallbacks>`.
  Shipping that variant with the callbacks type and then replacing the callbacks type breaks one enum
  twice in a release. The WebSocket build's **PR 21** belongs in the same design conversation as the
  HTTP build's **PRs 1–9**, not behind them, and it is also what answers D3's remaining half. Its
  bound was spiked on all five adapters and falsified against `Rc<()>`.
- **The GraphQL build's PR 6 is blocked by the `ws/` build's PR 2.** F187 routes graphql-transport-ws's
  five close codes through `WsError::Refused`, and on the message path that variant writes the
  canonical envelope and no close frame (**F201**) — the inverse of what it documents. Until PR 2
  merges, F187's fix writes envelopes where it means to write closes.
- **Four query parsers is five.** The WebSocket handshake's query map is the fifth site (**F206**) and
  the only one that does not percent-decode at all. Whoever settles the crate settles it for five.
- **Four query parsers, three owners.** `Query<T>`, the `#[query]` marker's `Query<HashMap<_, _>>`,
  `Path`'s round trip and `Request::from_parts` each parse a query string and they disagree (F117,
  F180). ADR-0052 picks the crate, the extraction build owns the first two, the DI build owns the
  fourth. Whoever runs first should settle the crate for all of them.
- **The RPC audit's F226 is the DI audit's ambiguity finding at a fourth site.** `pattern_map.insert`
  in `make_rpc_callbacks` answers "which controller owns this pattern?" with whatever the last insert
  wrote, reporting nothing — which is `DI_AUDIT.md`'s own summary of its central finding, on the RPC
  dispatch index rather than in the container. Whoever fixes the three container sites should take
  this one in the same pass; the refusal has the same shape as ADR-0029's module-identity clash.
- **The RPC audit's F219 and the extraction audit's F163 rhyme and must not be merged.** Both are one
  seam answering fewer distinct outcomes than the situation has. F163 is a status a macro hardcodes
  and a `Custom { status, message }` variant fixes it. F219 is a framework event that three
  transports structurally cannot raise, because on a subject-based broker a message for an unhandled
  pattern is never delivered to anything — no status change reaches it, and what is owed there is
  honesty about which transports can raise it.
- **The RPC build's Phase 2 rides the HTTP build's PRs 1-9, and its Phase 1 rides nothing.** Its D1
  settled as a capability declaration — transports say what they can carry — which is
  `AdapterCapabilities` on a third transport. **One `#[non_exhaustive]` type and one ADR across
  HTTP, WebSocket and RPC**, co-designed rather than invented three times; the GraphQL audit's D7
  takes the same shape for the engine SPI, which makes four. An earlier revision of this file said
  the RPC build had no cross-subject dependency; that was written before D1 was settled and only
  its Phase 1 — eight PRs closing eight findings — is still free-standing.
- **The RPC build's PR 13 reserves a signature three transports have an opinion about.** Request-side
  streaming rides at the SPI only (its D7): `RpcMessageCallbacks` takes `Items<RpcData, RpcError>`,
  which is ADR-0049's cardinality in the inbound direction. gRPC already answers the handler half of
  the same question with `Inbound<T>` (ADR-0043). Whoever builds the follow-on call shapes reads
  that extractor rather than inventing a second one. The **gRPC audit has now run** (2026-09-23) and
  confirms `Inbound<T>` is the extractor to read; its F237 records the one thing `Inbound<T>` costs,
  which is that it consumes the request and so shuts a client-streaming method out of its own
  metadata.

- **The gRPC audit's F235 and the RPC audit's F222 are one question asked on two transports, and
  should be answered once.** Both are an execution that is never told its caller has gone: F222 from
  the client side, where no cancel notice is sent on a timeout, and F235 from the server side, where
  the token fires on a streaming answer and cannot fire on a buffered one. The shared root is that
  abandonment is observable on exactly one code path in the framework — a dropped `ScopedStream` —
  and every transport's buffered answer sits outside it. The gRPC memo's **D3** is the scope call and
  says so; deciding it for gRPC alone would answer a quarter of it.

- **The gRPC audit's D4 is the pipeline build's PR 7 and should be read before it starts.** F136
  unifies each transport's error, and `HttpError` is already that shape — named variants,
  `Custom { status, message }` for the codes `ErrorKind` does not reach, and `AppError`. `RpcError`
  and `WsError` have **no `Custom` arm**, so PR 7 is giving them HTTP's escape hatch; gRPC takes the
  same arm with `GrpcCode` in place of `u16`. **The wrinkle runs the other way**: ADR-0039's
  `caused_by` lets a `GrpcStatus` carry a hand-named code *and* a typed error, which HTTP's `Custom`
  cannot do. **Settled 2026-09-23: the unification propagates that capability rather than flattening
  it** — `Custom { status, message, source: Option<Arc<dyn Error>> }` on all four transports, so a
  handler naming an unusual code keeps `#[catch(MyError)]` reachable everywhere, which today only gRPC
  manages. PR 7 must carry the three-field form. `GRPC_AUDIT.md` §8's D4 carries the argument.
- **The gRPC audit's F240 is ADR-0050 reaching only three transports.** Nothing implements
  `IntoOutput<Grpc>`, and `#[grpc_methods]` still decides fallibility by matching the last path
  segment against `Result` — the check ADR-0050 removed from HTTP, RPC and WebSocket. The structural
  half belongs with the pipeline build's **PR 9** (the API pass), not with a gRPC-only change.
- **The gRPC audit's F241 is the HTTP audit's D6 seen from the fourth transport.** `Error::details()`
  is rendered on three transports and dropped on gRPC, while HTTP is adopting RFC 9457 to carry more
  of it and the extraction build's D1 is about to put `ValidationErrors` there. gRPC has a specified
  slot — `grpc-status-details-bin` — that nothing writes.

- **The pipeline build's PR 12 decides whether gRPC's `add_service` bypass shrinks.** Pipeline D2
  settles that the pre-dispatch role reaches all four transports and does not say where it anchors on
  gRPC. Anchored in the `Server::builder().layer(…)` stack above tonic's router, where `TracingLayer`
  and `MethodPathLayer` already sit, it covers services registered through `add_service`; anchored
  inside the generated trait method it covers nothing new. `GRPC_AUDIT.md` §8's D2 establishes the
  ceiling on any wider fix — a guard reading the decoded message cannot run above the codec, so the
  two-tier split is structural rather than a choice — and recommends the router anchor.

- **ADR-0053 carries a framework-wide clause inside an HTTP record, and its build is HTTP-only.**
  *"`render_error` becomes reachable. It is `pub(crate)` on every transport today"* is what takes F22
  and F87, and the HTTP build's PR 11 verifies the envelope *"on all five"* adapters — five adapters,
  not four transports. `rpc/rpc_error.rs` and `ws/ws_error.rs` keep private renderers that no PR in
  any folder touches. gRPC needs nothing: `GrpcStatus::from_error(&dyn Error)` is already public and
  already takes the borrow a `#[catch]` handler holds, which is the signature the other three want.
  Decide it once, in PR 11, rather than leaving two transports to a later pass.
- **The render pass ran on 2026-09-23 and produced no memo, by its own first section.** Its two
  findings both belong to the HTTP build: **F242** (a recovered panic's payload reaches the caller on
  all four transports, pinned as correct by three tests) and **F243** (the envelope is written at more
  sites than ADR-0053 counts, and the uncounted ones — request/response conversion, actix's channel
  bridge, rocket's unperformed upgrade — sit outside the routing seam ADR-0052 collapses, so 0053's
  *"one renderer instead of six"* is not what the tree will hold). Its four appends to F22, F64, F69
  and F87 are read by whoever deletes those entries.

The triage pass owns sequencing. Neither the DI memo nor the pipeline memo recommends a place in the
queue, by decision.

## The rule the builds share

Pre-release, the crate is unpublished and the first release is a beta, so no build here retains an
API for compatibility: delete rather than deprecate, rename rather than alias. Recorded as D13 in
`DI_AUDIT.md`; it reads across the set.
