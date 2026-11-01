# The gRPC audit

Scope as asked: **"grpc audit"** — run against `GRPC_AUDIT_HANDOFF.md`, which sets it at `ulo-grpc`,
`ulo::grpc`, `ulo-build` and `#[grpc_methods]`, measured against the gRPC specification, against
tonic's contract, and against what the other three transports do.

Run 2026-09-23 against master `654c7183`.

## Where the findings live

`FRAMEWORK_GAPS.md`, as **F235-F241**. This memo does not restate them; it carries the seam
argument, the probe record and the calls that are the user's.

**F240 and F241 were filed in a second pass on 2026-09-23**, after the first was read and the brief's
second half was restated: gRPC is measured against the **proposals the other audits have already
settled**, not only against what the framework does today. §6a is that comparison and is where both
came from. F238 was corrected in the same pass — the pipeline audit's D4 already answers the
specification half of it.

One entry outside that range moved: **F63** gained an append. The specification answers the question
F63 defers, and one clause of F63 was wrong — see §5.

## Not a build — until 2026-09-23

No branch exists and nothing here is written. **The build was approved on 2026-09-23**: F240 named
explicitly, the rest of §7 and every decision approved with it, so `GRPC_BUILD_HANDOFF.md` now exists
beside this file and carries the process. §7 stays the plan.

## Confidence marking

- **Probed** — run this session against `654c7183`. The probe is
  `artifacts/grpc_audit_probe.rs`, eight cases, no Docker and no broker; its header says how to run
  it.
- **Read** — read off the source, cited by file and line. Where the source is tonic it is cited as
  `tonic 0.14.6` with its own path.
- **Spec** — quoted from `PROTOCOL-HTTP2.md`, `statuscodes.md` or `interop-test-descriptions.md`,
  fetched this session rather than recalled.
- **Unverified** — believed, not established. Marked inline.

## The shape of it

The brief's first job was to decide where the seam is, and the answer changes what the rest of the
pass was worth looking at. **tonic owns more than the brief's draft cut allowed, and the one thing
the brief predicted ulo had solved is the one thing it has not.** Sections 1 and 2 are that argument
and are read in order. Sections 3-6 are independent and can be taken in any order: 3 is the
cancellation and deadline defect, 4 is metadata, 5 is the status vocabulary, 6 is the smaller defects
and what was checked and found sound. **§6a is the second pass** — gRPC against the six audits'
settled decisions, which is where F240, F241 and D4 came from and where the DX and syntax comparison
lives. Section 7 is what a build would be, §8 the decisions, and the last section what was left alone
and why.

---

## 1. The seam, settled

The brief proposed a cut "to be confirmed rather than assumed". Confirmed with one correction, which
is the finding of the pass.

**tonic's, and auditing them audits tonic:** HTTP/2 framing, the `application/grpc` content type,
length-prefixed messages, trailers, `grpc-status` and `grpc-message` encoding, flow control,
keepalive, TLS, and — the correction — **deadline enforcement** and **the `UNIMPLEMENTED` a missing
method answers**.

**ulo's:** which status code a failure becomes, what a handler may take and return, what metadata an
application can read and write, whether an execution learns it was abandoned, what `add_service`
bypasses, what `ulo-build` generates, and the shape of `#[grpc_methods]`.

The correction matters because `CLAUDE.md` states the deadline arrangement as *"it reports, tonic
enforces"*, and both halves are weaker than they read. tonic enforces a deadline by racing the
inner service future against a sleep and dropping it (`tonic 0.14.6
src/transport/service/grpc_timeout.rs:41-62, 80-92`, layered at `src/transport/server/mod.rs:1238`)
— **Read**. That race ends when the handler returns its `Response`, so for a streaming method it
covers the time to the first header and nothing after. ulo reports the deadline and acts on it
nowhere. Between them, a deadline on a streaming call is enforced by neither. That is F236.

**The honest summary the brief asked for, if it were the whole answer, would be "tonic does it and
ulo adds four things, three of which are fine".** It is not the whole answer. ulo's four additions
are the enhancer pipeline, the extractor-shaped handler, the metadata view and the execution — and
**two of the four are defective at the seam**, not at the wire. The execution does not learn that its
call was abandoned (F235), and the metadata view drops a category the specification defines (F237).
Both are invisible from the wire, which is why a wire-level suite would not have found them.

## 2. The interoperability suite — decided, and the answer is no

The brief set this as a first-day call. **Do not stand it up.** Three reasons, in order of weight.

**It would measure tonic.** The suite's cases are wire behaviour, and the wire is tonic's by §1. A
ulo server passing `empty_unary`, `large_unary`, `client_streaming`, `server_streaming` and
`ping_pong` would demonstrate that tonic frames messages correctly, which is not in doubt and is not
ulo's to fix when it is not.

