# Salvage: gRPC

**Subject.** The September Smile gRPC audit: `ulo-grpc`, `ulo::grpc`, `ulo-build` and `#[grpc_methods]` on the old framework, measured against the gRPC specification, against tonic's contract and against the other three transports. Findings F235-F241, the F63 and F238 corrections, decisions D1-D4, and the gRPC rows of `BUILD_SEQUENCE.md`.

**Source files read in full** (under `docs/experiments/di-redesign/salvage/source/`):

| File | Words |
| --- | --- |
| `grpc/GRPC_AUDIT.md` | 6,446 |
| `grpc/GRPC_AUDIT_HANDOFF.md` | 1,470 |
| `grpc/GRPC_BUILD_HANDOFF.md` | 2,073 |
| `grpc/artifacts/grpc_audit_probe.rs` | 1,341 |
| `grpc/artifacts/grpc_spelling_probe.rs` | 370 |
| `BUILD_SEQUENCE.md`, the gRPC rows, S2/S3/S5/S13 and Halt 2 only | — |

Also read: `FRAMEWORK_GAPS.md` entries F16, F63, F75, F129, F130, F235-F240; `transports/DESIGN.md` §2.4, §2.6, §2.8, §6; `integrations/QUESTIONS_5.md` (record list); the rebuild's `crates/ulo-grpc`, `crates/ulo-grpc-macros`, `crates/ulo-build`, the gRPC tests in `crates/ulo-codegen-tests/tests`, and `crates/ulo-transport/src/error.rs`.

**Date** 2026-11-01. **Tip** `331c35b9` on `experiment/di-redesign`.

**Two facts about the record.**