**The cases that do reach ulo's seam are the five findings already filed**, and an eight-case probe
file reached all of them in one session with no new protos. `cancel_after_begin` and
`cancel_after_first_response` are F235. `timeout_on_sleeping_server` is F236. `custom_metadata` is
F237.

**And its sharpest case cannot be passed by any tonic server, so a failure there would carry no
information about ulo.** `custom_metadata` requires the server to echo `x-grpc-test-echo-trailing-bin`
back *as trailing metadata* (**Spec**, `interop-test-descriptions.md:1440-1446`). `tonic::Response`
exposes `metadata()` and `metadata_mut()` and has no trailer API at all (**Read**, `tonic 0.14.6
src/response.rs:26-124`). A ulo server would fail that case for a reason one layer below anything
this framework decides.

The RPC pass's lesson — that `ulo-rpc-conformance` was right and nearly empty — does not transfer.
It transfers backwards: the reason that suite was cheap to be wrong about is that ulo wrote it, and
the reason this one is expensive is that ulo would be adopting a specification it does not implement.

**What is worth pinning instead** is the subset where ulo's seam is what answers: a call abandoned
mid-flight, a deadline that expires against a streaming reply, and a binary metadata key on the way
in. Three ordinary integration tests against the existing `orders.proto`, written as part of
whichever of F235-F237 is built, and no `grpc-testing` protos adopted.

## 3. Cancellation and the deadline — F235, F236

The defect a caller pays for, and the section to read if only one is read.

**Probed**, both triggers, both reproducing: a 300 ms deadline against a 1.5 s handler, and a client
dropping a unary call at 250 ms. In each the handler future is **dropped** mid-await, the execution's
cancellation token does **not** fire, and a `tokio::spawn`ed task holding `ctx.cancellation().clone()`
**ran to completion**.

The mechanism, established by reading rather than inferred from the probe — the trap
`feedback-a-probe-shows-what-not-why` records. `CancellationToken` has no `Drop`
(`crates/ulo/src/context/cancellation.rs:34-83`); on gRPC the only caller of `cancel()` is
`ScopedStream::drop` (`crates/ulo/src/dispatch/scoped_stream.rs:60-66`); and a unary reply has no
`ScopedStream`, because `IntoScoped` resolves to the identity impl for a message and the wrapping one
only for a stream (`crates/ulo/src/grpc/runtime.rs:414-430`). No unary path can fire the token. What
drops the handler is tonic's timeout future or the reset stream, neither of which runs ulo code.

**The token's own doc states the justification, and the probe falsifies it.**

> A buffered answer never fires it: the handler's future is dropped with the connection, and nothing
> that could observe the token is still alive to.

Detached work is alive to, and handing a clone of the token to detached work is what the token is
for. `grpc_tail.rs`'s own `detached_producer` does exactly that for the streaming case, where it
works. The sentence should go with whatever is decided, because it is false either way.

**The asymmetry is the part that misleads.** A streaming gRPC reply fires the token — ADR-0033, pinned
by `grpc_tail.rs`. A unary one cannot. The two are written in one impl block, under one attribute,
with nothing marking where the guarantee stops.

F236 is the same root reached from the other side. ulo parses `grpc-timeout` into an `Instant` at
construction and hands it out through `deadline()` (`crates/ulo/src/grpc/context.rs:57-62, 196`), and
nothing consumes it. **Probed**: a `watch_progress` call carrying a 300 ms deadline delivered **all
400 items over ten seconds** and the token never fired. This is F222's leak on the one transport
whose wire states the budget and whose context already holds it parsed.

The fix for F236 is cheaper than the one for F235 and covers part of it: ulo holds the deadline and
owns the token, so firing the token when the deadline passes needs no tonic change and reaches the
unary and streaming paths alike.

## 4. Metadata — F237

**Probed**: a call appending `x-tag: first` then `x-tag: second` and inserting `x-trace-bin` reaches
a handler whose `ctx.headers()` holds `x-tag = "second"` and no `x-trace-bin`.

Both losses are specification surfaces rather than open questions. `PROTOCOL-HTTP2.md` defines
`Binary-Header → {Header-Name "-bin"} {base64 encoded value}` as one of the two forms
**Custom-Metadata** takes, and says duplicate names *"may have their values joined with "," as the
delimiter and be considered semantically equivalent"* — **Spec**. One value is not equivalent to two.

The brief predicted the RPC pass's flat-map question would recur here, and it does, with the
difference the brief named: on four brokers each transport resolved it differently and nothing
decided between them; here a published document decides it and ulo differs from it.

**Where the escape hatch reaches and where it stops** is the part that makes this worth building
rather than documenting. `GrpcRequest<T>` hands a handler tonic's `MetadataMap` whole and recovers
both losses — for a **unary** handler, and for the handler alone. It declares `CONSUMES` as
`Inbound<T>` does, so a client-streaming method cannot take both and has no route to its metadata;
and guards, interceptors and error handlers receive `&GrpcContext` and nothing else, so **the layer
most likely to want a binary credential is the one layer with no way to read one**.

Two smaller things in the same entry, neither worth its own number. The reply side refuses `-bin`
keys by construction — `MetadataKey::<Ascii>::is_valid_key` is `!key.ends_with("-bin")` (**Read**,
`tonic 0.14.6 src/metadata/encoding.rs:115-117, 184-186`) — which `reply.rs:34-36` documents as a
limit. And neither side screens the `grpc-` prefix the specification reserves, so an enhancer can
stamp `grpc-anything` onto a reply.

## 5. The status vocabulary — F238, and what F63 gets wrong

Two separate things meet on one line of `crates/ulo/src/grpc/runtime.rs`.

**F238 is ulo's and is a defect.** `GuardRejection` carries two renderings on purpose: `Display` is
the diagnostic, `Error::message()` is what a caller is told. HTTP, RPC and WebSocket render through
`message()`. gRPC formats the diagnostic itself into the status message, and **Probed**, a refused
caller receives `PermissionDenied` with message `"guard 0 rejected request"` — the guard's position in
the chain, enumerable by varying credentials. The branch one line above does the same with a panic
payload.

**The specification question on the same line is F130's gRPC half and is not a defect of the same
kind.** `statuscodes.md` requires that `PERMISSION_DENIED` *"must not be used if the caller can not
be identified (use UNAUTHENTICATED instead for those errors)"* — **Spec**. A `Guard` answers `bool`,
so nothing in the framework can distinguish the two cases the specification separates. On HTTP and
WebSocket the equivalent choice is an invention and either answer is defensible; on gRPC a published
rule says which, and the type system cannot express it. Recorded in F238 and left to F130's build.

**F63 is answered by the specification, and one of its clauses is wrong.** F63 defers the choice of
word for a shed load and rests on a split — a rate limit is the caller's fault, a shed is the
server's condition. `statuscodes.md` declines that split: `RESOURCE_EXHAUSTED` is *"Some resource has
been exhausted, perhaps a per-user quota, or perhaps the entire file system is out of space"*, one
code covering both readings deliberately, and its library table names a shed load directly — *"Server
temporarily out of resources ... RESOURCE_EXHAUSTED"* (**Spec**). **Probed**: ulo's gRPC server under
`with_max_inflight(1)` answers `ResourceExhausted`, so the spec-conforming behaviour already ships on
this transport. The wrong clause is F63's *"which tonic's table maps from no `ErrorKind`"*:
`code_for(ErrorKind::TooManyRequests) => GrpcCode::ResourceExhausted` and `kind_for` inverts it
(`crates/ulo/src/grpc/status.rs:147, 175`). The framework's own table already equates them. Appended
to F63; F223 is untouched, since its `unsupported` and `error` are outside every candidate.

## 6. The two smaller defects — F239, and what was checked and found sound

**F239**: `parse_grpc_timeout` splits one byte from the end of the value and `str::split_at` panics
on an index inside a multi-byte character (`crates/ulo/src/grpc/context.rs:210`). **Probed**:
`GrpcContext::new` with `grpc-timeout: "5€"` panics. Unreachable through the adapter, which is why it
has survived — `HeaderValue::to_str` admits only visible ASCII — and reachable from every other
caller of a `pub` constructor, including the adapter-less pipeline the type's own doc describes as
supported. The function's doc promises the opposite of what it does. One-line fix, `split_at_checked`,
stable under the 1.88 MSRV.

**Checked and sound**, recorded so the next pass does not re-probe them:

- `with_drain_timeout`, `with_max_inflight` and `with_max_per_connection` all work. **Probed** for
  the shed; the drain path is **Read** and is pinned by existing tests. The brief asked for
  confirmation rather than assumption, and it is confirmed.
- Binding is synchronous inside `into_lifecycle`, so a port conflict is an `Err` from `app.bind()`,
  and the TLS acceptor is built there too, so a bad certificate is a `StartupError::Adapter`. Both
  match ADR-0024's refuse-or-none contract.
- `ulo-build`'s six error variants each name their cause and `NoService` names the fix. A wrong
  `shapes("pkg")` reports the path it could not read. One scoping note, not filed: `compile_protos`
  ends in `shapes_in_out_dir()`, which rewrites every `.rs` in `OUT_DIR` rather than the files it
  wrote. Benign in both crates that use it, and an ordering constraint nothing states.