1. Part of the gRPC build ran on old `master` before the pause. Rows 16-21 merged in `3cc36510` (#355 F239, #356 F236, #357 F235, #358 F241 doc half, #359 the `add_service` warning, #370 F237 request side and the F241 trailer). `3cc36510` is an ancestor of HEAD. The rebuild then replaced every file those PRs touched. Status below is judged against the rebuild's code, not against those merges.
2. **F241 has no entry in `FRAMEWORK_GAPS.md`.** `grep -n F241` finds only line 1149, inside F289, which names F240. F235, F236 and F239 carry "Closed by #355/#356/#357" notes. F241 appears to have been deleted rather than closed with a note.

---

## Summary

| Status | Count |
| --- | --- |
| built (including "built differently") | 32 |
| superseded | 29 |
| open | 8 |
| dropped | 8 |
| UNVERIFIED | 2 |
| method, no status | 1 |
| **total** | **80** |

A probe row counts as built when its defect is fixed in code. Where the probe's case is not asserted by any test, the row says "unpinned" and G-PINS collects it. The open items are F237-b, F237-c, G-INTEROP, G-INTEROP-2, G-TRAILER, H-verify, G-SHED and G-PINS. The UNVERIFIED items are P6 and the interceptor case under the last section.

### The five most valuable open items

1. **G-SHED (F63 / S3): the code a load shed answers.** The rebuild answers UNAVAILABLE (`crates/ulo-grpc/src/dispatch.rs:159-161`). That contradicts the audit's spec reading (statuscodes.md gives RESOURCE_EXHAUSTED for "Server temporarily out of resources") and the user's approved S3 ruling. The only record of the change is a "for the record" line in `integrations/QUESTIONS_5.md:84`, and it carries no argument.
2. **F237-c: the reserved `grpc-` prefix is not screened on reply metadata.** `RESERVED` lists eight names (`dispatch.rs:106-115`), and `Response::metadata` accepts any other ASCII key (`transport.rs:203-211`). The ledger already holds this as F237's surviving clause.
3. **G-TRAILER: no trailing-metadata API.** `Response<T>` writes headers only, and the trailers are the encoder's own. Before the rebuild this was tonic's limit. Now that ulo writes the server, it is ulo's. The ledger does not hold it.
4. **G-PINS: four behaviours the audit proved are not pinned by any test at HEAD.** They are unary abandonment firing `ClientCancelled`, repeated and `-bin` request metadata reaching a guard, the gRPC guard refusal's message (only the code is asserted, which was F238's own failure mode), and an aliased return type compiling (the spelling probe's inversion).
5. **G-INTEROP: the decision not to adopt the gRPC interop suite rests on "it would measure tonic".** That premise no longer holds at HEAD. ulo now owns the HTTP/2 server, `grpc-timeout` enforcement, status and trailer writing, UNIMPLEMENTED, and the compression refusal.

---

## Items

### Findings

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F235 | Abandoned unary call drops the handler future and never fires the token; detached work runs on | built | `CancelOnDrop` is armed around the pipeline and disarmed at a clean answer (`dispatch.rs:196, 208, 592-602`). `CallBody::drop` fires `ClientCancelled` for a body dropped before its end (`dispatch.rs:1018-1026`). DESIGN §2.6 X5, §6.2 Cancellation | No test pins the unary case (see G-PINS). The audit's probe shape is the test |
| F236 | `grpc-timeout` read and enforced nowhere; streaming reply outlives the deadline | built | The pipeline races the deadline and cancels with `Deadline` before the drop (`dispatch.rs:197-207, 614-629`). The deadline moves into the body and ends a running stream with DEADLINE_EXCEEDED (`dispatch.rs:953-1005`). Pinned by `deadlines.rs:555` `a_deadline_passing_during_a_streamed_reply_ends_it_with_deadline_exceeded` | none |
| F237-a | Request metadata flattened: repeated key keeps last, `-bin` keys dropped | built differently | `GrpcMetadata` wraps tonic's `MetadataMap` whole (`transport.rs:162-182`), so repeated and binary values survive. A guard reads it through `cx.metadata()`. A client-streaming handler reaches it through the non-consuming `GrpcMetadata` extractor (`extract.rs:92-96`) | `GrpcMetadata::get` answers the **first** value of a repeated key (http `HeaderMap::get`), where the old `header(k)` answered the last. Nothing pins either. See D1 |
| F237-b | Reply side refuses `-bin` keys | open (limit) | `Response::metadata(key: &'static str, value: &str)` parses `MetadataKey::<Ascii>` and drops a `-bin` key with a `warn` (`transport.rs:203-211`). An interceptor can still write any header through `Reply::headers_mut()`, base64 included | A binary reply header has no typed route. Not a defect if stated. The doc on `Response::metadata` names "valid ASCII metadata" and not the `-bin` refusal by name |
| F237-c | Neither side screens the reserved `grpc-` prefix | open; ledger F237 (surviving clause) | `RESERVED` holds eight names: `te`, `content-type`, `content-length`, `grpc-status`, `grpc-message`, `grpc-message-type`, `grpc-status-details-bin`, `grpc-encoding` (`dispatch.rs:106-115`). `with_metadata` filters only those (`dispatch.rs:428-438`), so `grpc-anything` passes | Spec: PROTOCOL-HTTP2.md reserves the `grpc-` prefix for future gRPC use (cited `GRPC_AUDIT.md` §4). A fix is a prefix test beside the list |
| F238 | gRPC puts the guard's chain index (`"guard 0 rejected request"`) and a panic payload on the wire | built | `GuardRejected { guard }` keeps the name for the error handlers (`crates/ulo/src/error/mod.rs:546-566`). `from_boxed` renders it `Forbidden`/`"forbidden"` (`crates/ulo-transport/src/error.rs:251, 293-295`). A panic renders `Internal` with a generic message (`error.rs:290-292`) | Unpinned on the wire: `enhancers.rs:194-210` and `grpc_hand_written.rs:332-333` assert the code only, the exact gap the audit named at `rpc_grpc_macros.rs:879-887` |
| F238-spec / F130 gRPC half | A `bool` guard cannot tell PERMISSION_DENIED from UNAUTHENTICATED, which statuscodes.md separates | built | `Guard::can_activate -> Result<bool, BoxError>`. `Ok(false)` is PERMISSION_DENIED, and `Err(CallError::unauthorized(..))` is UNAUTHENTICATED (`crates/ulo/src/transport/mod.rs:70-82`). `code_for` maps `Unauthorized => Unauthenticated` (`status.rs:24`) | Spec citation worth keeping in the guard docs: statuscodes.md, PERMISSION_DENIED "must not be used if the caller can not be identified (use UNAUTHENTICATED instead for those errors)" |
| F239 | `parse_grpc_timeout` panics on a value ending mid-character (`"5€"`) | superseded | The public constructor is gone (`GrpcCx::new` is `pub(crate)`, `transport.rs:73`). `parse_timeout` checks `is_char_boundary` (`dispatch.rs:539-543`). On the wire a non-visible-ASCII value fails `to_str` and is refused INTERNAL before any execution opens (`dispatch.rs:524-537`) | No test sends a non-ASCII `grpc-timeout`. `deadlines.rs:460-497` covers the ASCII off-grammar cases |
| F240 | `#[grpc_methods]` reads the return type by name (`Result`, `Response`); aliases fail to compile | built (superseded mechanism) | No name read. A six-arm autoref `ReplyProbe` ranks on the concrete type and checks message and shape against the marker (`crates/ulo-grpc/src/__private.rs:143-263`). The attribute spans the check at the return type (`crates/ulo-grpc-macros/src/method.rs:105-114`) | The inversion test the handoff required (spelling probe must compile) was never added. See G-PINS. Ledger F240 has no closure note |
| F241-doc | `details()` does not travel on gRPC; state it | superseded | Details travel (next row), so the limit no longer exists | none |
| F241-trailer | Write `grpc-status-details-bin` | built differently | `with_details` maps the typed `Detail` enum one-to-one onto `google.rpc` messages through tonic-types. `Detail::Json` becomes a `google.protobuf.Value` in an `Any` (`status.rs:97-171`). Pinned by `grpc_status_details.rs:42-122` | Halt 2's option 3 (the rich error model), not the chosen option 2 (one `Struct` in an `Any`). The shape rule Halt 2 said was missing is the `Detail` enum (DESIGN §2.4). Ledger entry missing (see header) |
| F63-corr | The append: statuscodes.md answers F63; "tonic's table maps from no `ErrorKind`" was wrong | superseded, against the audit and S3 | See G-SHED | See G-SHED |
| F75 | A wrong `Payload<T>` fails on first call, not at compile time; designed fix `GrpcReads`, parked | built | `ParamProbe` checks each body-consuming parameter's message type and shape against `M::Request` and `M::SHAPE` at compile time (`__private.rs:52-122`). UI tests: `grpc_request_of_another_message.rs`, `grpc_streamed_request_for_a_unary_method.rs`, `…_beside_a_user_matches.rs` | The audit's "one marker, two questions" coupling (`GRPC_AUDIT.md` §7) is what was built: `Method { PATH, SHAPE, Request, Response }` serves both checks (`crates/ulo-grpc/src/method.rs:26-32`). The cost that parked F75, a trait every custom extractor implements, was avoided: a non-body parameter falls to `NotBody` and compares `()` with `()`. Ledger F75 is still open |
| F16 | One struct cannot serve two proto services | superseded | Each handler binds its own marker, and the route is `<Marker as Method>::PATH` per handler (`method.rs:69-71` in the macros crate). The server's table is keyed by path (`server.rs:259-305`), so nothing ties an impl to one service | Unpinned; ledger F16 still open |
| F129 | gRPC keeps a second interceptor chain | superseded | One `ulo::dispatch` for every transport (`dispatch.rs:332`) | none |
| F1 / F251 gRPC half | An extraction or validation failure renders INTERNAL on gRPC | superseded (touched, owned by extraction) | `ExtractError` maps to `BadRequest`/`Unprocessable` (`error.rs` `from_boxed` table). DESIGN §2.4: `Unprocessable` is INVALID_ARGUMENT with `BadRequest` field violations | none |

### The seam and the interop suite (`GRPC_AUDIT.md` §1, §2)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| G-SEAM | Seam: tonic owns framing, content type, trailers, status encoding, flow control, keepalive, TLS, deadline enforcement and UNIMPLEMENTED | superseded | ulo now owns the HTTP/2 server over `ulo-server-hyper`, TLS through `ulo-net`, `grpc-timeout` parsing and enforcement, UNIMPLEMENTED, the compression refusal and status/trailer writing (`server.rs:1-2`, `dispatch.rs:1-59`, DESIGN §6.2 Engine). tonic keeps the codec, the clients, and the health and reflection services | The method survives: decide per surface whether the library or the framework writes it, and file a library defect as a dependency note. Applied at HEAD, more of the wire is ulo's to audit than the old cut allowed |
| G-SEAM-2 | "It reports, tonic enforces" is weaker than it reads: tonic's `GrpcTimeout` races only to the response head | superseded | ulo enforces through the body (`dispatch.rs:953-1005`) | Mechanism worth keeping for the client side, which is still tonic's: `tonic 0.14.6 src/transport/service/grpc_timeout.rs:41-62, 80-92`. The race ends when the inner service returns its `Response`, so for a stream it covers only the time to first headers |
| G-INTEROP | Do not adopt the `grpc-testing` interop suite: it measures tonic; its seam-reaching cases were already the findings; `custom_metadata` cannot be passed by any tonic server | open (premise changed) | See the first reason in the value column | The first reason no longer holds. Of the second, the cases that reach ulo's seam now include `timeout_on_sleeping_server`, `cancel_after_begin`, `cancel_after_first_response`, `status_code_and_message` and `special_status_message`, all written by ulo's dispatcher at HEAD. Case names cited from `interop-test-descriptions.md` via `GRPC_AUDIT_HANDOFF.md`. `custom_metadata` (`interop-test-descriptions.md:1440-1446`, echo `x-grpc-test-echo-trailing-bin` as trailing metadata) now fails for ulo's own reason, G-TRAILER. See the designer questions |
| G-INTEROP-2 | Pin instead: a mid-flight abandon, a deadline on a streaming reply, a binary key on the way in | partly built | Deadline-on-stream is pinned (`deadlines.rs:555`). The other two are not | See G-PINS |
| G-DEP1 | Dependency note: tonic maps its timeout to CANCELLED where statuscodes.md names DEADLINE_EXCEEDED (`tonic 0.14.6 src/status.rs:644-646`) | superseded server side; holds client side | ulo's server writes DEADLINE_EXCEEDED (`status.rs:88-95`). DESIGN §6.2 Deadlines: a caller through tonic's `Channel` gets CANCELLED from its own `GrpcTimeout` and never reads the server's answer | Keep the citation in the client docs (`outgoing`). It is why a tonic client never observes a ulo error handler's reshaped timeout |
| G-DEP2 | Dependency note: `tonic::Response` has no trailer API (`tonic 0.14.6 src/response.rs:26-124`) | superseded by G-TRAILER | ulo's `Response<T>` is its own type (`transport.rs:188-216`) | — |
| G-TRAILER | (derived) no API for custom trailing metadata | open; not in ledger | `Response<T>` carries `metadata` written as headers (`transport.rs:191-211`, `dispatch.rs:428-438`). The trailers are `EncodeBody`'s `grpc-status`/`grpc-message`/`grpc-status-details-bin` only | gRPC defines Custom-Metadata in trailers as well as headers (PROTOCOL-HTTP2.md `Trailers → Status [Status-Message] *Custom-Metadata`). Whether ulo needs it is a design call. If yes, `Response::trailer(k, v)` is the shape that mirrors `.metadata` |

### Checked and found sound (`GRPC_AUDIT.md` §6)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| G-S1 | `with_drain_timeout`, `with_max_inflight`, `with_max_per_connection` work | built differently | `max_inflight` (1,024 at `Default`), `max_per_connection`, `max_concurrent_streams` (`server.rs:119-139`). The drain is the core's. Pinned by `grpc_admission.rs:36-100` | The refused code changed (G-SHED) |
| G-S2 | Bind is synchronous, so a port conflict is an `Err` from `bind`; a bad certificate is a startup error | built | TLS loads in `prepare`, and a lone TLS failure returns the `TlsError` itself (`server.rs:367-385`). Bind is `ulo_net::bind_all` (`server.rs:414`) | none |
| G-S3 | `ulo-build`'s six error variants each name their cause | dropped | `compile` returns `io::Result` (`crates/ulo-build/src/lib.rs:69-102`). The one ulo-authored message names the fix for an unset `OUT_DIR` (`lib.rs:72-74`). Every other error is prost-build's | none |
| G-S4 | `compile_protos` ends in `shapes_in_out_dir()`, which rewrites every `.rs` in `OUT_DIR`; an unstated ordering constraint | superseded | Markers are appended through a `ServiceGenerator` wrapping tonic's, per service, inside prost-build's own write (`crates/ulo-build/src/markers.rs:36-58`). No post-pass rewrites files | none |
| G-S5 | `GrpcCode::from_i32` has no arm for 2; `_` answers Unknown | dropped | `GrpcCode` is gone. `tonic::Code` is used directly, and `to_status` ignores a `grpc_code` outside 1-16 (`status.rs:58-65`) | none |

### §6a: gRPC against the other audits' settled decisions

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| 6a-1 | Pipeline API: one type parameter, `Guard<Grpc>` | built | `Guard<T: Transport>` with `T::Cx` (`crates/ulo/src/transport/mod.rs:81-83`) | none |
| 6a-2 | `HasHeaders` / `GrpcReply::header` for the stamping case | superseded | `Reply` is `http::Response<tonic::body::Body>`, and an interceptor writes its headers (`transport.rs:35-41`) | none |
| 6a-3 | Pipeline D1: declaration order is the contract | built (owned by pipeline) | One `ulo::dispatch` | none |
| 6a-4 | Pipeline D4: a guard answers `Result` | built differently | `Result<bool, BoxError>` (see F238-spec) | none |
| 6a-5 | F136: one error vocabulary | superseded | See D4 | See D4 |
| 6a-6 | Extraction D1: `RequestError` is not `ulo::Error` | superseded | `ExtractError` maps through `from_boxed`. `TooLarge` over tonic's 4 MiB limit is INVALID_ARGUMENT (`extract.rs:16-18, 105-117`) | none |
| 6a-7 | DI D2, DI D14: nothing gRPC-specific | dropped | Old DI model replaced | none |
| 6a-8 | HTTP D3/D4 borrowed standard types; gRPC cannot borrow tonic's because core is tonic-free | superseded | The gRPC context lives in `ulo-grpc`, which names tonic freely (`GrpcMetadata` wraps `MetadataMap`, `transport.rs:164-182`) | The argument that decided D1 against option (b) dissolved once the transport left core. Recorded under D1 |
| 6a-9 | HTTP D6 / F241: `details()` not read on gRPC | built | See F241-trailer | none |
| 6a-10 | ADR-0050 / F240: no `IntoOutput<Grpc>` | built differently | `IntoReply<Grpc>` is implemented for the crate-local `Answer` behind the probe (`__private.rs:265-283`), because the orphan rule refuses a blanket over `prost::Message` (`__private.rs:6-11`) | The handoff predicted the E0119 wall and the orphan placement (`GRPC_BUILD_HANDOFF.md` PR 3). The rebuild met both and solved them with autoref rank plus a local carrier type |
| 6a-11 | RPC D3(b): carry the deadline and fire the token, nearly free on gRPC | built | DESIGN §2.4: "One deadline rule holds on every transport that has a deadline" | none |
| G-SYN1 | `#[grpc_stream]` is the better design and should not be "simplified" into inference from the return type, which would be a third name match | superseded | The marker attribute is gone. Shape comes from `Method::SHAPE`, which `ulo-build` reads off the proto (`markers.rs:97-102`), and the reply probe checks the handler against it (`__private.rs:46-50`) | The audit's objection was to inferring shape from **syntax**. The rebuild infers it from the **proto**, which that objection does not cover. The principle to keep: a fact the proto states is read from the proto, and the handler is checked against it |
| G-SYN2 | The attribute names the proto trait because the contract is in a file the macro cannot read | superseded | `#[ulo_grpc::method(pb::svc::Method)]` names a generated marker per handler, and no tonic server trait is generated (`build_server(false)`, `lib.rs:99`) | none |
| G-CLAUDE | `CLAUDE.md` row "(gRPC excluded — tonic fixes the signature)" contradicted ADR-0038 | dropped | Old workspace reference; old API gone | none |

### Decisions (`GRPC_AUDIT.md` §8)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| D1 | Metadata view: ASCII multimap plus binary map, `header`/`headers_all`/`header_bin`; rejected: hand guards tonic's `MetadataMap` (core is tonic-free); rejected: document the limit | superseded (the rejected option was taken) | `GrpcMetadata { map: MetadataMap }`, with `map()` and an ASCII `get` (`transport.rs:164-182`) | The rejection argument held only while the context lived in core. A transport-owned context may expose the transport library's types. The trade the rebuild accepted: `tonic` becomes part of `ulo-grpc`'s public API (already true through `Status`, `Reply` and `Streaming`). The ADR-0020-style `get`/`get_all` split was not reproduced. `get` returns the first value |
| D2 | `add_service` bypasses the pipeline; the split by what an enhancer reads (pre-dispatch tier above the router reaches every service; guards inside the method reach ulo-dispatched ones only); startup `warn` | superseded | No `add_service`. The only non-handler services are health and reflection, routed after the unscoped pre-dispatch entries with no scoped entries and no error handlers (DESIGN §6.2 Health; `dispatch.rs:279-302`) | Two parts worth keeping. **The stance, for the record:** "the pipeline reaches what ulo dispatches; what ulo does not dispatch gets the pre-dispatch role and nothing below it", which is what HEAD does for its builtins. **The argument against per-enhancer tier declaration:** whether a guard reads the message is a runtime fact invisible in the type, so the framework cannot place it. At HEAD the codec boundary moved: guards run **before** decode and cannot read the message (DESIGN §6.1, "How a marker becomes a service"; `dispatch.rs:54-59`). See designer question 4 |
| D2-warn / row 21 (GRPC-5) | Startup `warn` per `add_service` service | dropped | Nothing to warn about: no user-registered raw service exists | none |
| D2-anchor / row 101 (PIPE-12), S13 | Anchor gRPC's pre-dispatch role above tonic's router | built differently | `PreDispatch<Grpc>` runs in the dispatcher before routing (unscoped) and after it (scoped by path) (`pre_dispatch.rs:1-27`, `dispatch.rs:263-314`). A non-200 answer is translated by the spec's HTTP-to-status table (`status.rs:39-48`, `dispatch.rs:486-500`) | The HTTP-to-gRPC table (400 INTERNAL, 401 UNAUTHENTICATED, 403 PERMISSION_DENIED, 404 UNIMPLEMENTED, 429/502/503/504 UNAVAILABLE, else UNKNOWN) is the spec's. Pinned at `enhancers.rs:242` |
| D3 | Scope of the abandonment fix: (i) gRPC only, or (ii) every transport and both answer shapes, gRPC first | built as (ii) | `CancelReason { Disconnected, ClientCancelled, Deadline, Drain, Explicit }` fired per wire on every transport (DESIGN §2.6 X5) | The falsifiability argument for sequencing (`GRPC_AUDIT.md` §8 D3) is a reusable method: build the cross-transport mechanism first where it needs no wire-grammar change, so a wrong design is wrong once. Also: a design validated only where the **library** owns the drop site has not been tested where the framework owns it |
| D3-rows (131-133), S2 | HTTP, RPC and WS buffered-answer drop sites | superseded | X5 above. F245 (RPC cancel aborts without firing) is outside this subject | none |
| D4 | Error vocabulary: `Status{kind,message}` / `Custom{code, message, source}` / `AppError`, `Custom` carrying a source on all four transports; rejected: grow `ErrorKind` with gRPC's codes; rejected: exempt gRPC | superseded, with one loss | One `CallError { kind, message, details, challenge, grpc_code, source }` for all transports (`error.rs:83-96`). `with_grpc_code(i32)` names an off-kind code and `with_source` keeps the typed error (`error.rs:181-219`). A returned `tonic::Status` reaches the wire as it stands (`status.rs:80-85`) | **What D4 wanted on every transport exists on gRPC only.** HTTP has no `with_http_status`, so a handler cannot name an HTTP status outside the eleven kinds' table while keeping `#[catch]`-reachable typed errors. D4 called this "a capability the other three lack and nobody has noticed" (`GRPC_AUDIT.md` §8 D4). The rejected-alternative argument stands and is reusable: a wider wire code space is met by an escape-hatch field, not by growing the shared kind vocabulary, which would force HTTP to invent a status for `FailedPrecondition`, `Aborted`, `OutOfRange`, `AlreadyExists`, `DataLoss` and `Cancelled`. See designer question 3 |
| D4-row (56, PIPE-7) | Build the three-arm vocabulary | superseded | As above | — |

### The build plan (`GRPC_BUILD_HANDOFF.md`, `BUILD_SEQUENCE.md`)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| row 16 / GRPC-1 | F239 | superseded | See F239 | — |
| row 17 / GRPC-7 | F236, delete the token doc's falsified sentence | built | The sentence ("A buffered answer never fires it … nothing that could observe the token is still alive to") does not occur in the tree (grep, `crates/`) | The sentence is worth keeping as a negative example. It justified a missing guarantee by asserting no observer exists, and detached work holding a clone is that observer |
| row 18 / GRPC-8 | F235 drop guard, disarmed at a clean return | built | `CancelOnDrop` (`dispatch.rs:592-602`) | Handoff verification, still owed: a client dropping a unary call mid-flight fires the token and detached work stops. **Falsify against a call that completes normally, which must not fire it** |
| row 19 / GRPC-6 | F237 + F241 trailer | built differently | See F237-a, F241-trailer | — |
| row 20 / GRPC-4 | F241 doc half | superseded | — | — |
| row 21 / GRPC-5 | `add_service` warn | dropped | — | — |
| row 59 / GRPC-2 | F238 rendering | built | See F238 | — |
| row 61 / GRPC-3 | F240 structural, spelling probe compiles; read F75 first | built | See F240, F75 | — |
| H-gate | Divergence gate: PR 3 needing a change to `ulo-build`'s public API; PR 3 shipping the diagnostic only | dropped | Old API replaced | none |
| H-verify | Per-PR verification lines | partly built | See G-PINS | The lines are test specifications and are kept under G-PINS |
| Halt 2 | Encoding of `grpc-status-details-bin`: three options; chosen option 2 | superseded | Option 3 built (F241-trailer) | Halt 2's reader argument stays relevant to the designer: every client decodes `google.rpc.Status`, a `Struct`/`Value` detail is readable everywhere, and typed details only by clients that know the types. HEAD writes typed details where `Detail` names one and a `Value` otherwise |
| S3 | `overloaded` → `TooManyRequests` on the gRPC-parity argument, approved 2026-09-24 | superseded, against the ruling | See G-SHED | See G-SHED |
| S5 | F242 (panic payload on the wire), riding with GRPC-2 | built (owned by render) | gRPC panic renders generic `Internal` (`error.rs:290-292`, `dispatch.rs:967-977` for a body panic) | none |

### The audit brief's own items (`GRPC_AUDIT_HANDOFF.md`)

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| B-1 | Parked memory entry: gRPC handler DX parity (macro writes the tonic trait) | superseded | No tonic trait is written. Handlers sit in `#[routes]` beside other transports (DESIGN §6.1) | none |
| B-2 | Parked: `#[grpc_method(...)]` per-method metadata | built | `#[meta]` on a gRPC method; UI test `meta_on_grpc_method_names_another_transport.rs` | none |
| B-3 | Parked: TLS, reflection, `tonic-build` wrapper out of scope | built | `Server::tls` (`server.rs:113-117`), reflection v1 and v1alpha (`server.rs:157-168, 360-365`), `ulo-build` | Reflection is on by default in debug builds only (`server.rs:99`, DESIGN decision 7) |
| B-4 | Candidate: `ulo-build`'s three ways to point at a proto compiled elsewhere (`shapes`, `shapes_in_out_dir`, `shapes_in_file`) | dropped | One entry point, `configure().compile(..)` (`lib.rs:41-102`) | A crate whose protos another build step compiles has no route to markers at HEAD. Not filed. Raise only if a user needs it |
| B-5 | Method: separate tonic's defects from ulo's per finding; read the library before writing a mechanism | n/a (method) | — | Reusable for any pass over a transport library |

### Probes

| ID | What it proves | Status of the claim at HEAD | Value to extract |
| --- | --- | --- | --- |
| P1 (`probe_deadline_expiry`) | Unary handler past a 300 ms deadline: future dropped, token not fired, spawned task ran to completion | fixed; pinned | `deadlines.rs:320` asserts the drop and the `Deadline` reason. The detached-work half (a spawned task holding the execution observes the cancel) is not asserted |
| P2 (`probe_unary_abandoned`) | Client dropping a unary call at 250 ms: same result | fixed in code; **unpinned** | Test shape: handler awaits 1.5 s while a spawned task selects on the execution's cancellation against a 1.2 s sleep. Client drops at 250 ms. Assert the task saw `ClientCancelled`. A control call that completes must not fire it |
| P3 (`probe_metadata_shapes`) | `x-tag: first, second` and `x-trace-bin` reach the handler as `x-tag=second` and no `-bin` | fixed in code; **unpinned** | Test shape: append `x-tag` twice and `insert_bin("x-trace-bin", [DE AD BE EF])`. Assert a **guard** reads both `x-tag` values in order through `cx.metadata().map().get_all` and the four bytes through `get_bin` |
| P4 (`probe_load_shed`) | `with_max_inflight(1)` sheds a second call as RESOURCE_EXHAUSTED | changed: now UNAVAILABLE | See G-SHED |
| P5 (`probe_grpc_timeout_non_ascii`) | `GrpcContext::new` with `grpc-timeout: "5€"` panics ("end byte index 3 is not a char boundary") | constructor gone | A wire test sending obs-text bytes in `grpc-timeout` would pin the INTERNAL refusal |
| P6 (`probe_grpc_timeout_overflow`) | `99999999H`, `00000005S`, `0S` do not panic | changed | `0S` is now DEADLINE_EXCEEDED before admission (`dispatch.rs:156-158`, pinned `deadlines.rs:543-548`). `99999999H` is on the grammar. If `timer.now().checked_add(..)` overflows, the call opens with no deadline (`dispatch.rs:163-165`). UNVERIFIED on this platform, see the last section |
| P7 (`probe_streaming_deadline`) | 300 ms deadline on `watch_progress`: all 400 items delivered over 10 s, token never fired | fixed; pinned | `deadlines.rs:555`, `grpc_stream_cancel.rs:296` |
| P8 (`probe_guard_refusal_on_the_wire`) | Refused caller gets `PermissionDenied`, message `"guard 0 rejected request"` | fixed in code; message **unpinned** | Assert `status.message() == "forbidden"` and that the guard's type name appears in no wire field |
| SP (`grpc_spelling_probe.rs`) | `#[grpc_methods]` reads return types by name. Expected E0053 twice, at the attribute | fixed by mechanism; **unpinned** | **The probe file does not exercise its PROBE A.** Its header says `create` returns `Answer<CreateOrderResponse>`, but line 61 declares `Result<spell_pb::CreateOrderResponse, NotServed>`, so `type Answer<T>` (line 38) is unused and only the `Reply` alias (line 83) is tested. F240's ledger text reports both as probed. A HEAD test should declare both aliases: `type Answer<T> = Result<T, MyError>` on a unary handler and `type Reply<T> = ulo_grpc::Response<T>` inside a `Result`, each compiling and the first behaving as fallible |

### G-SHED: the code a load shed answers

**Status:** open, as a decision drift. The ledger holds F63 as open. The value of this record is that the reversal carries no argument.

**At HEAD.** Over the server's or a connection's in-flight bound, gRPC answers UNAVAILABLE "the server is at its in-flight limit", with no `RetryInfo` (`dispatch.rs:159-161`; DESIGN §6.2 "Load shedding and settings": "clients treat it as retryable"). RPC answers `unavailable` with a `RetryAfter` detail (`crates/ulo-rpc/src/dispatch.rs:216`). HTTP answers 503 with `Retry-After` (DESIGN §3.6). The rebuild unified the three on `Unavailable`, which was F63's original second option.

**What the old record decided.**

- The gRPC audit (`GRPC_AUDIT.md` §5, F63's append): statuscodes.md defines RESOURCE_EXHAUSTED as "Some resource has been exhausted, perhaps a per-user quota, or perhaps the entire file system is out of space", one code covering a caller-fault quota and a server condition alike. Its library table gives RESOURCE_EXHAUSTED for "Server temporarily out of resources (e.g., Flow-control resource limits reached)". On that reading the caller-fault versus server-condition split F63 rested on is one the spec declines to make. The old server already answered RESOURCE_EXHAUSTED. The old `code_for(TooManyRequests) => ResourceExhausted` already equated the two, and HEAD keeps that mapping (`status.rs:28`).
- S3 (`BUILD_SEQUENCE.md` row 122, approved 2026-09-24): `overloaded` → `TooManyRequests` on the gRPC-parity argument.

**What HEAD's record says.** `integrations/QUESTIONS_5.md:84`, under "For the record (no ruling needed)": "The gRPC admission bounds answer `UNAVAILABLE`, not `RESOURCE_EXHAUSTED`; the inventory's old name was wrong." That treats the change as a naming correction to an inventory, not as a reversal of a ruling.

**The arguments on each side**, for the designer. For UNAVAILABLE: statuscodes.md describes it as "most likely a transient condition, which can be corrected by retrying with a backoff", and generic retry policies retry it. One kind across three transports. For RESOURCE_EXHAUSTED: the spec's library table names this exact case, and it is what the old server shipped and what the user approved. A further point on HEAD's side: gRPC's shed carries no `RetryInfo` where RPC's carries `RetryAfter` and HTTP's carries `Retry-After`, an asymmetry inside the unified choice.

### G-PINS: behaviours the audit proved, unpinned at HEAD

All four are built in code. None has an assertion that would fail if it regressed.

1. Unary abandonment fires `ClientCancelled`, observed by detached work (P2). `CancelOnDrop` at `dispatch.rs:592-602`. The only `ClientCancelled` tests are for streams (`stream_end.rs:233-245`, `grpc_execution_end.rs:209`).
2. Repeated and `-bin` request metadata reach a guard (P3).
3. The guard refusal's message on gRPC is `"forbidden"` and names no guard (P8). Existing tests assert the code only (`enhancers.rs:194-210`, `grpc_hand_written.rs:332-333`), the same gap that let F238 survive at `rpc_grpc_macros.rs:879-887`.
4. Aliased return types compile and keep their fallibility (SP, with both aliases written in).

The handoff's rule applies to each: self-test it by planting the defect first (`GRPC_BUILD_HANDOFF.md` "Verification each PR needs").

---

## Worth discussing with the designer

1. **Shed code (G-SHED).** Keep UNAVAILABLE on gRPC, or answer RESOURCE_EXHAUSTED per statuscodes.md's library table and S3? *Recommendation:* decide it explicitly and record the argument in DESIGN §2.8 or §6.2, since today the record treats an approved ruling as an inventory typo. If UNAVAILABLE stays, attach `RetryInfo` from `shed_retry_after` so gRPC matches RPC and HTTP.
2. **Interop suite (G-INTEROP).** Should a subset of `grpc-testing`'s cases run against `ulo_grpc::Server` now that ulo writes the server? *Recommendation:* yes, the subset that reaches the dispatcher: `empty_unary`, `large_unary` (exercises the 4 MiB limit), `timeout_on_sleeping_server`, `cancel_after_begin`, `cancel_after_first_response`, `status_code_and_message`, `special_status_message`, `unimplemented_method`. Drop `custom_metadata`'s trailing half until G-TRAILER is decided.
3. **D4's cross-transport half.** gRPC can name an off-kind code and keep the typed source (`with_grpc_code` plus `with_source`). HTTP cannot name a status outside the kinds' table at all through `CallError`. Was that intended? *Recommendation:* if any HTTP user needs, say, 409-with-typed-error versus 412, add `with_http_status(u16)` mirroring `with_grpc_code`. That is the one-field cost D4 named.
4. **Guards cannot read the message.** The old #311 let a guard read a decoded copy (`ctx.message::<T>()`). HEAD decodes after the guards (DESIGN §6.1). A guard authorising on a field of the request (tenant id in the message) now has to live in the handler. *Recommendation:* confirm this is the intent and say so in the `Guard` docs for gRPC. D2's own reasoning shows a pre-decode tier cannot read the message, so the answer is a statement, not a feature.
5. **Reserved prefix (F237-c).** Screen `grpc-*` on reply metadata. *Recommendation:* yes, a prefix test beside `RESERVED`, dropped with a `warn` as an invalid key already is. Low cost, spec-backed.
6. **Trailing metadata (G-TRAILER).** Add `Response::trailer(k, v)`? *Recommendation:* no until a use appears. File it in the ledger so the limit is recorded rather than inherited silently from tonic.
7. **`GrpcMetadata::get` on a repeated key** answers the first value. The old view answered the last, and D1 wanted both reachable by name. *Recommendation:* state "first value" in the doc. `map().get_all` already reaches the rest.
8. **Ledger hygiene.** F238, F240, F75, F16 and F63 read open in `FRAMEWORK_GAPS.md` though HEAD resolves or reverses them, and F241's entry is missing. This is the lead's call; the reader does not edit the ledger.

## Not verifiable here

- **P6 / `99999999H`.** Whether `Timer::now().checked_add(Duration::from_secs(99_999_999 * 3600))` overflows on the tokio clock on this platform. If it does, the call opens with **no** deadline (`dispatch.rs:163-165`) rather than the 11,415-year one. Needs a run. The practical effect is nil, but the branch is silent.
- **An interceptor writing `grpc-status` into a 200 reply's headers.** `RESERVED` filters `Response::metadata` and pre-dispatch answers, but an interceptor holding `Reply` can call `headers_mut().insert("grpc-status", ..)`. What tonic's client then reads (headers-carried status versus the trailers') needs a run.