- `GrpcCode::from_i32` has no arm for `2`; the `_` arm answers `Unknown`, which is what `2` means. Not
  a defect.

## 6a. gRPC against what the other audits have already decided

Added 2026-09-23 after the first pass was read. The first pass measured gRPC against the
specification, against tonic and against what the other three transports **do today**. That is half
the brief: six audits have already run and their decisions are settled, so the standard gRPC has to
meet is what the framework **will be**, not what it is. This section is that comparison, and it found
two findings the first pass missed (F240, F241) and one decision it could not have seen (D4).

The three rows that matter most are the last three.

| Settled elsewhere | gRPC today | Verdict |
| --- | --- | --- |
| Pipeline API pass — one type parameter on all three roles: `Guard<Grpc>`, `Interceptor<Grpc>`, `Next<Grpc>` | `Guard<GrpcContext>`, `Interceptor<GrpcContext, GrpcHandlerResult>` | Conforms mechanically. `Grpc` is already a `Transport` with `Context`, `Output` and `Error` declared |
| Pipeline API pass — `HasHeaders` for the stamping case | `GrpcReply::header` / `set_header` exist and name no reply type | **Already built for this.** The pipeline audit names `GrpcReply` as one of the two implementors; #312 built the shape it needs |
| Pipeline D1 — a declaration's order is the contract | gRPC reads `enhancers.guards` then `handler_guards[method]` | Conforms; F121's fix is transport-agnostic |
| Pipeline D4 — a guard answers `Result<(), E>` | `can_activate -> bool` | Conforms, **and gRPC gains most from it** — see §5 and the F238 correction |
| Pipeline F136 / F129 — one error vocabulary, one renderer | `GrpcStatus { code, message, source }` — no `kind` arm, no `AppError` variant, a *field* where the other three have a *variant* | **Collision. D4 below.** |
| Extraction D1 — `FromContext::Error: ulo::Error` | `RequestError` implements `std::error::Error` and not `ulo::Error` | Conforms with work: four variants, all `Internal`, plus the `Validated` case the F1 append records |
| Extraction D2 — markers stop being a second code path | gRPC has no markers | Not applicable |
| DI D2 — a token is single-bound unless declared multi | gRPC globals go through the same registry | Conforms; nothing gRPC-specific |
| DI D14 — an injectable is shared, `Arc` for singleton and execution | gRPC services build per call when execution-scoped | Conforms |
| HTTP D3/D4 — the status is `http::StatusCode`, headers are `http::HeaderMap` | `GrpcCode` is ulo's own enum; metadata is a `HashMap` | **Cannot take the same answer, for a good reason** — the equivalent borrowed types are `tonic::Code` and `tonic::metadata::MetadataMap`, and core is deliberately tonic-free. This is the argument that decides §8's D1 against option (b) |
| HTTP D6 — RFC 9457 `problem+json` carries structured error detail | `Error::details()` is read on three transports and **not** on gRPC | **F241.** And gRPC has a specified slot for it that nothing writes |
| ADR-0050 — `IntoOutput<T>` decides what a handler returns | **No `IntoOutput<Grpc>` impl exists**, and the macro name-matches `Result` and `Response` | **F240.** The check ADR-0050 removed from the other three is still live here |
| RPC D3(b) — carry the deadline, fire the token when it passes. *Filed with a trigger, not built* | gRPC already reads the deadline and fires nothing | **F236 is RPC's (b) on the one transport where it is nearly free.** See §8's D3 |

### The syntax, compared rather than described

What a handler looks like on each transport, which is the question "is the DX consistent" reduces
to. Three of four now read the same way, and gRPC's differences divide into two kinds.

**Inherent, and not defects.** The attribute names the proto trait — `#[grpc_methods(orders_server::Orders)]`
where the other three take no argument — because the contract is in a file the macro cannot read.
`#[grpc_method]` and `#[grpc_stream]` mark which methods are handlers, where `#[get]` and
`#[message_pattern]` carry a route; a macro cannot resolve the trait's items, so the marker is the
only way to tell a handler from a helper in the same impl. **`#[grpc_stream]` is the better design
of the two available** and should not be "simplified": inferring streaming from the return type would
be a third name match of exactly the kind F240 is about. One struct serves one proto service (F16),
which is incidental rather than decided.

**Not inherent.** The return type is read by name (F240). `details()` does not travel (F241). Binary
metadata is unreachable from an enhancer (F237). None of these follows from tonic owning the wire;
each is a seam ulo owns and wrote differently here.

**One row of `CLAUDE.md` said the opposite of the code and is corrected.** It claimed handlers may
take `&HttpContext`/`&RpcContext`/`&WsContext` "(gRPC excluded — tonic fixes the signature)", while
the gRPC section two screens down describes `&GrpcContext` as an ordinary param per ADR-0038. The
probe takes one and it compiles. The exclusion was true before ADR-0038 and outlived it.

## 7. What a build would be

Seven PRs, and the only sequencing is that F236 precedes F235. **Approved 2026-09-23**;
`GRPC_BUILD_HANDOFF.md` carries the process, the divergence gate and the per-PR verification. **Three of them wait on decisions that are not this pass's**: F237 on §8's D1
(now settled, so it is buildable), F240's structural half on the pipeline build's PR 9, and the
`GrpcStatus` shape on that build's PR 7 via §8's D4.

1. **F239** — one line plus a non-ASCII test case. Independent of everything.
2. **F238** — render `Error::message()` the way the other three transports do; keep the index on the
   event and in the `tracing::debug!` beside it. The existing test asserts the code only, so it gains
   a message assertion. Survives the pipeline audit's D4 — see the F238 entry.
3. **F240** — a diagnostic naming the rule, spanned at the return type, for both name matches. The
   structural half (putting gRPC on `IntoOutput<Grpc>`) is larger and is sized with the pipeline
   build's PR 9, not here. *Superseded at approval, 2026-09-23: PR 3 removes the name matching and
   puts gRPC on `IntoOutput<Grpc>` in `ulo-grpc`; `GRPC_BUILD_HANDOFF.md` carries that scope, and
   shipping the diagnostic alone is a divergence.*
4. **F241's cheap half** — state in `to_tonic`'s doc and in `Error::details()`'s that detail does not
   travel on gRPC. The real fix writes `grpc-status-details-bin` and rides F237's trailer work.
5. **F236** — fire the token when the deadline passes, from the one place that builds the context.
   Delete the falsified sentence from `CancellationToken`'s doc in the same change.
6. **F235** — a drop guard on the unary path, disarmed at a clean return. Larger than the others and
   partly covered by 5; worth sizing after 5 merges, and its scope is §8's D3.
7. **D2's warning** — a startup `warn` per service registered through `add_service`, naming the
   bypass.

**F237 is not in that list**, because its fix changes a public type and the shape is the user's call
— see §8's first decision.

**F75 stays parked**, and what that paragraph is saying, stated plainly. F75 is the defect that
`Payload<Wrong>` on a gRPC handler compiles and fails the *first call* with `Internal` naming both
types, where every other transport catches a wrong extractor at compile time. The user parked it on
2026-09-16 with "let us have a look at this when it allows". The fix designed for it is a trait
called `GrpcReads`, which every gRPC extractor would implement to declare what wire shape it wants —
`Payload<T>` wants one message, `Inbound<T>` wants a stream, `Extensions` wants neither — so the
macro can assert the handler's parameters against what `ulo-build` recorded for the method. The
**cost that parked it** is that the trait is not optional: a user writing their own gRPC extractor
would have to implement it too, one line, because neither associated-type defaults nor const
`TypeId` comparison is stable.

The brief asked whether this audit changes that cost. **It does, and the first answer given here was
wrong about why.**

*The wrong reason, retracted.* The first pass said §4 makes a hand-written gRPC extractor the
workaround for unreachable binary metadata, so custom extractors become likelier and F75's per-extractor
cost rises. **The mechanism does not support it.** A user's extractor reaches the request through
`RequestCarrier`, whose `take_message` hands back the message and whose `into_any` hands back the
carrier as `dyn Any` — and the only type that downcast could target, `MessageRequest<T>`, is private
to `ulo-grpc` (`crates/ulo-grpc/src/shape.rs:54`). **No custom extractor can reach the metadata map.**
The one route is taking `GrpcRequest<T>` as a handler parameter, which is a built-in and not a custom
extractor, and which yields the message too through `into_inner()`. So §4's workaround costs a
parameter change, not a new trait impl, and it makes custom extractors no likelier than before.
Settling D1 removes even that: `header_bin(k)` on the context reaches binary metadata with the
request untouched.

*The real coupling, which is with F240.* Both F75 and F240 want the same thing from the same place.
F75's design puts `type Wants = Unary<GreetRequest>` on the `MethodShape` marker `ulo-build` writes,
so the macro can assert a parameter against what the method carries. F240's candidate fix wants
fallibility and reply shape read off that same marker rather than matched against the identifiers
`Result` and `Response`. **One generated marker, two questions asked of it.** Whichever is built
first pays for extending `ulo-build`'s codegen and the assertion machinery, and the second is then
much cheaper.

That is the input the brief asked for: F75's cost falls if F240 is built, and F240 is a defect that
needs building anyway. **It stays parked** — the audit records the coupling, it does not unpark it —
and whoever takes F240 should read F75's design before choosing where the answer lives.

## 8. Decisions

All four are answered. D1 was settled by the user; D2, D3 and D4 are answered here with the work the
first two passes owed them. **D4 is settled but is not this build's to execute** — it belongs to the
pipeline build's PR 7.

### D1 — the shape of the metadata view (F237). **SETTLED 2026-09-23: option (a).**

The value type carries what the wire carries: an ASCII multimap plus a binary map, `header(k)`
keeping today's most-specific-wins shape, `headers_all(k)` and `header_bin(k)` beside it. It is the
`get` / `get_all` split ADR-0020 already settled for declared metadata one field over.

*Rejected — handing guards and interceptors tonic's `MetadataMap`.* It is the cheaper fix and it
reads well, and it costs the property that `ulo` names no tonic type. §6a is what settles it rather
than taste: the HTTP audit's D4 took `http::HeaderMap` from the `http` crate precisely because HTTP's
headers are already a standard type core can depend on, and gRPC's equivalent is a type core has
decided it cannot name. The two transports differ in what they may borrow, so they differ in answer.

*Rejected — documenting the limit.* It leaves a guard unable to read a binary credential, which §4
establishes is the case most likely to want one.

### D2 — `add_service` and the pipeline. **Answered 2026-09-23: the split is the way, and it is already designed elsewhere.**

**Why it bypasses.** The enhancer pipeline on gRPC is not a layer. It is code `#[grpc_methods]`
generates *inside each trait method body*, which calls `run_grpc_pipeline` and then the handler.
`add_service` takes a `tower::Service<http::Request<TonicBody>>` and puts it in the same
`RoutesBuilder` the generated services go into — a peer in tonic's router, below which ulo has no
per-method seam. Nothing is being skipped; there is nowhere for it to be applied.

**Why the pipeline sits inside the method, which is the ceiling on any fix.** A guard reads the
decoded message through `ctx.message::<T>()`, and the message is typed only after tonic's codec has
run — which happens inside the generated method, where `MethodShape::install` hands it to the
execution. A tower layer above the router sees length-prefixed bytes and a method path and cannot
decode them, because decoding needs the per-method type that only the trait impl names. **So the
split is by what an enhancer reads**, and no wholesale lift is available.

**The split, and the thing the first pass missed: it is not a new concept and it needs no new
registration surface.** The pipeline audit's **D2 is settled** and already builds exactly the tier
this needs — *"a pre-dispatch role on all four transports — request in, answer out … reaching RPC,
WebSocket and gRPC for the first time"*. Two tiers, each already named:

| tier | reads | runs as | reaches |
| --- | --- | --- | --- |
| **pre-dispatch role** (pipeline D2) | method path, metadata, peer, deadline — everything before the codec | a layer above tonic's router | **every service, `add_service`d ones included** |
| **guards / interceptors / error chain** | the decoded message, `Payload<T>`, `ctx.message::<T>()` | generated inside the trait method | ulo-dispatched services only |

This matters because the alternative designs are worse and both were considered. Declaring the tier
*on the enhancer* cannot work: whether a guard reads the message is a runtime call, invisible in the
type, so the framework cannot place it and a user would not know which tier they had written.
Declaring it at *registration* would mean two global surfaces for one role, which collides head-on
with the pipeline audit's settled F133–F135 — **one registration surface per transport per role.**
The pre-dispatch role escapes both because it is a *different role*, not a second spelling of Guard.

**The stance, stated for the ADR.** *The pipeline reaches what ulo dispatches. What ulo does not
dispatch gets the pre-dispatch role and nothing below it, and the framework says so at startup rather
than leaving it to be discovered.* That is honest about the ceiling instead of promising a uniformity
the codec boundary will not allow.

**What this pass owes the pipeline build, which is one specific question.** Pipeline D2 settles that
gRPC gets a pre-dispatch role and does not say **where it anchors**. On gRPC that choice decides
whether `add_service` is covered:

- **Above tonic's router**, in the `Server::builder().layer(…)` stack where `TracingLayer`,
  `DrainLayer` and `MethodPathLayer` already sit (`crates/ulo-grpc/src/grpc_adapter.rs:293-296`) —
  the anchor physically exists today, and a role there sees every request including
  `add_service`d ones.
- **Inside the generated method**, above the guards — reaches nothing new, and D2's bypass stays
  exactly as wide as it is now.

**Recommendation: anchor above the router, and take the startup `warn` now.** The warning (§7's
PR 7) is worth having whichever way the anchor goes, because even the covered case leaves guards and
interceptors out of reach. The anchor question belongs in the pipeline build's PR 12 and is recorded
in the campaign README as a coupling.

### D3 — the asymmetry in F235. **Brief, as asked, and then the question.**

**Yes, it is the cancellation.** In full, because the first pass compressed it:

Every execution carries a `CancellationToken`. It is how a handler learns *the caller is gone, stop
working* — you clone it, hand it to whatever is doing the work, and that work stops. It is the only
such signal in the framework.

The token fires in exactly one place: when a `ScopedStream` is dropped before its stream has ended.
So on gRPC:

- **A handler that answers with a stream** gets a `ScopedStream` wrapped around its reply. Caller
  disconnects, stream drops un-drained, **token fires**, producer stops. This is ADR-0033 and it
  works.
- **A handler that answers with a message** gets no wrapper — `IntoScoped` resolves to the identity
  impl for anything that is not a stream. There is no drop site, so **nothing can fire the token**,
  ever, on any unary call.

The two are written in the same impl block, under the same attribute, one method apart. Nothing in
the types, the docs or the diagnostics marks where the guarantee stops.

**What the probe showed happens instead.** The handler's *future* is dropped — by tonic, at the
deadline or on the reset stream — so a handler that `await`s its work does stop. A handler that
**spawned** its work does not: the spawned task holds a clone of a token that will never fire, and
it ran to completion for a caller that had gone. The framework's own doc says this cannot happen —
*"nothing that could observe the token is still alive to"* — and the spawned task is alive to.

**Why it was written as "cheap vs. properly done".** It was not meant as a cost argument and reads
like one. The real question is **scope**.

**What RPC faced, since the first pass cited it without stating it.** RPC's D3 asked one question —
*does a caller's abandonment reach the server?* — and found the machinery built and unused: ADR-0032's
cancel notice fires when a **reply stream** is dropped, and no `send` timeout sends one. The same
shape as gRPC's, from the client side. Three options were on the table:

- **(a) Send the cancel notice on a `send` timeout too.** One branch per client transport, reusing
  the frame and the registry that already exist. Handles *a caller that walked away*.
- **(b) Carry the deadline on the wire** — a reserved header, `RpcContext::deadline()` reading it,
  the handler's token firing when it passes. Handles *a server that should never have started*.
- **(c) Both.**

**Settled 2026-09-23 as (a) now, (b) filed with a trigger.** The reason (b) was deferred is stated
there and is entirely about RPC's own shape: it is a **grammar change on seven transports**, plus a
clock-skew decision — and that memo names gRPC as the design that avoids skew, because gRPC sends a
*duration* rather than an instant.

**So the deferral's reasons do not hold on gRPC.** No grammar change: the header is defined by the
specification, already on the wire, already parsed into an `Instant` at context construction. No skew
question, for the reason RPC's own memo gives. **gRPC is where RPC's expensive half is nearly free**,
and the two halves are not alternatives — (a) is the client telling the server, (b) is the server
enforcing what the caller already said. gRPC needs only (b), and tonic supplies part of (a) by
dropping the future.

**Is (ii)-sequenced-gRPC-first optimal?** On the evidence, yes, and the argument is falsifiable rather
than aesthetic: the thing nobody has verified is whether a drop-site design actually fires the token
correctly under real abandonment. gRPC is the only transport where that can be built and tested
**without touching a wire grammar**, so it is the cheapest place to find out — and if the design is
wrong, it is wrong before six other transports have been changed to match. Building RPC or HTTP first
would cost more to learn the same thing. The one risk to name: gRPC's unary drop path is tonic's
future-drop, so a design validated only there has not been tested against a transport where ulo owns
the drop site. That is an argument for gRPC being first, not for it being the only one.

**The question, then, is not whether to fix it but how wide the fix is:**

- **(i) gRPC only** — fire the token when the deadline passes, and on the unary drop path. Closes
  F235 and F236. Leaves HTTP, RPC and WebSocket with buffered answers that signal nothing.
- **(ii) The framework** — an execution learns it was abandoned on every transport and both answer
  shapes, which means a drop site on the unary path of all four. This is one concept built once, and
  it is what makes the token's documentation true rather than nearly true.

**Recommendation: (ii), sequenced so gRPC goes first.** gRPC is the only transport that can be built
and verified without touching a grammar, so it is the cheapest place to find out whether the drop-site
design holds. If the user's answer is "properly done", (ii) is the properly-done shape and (i) is the
first PR of it — not a smaller alternative to it. This should be answered together with RPC's D3(b),
and the campaign README now records the two as one coupling.

### D4 — what happens to `GrpcStatus` when the error vocabulary unifies. **HTTP already solved this; gRPC borrows it. One thing does not borrow.**

The first pass presented option (a) as a compromise. It is not a compromise — **it is the shape HTTP
already ships**, and the user was right to ask.

**HTTP faced the identical arithmetic and answered it.** HTTP's status space is roughly sixty codes;
`ErrorKind` has eleven. `HttpError` resolves that with three kinds of arm
(`crates/ulo/src/http/error.rs:71-101`):

| arm | what it is for | how it yields a status |
| --- | --- | --- |
| named variants — `BadRequest(String)` … | the common kinds | a literal per variant (`:150-156`) |
| **`Custom { status: u16, message: String }`** | **the codes `ErrorKind` does not reach** | passed through (`:157`) |
| `AppError(Arc<dyn Error>)` | a domain error | `status_for(e.kind())` (`:158`) |

F136's unified shape — `Status { kind, message }` / `Custom { status, message }` / `AppError` — **is
this, generalised.** HTTP is the precedent, not the odd one out: `RpcError` and `WsError` have named
variants plus `AppError` and **no `Custom` arm at all** (`crates/ulo/src/rpc/rpc_error.rs:30-44`,
`crates/ulo/src/ws/ws_error.rs:24-40`), so F136 is giving them the escape hatch HTTP has had all
along. gRPC needs the same arm with one substitution — `GrpcCode` where HTTP has `u16` — and that
substitution is already made, because `GrpcCode` exists, mirrors `tonic::Code` numerically, and is
ulo's own type for the reason D1 gives.

So: **`Status { kind, message }` / `Custom { code: GrpcCode, message }` / `AppError`.** Nothing
invented, one type swapped, and ADR-0039's expressiveness kept because `Custom` is exactly where a
handler names `FailedPrecondition` or `Aborted`.

**The one thing that does not borrow, and it runs the other way.** ADR-0039 gave `GrpcStatus` a
`caused_by(e)` that keeps a **hand-named code** *and* a domain error on the same value
(`crates/ulo/src/grpc/status.rs:102-110`). HTTP's `Custom { status, message }` carries no error, and
`AppError(e)` derives its status from `e.kind()` — so **on HTTP you can name an unusual status, or
keep the typed error for `#[catch(MyError)]`, but not both.** gRPC can do both today.

That is not a gRPC problem to solve; it is a capability the other three lack and nobody has noticed,
because gRPC is where a wire code space wide enough to want it met a framework that cared about typed
errors. **The unification should propagate it rather than flatten it**: `Custom { status, message,
source: Option<Arc<dyn Error>> }` on every transport gives all four what ADR-0039 gave one, at the
cost of one optional field.

**SETTLED 2026-09-23: take HTTP's shape, substitute `GrpcCode`, and add the source slot to `Custom`
on every transport.** It changes no behaviour anywhere, keeps ADR-0039 whole, and closes on the other
three a gap nobody had named. The pipeline build's PR 7 executes it; what it must carry is that
`Custom` is `{ status, message, source: Option<Arc<dyn Error>> }` on all four, not `{ status, message }`
— so a handler naming an unusual code keeps `#[catch(MyError)]` reachable, which today only gRPC can
do.

*Rejected — growing `ErrorKind` to cover the gRPC codes.* It would put `FailedPrecondition`,
`Aborted`, `OutOfRange`, `AlreadyExists`, `DataLoss` and `Cancelled` on every transport and force HTTP
to invent a status for each. HTTP's own answer to a wider code space was `Custom`, not a wider kind
vocabulary, which is the precedent.

*Rejected — exempting gRPC from the unification.* It is the outcome the arithmetic points at if
`Custom` is forgotten, and it should not be arrived at by accident.

**Whose decision this is.** Settled here, executed by the pipeline build's **PR 7**. Recorded in this
memo because this pass found it, and in the campaign README as a coupling so PR 7 reads it before
starting.

## What is out of scope, and why

- **Guards, interceptors and the error chain.** The pipeline audit took all four transports and
  settled them; F129 and F130 belong to that build. F238 touches the same line as F130 and is filed
  as a separate defect for that reason.
- **Extraction.** The extraction pass covered `FromContext<GrpcContext>` and appended its gRPC finding
  to F1. Checked before probing, as the brief instructs; this pass takes only what was left, which is
  the `CONSUMES` interaction in §4.
- **Rendering.** `RENDER_AUDIT_HANDOFF.md` owns the renderer seam.
- **The wire below the seam**, by §1. A defect there is tonic's, and two were found and recorded as
  dependency notes rather than findings: tonic maps a timeout to `CANCELLED` where `statuscodes.md`
  names `DEADLINE_EXCEEDED` (`tonic 0.14.6 src/status.rs:644-646`), and `tonic::Response` has no
  trailer API, so no tonic server can pass `custom_metadata`'s trailing half.
