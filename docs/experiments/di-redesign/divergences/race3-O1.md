# Divergences: race 3, area O1 (the carriers in the four hubs)

Model: Opus. Stage 5. Every place O1 departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md`, the
fourth and fifth responses or the spine's entries in `race3-S.md`, and every point the plan leaves
to O1. Each entry gives what the design says, what the code does, and why. All await the user's
sign-off.

No cargo was run. Library facts are read from the registry under `~/.cargo/registry/src`: `http`
1.5.0 and `tonic` 0.14.6, the versions `Cargo.lock` holds.

## Files

| File | Lines | What O1 wrote |
|---|---|---|
| `crates/ulo-http/src/trace.rs` | 40 | `extract`; `record_status`, the 5xx rule |
| `crates/ulo-http/src/service.rs` | 883 | one line: `trace::record_status` beside `http.response.status_code` |
| `crates/ulo-grpc/src/trace.rs` | 78 | `extract`; `inject`; `record_status`, the gRPC code rule |
| `crates/ulo-grpc/src/dispatch.rs` | 893 | one line: `record_status` calls `trace::record_status` |
| `crates/ulo-rpc/src/trace.rs` | 78 | `extract`; `inject` with the `tracestate` merge; `record_failure`, the kind rule |
| `crates/ulo-rpc/src/dispatch.rs` | 752 | `send` and `settle_event` record a failure on the call's span |
| `crates/ulo-ws/src/trace.rs` | 70 | `extract`; `record_failure`; `record_refusal` |
| `crates/ulo-ws/src/connection.rs` | 1588 | the connect phase, a message and a streamed answer record a failure on their span |

Not edited, though the plan's O1 table lists them: `crates/ulo-grpc/src/client.rs` and
`crates/ulo-rpc/src/client.rs`, whose injection call sites S-core wrote as the plan asks (C19) and
which need nothing further; `crates/ulo-ws/src/gateway.rs` (O3 below). No `lib.rs` was touched:
S-core wrote each `mod trace;`. No `todo!()` remains in the four hubs.

## Points the plan leaves to O1

P1. **Which failures mark a server span as an error.** The design states the rule for HTTP alone,
a 5xx and not a 4xx (§6.4). `otel.status_code` is recorded as `error` and never as `ok`:
OpenTelemetry leaves a successful server span's status unset.
- HTTP: `StatusCode::is_server_error`, read where the service records
  `http.response.status_code`. A body that fails after its head was written changes no status and
  marks nothing.
- gRPC: OpenTelemetry's gRPC conventions for a server span: `UNKNOWN`, `DEADLINE_EXCEEDED`,
  `UNIMPLEMENTED`, `INTERNAL`, `UNAVAILABLE` and `DATA_LOSS`. Read wherever the dispatcher already
  records `rpc.grpc.status_code`, a trailers-only answer and the trailers of a body alike, so a
  stream ending on an error item counts. A path off the table is `UNIMPLEMENTED` and counts, as
  the convention has it; a pre-dispatch entry's HTTP answer counts by the code the gRPC table gives
  it (503 is `UNAVAILABLE` and counts, 401 is `UNAUTHENTICATED` and does not).
- RPC and WebSocket: the kinds HTTP answers with a 5xx, `Timeout`, `Unavailable`, `Unimplemented`,
  `Internal` and any kind added later, as `ulo_http::render::status_of` maps an unknown kind to 500.
  The seven 4xx kinds leave the status unset. One rule for the two transports that render
  `ErrorKind` keeps a kind the same verdict wherever it is served. On RPC every failure a caller is
  told of is an `err` frame, so `send` records it, the oversized-reply replacement included; an
  event writes no frame, so `settle_event` records the kind it logs, a passed deadline as `Timeout`.
  A call cancelled by its caller writes nothing and marks nothing; one cut at the drain's end
  answers `unavailable` and counts. A pattern nothing handles answers `Unavailable` with
  `pattern_unhandled` and counts, as gRPC's unknown method does.
- WebSocket: a message's `Err`, a reply frame the gateway's codec cannot write (`Internal`) and a
  streamed answer's late error, whichever error the error handlers leave written. An event nothing
  handles is `Unimplemented` and counts. A connect refused with a kind follows the kind, a guard's
  `Forbidden` not counting; one refused for capacity counts as `Unavailable`; one refused with a
  close code alone (`ConnectRefused::code`) counts for 1011, 1012, 1013 and 1014, the codes
  RFC 6455 §7.4 and the IANA registry give a server's condition. A refusal before the span exists,
  the drain's 1001 or a session that cannot be built, marks nothing.
- An RPC event's consumer span follows the server rule. OpenTelemetry's messaging conventions mark
  a consumer span failed when its processing failed; reading a handler's `BadRequest` as the
  producer's mistake keeps the event beside the request it resembles.

P2. **The `otel.kind` each non-HTTP span records.** S-core's arguments are kept (C13): gRPC
`Server`; RPC `Server` for a request and a streamed request, `Consumer` for an event (§6.2);
WebSocket `Server` for the connect phase and for every message. A WebSocket message without an id
expects no answer, but its span's parent is the connection's own context, set on this server, not
a producer span the client opened, which is what `Consumer` would claim; and OpenTelemetry has no
WebSocket convention to follow.

P3. **Where the WebSocket connection keeps its context.** S-core's `ConnInner::trace`, a
`OnceLock<TraceContext>` set from `exec.trace()` right after the connect phase's `span::call`, and
`Connection::exec_options`, which opens a message's, a hand-written gateway's and the terminal
`on_disconnect`'s execution with it (C18), are kept as written. Read against the plan's order: the
handshake's context is extracted before the connect execution opens; the context stored is the
connect execution's own with a source bound and the handshake's without; a message's execution
receives it through `ExecOptions::trace`, marked `Received`, and its own `span::call` replaces it
with an `Originated` one when a source is bound.

## Decisions within the frozen helpers

O4. **A header value that is not visible ASCII.** `HeaderValue::to_str` and `MetadataValue::to_str`
refuse anything outside visible ASCII and tab (`http-1.5.0/src/header/value.rs:243-260`,
`tonic-0.14.6/src/metadata/value.rs:434`), which neither header's grammar admits. A `traceparent`
field that is not text is read as off the grammar, and the call carries no context, as H4 has an
invalid one. A `tracestate` field that is not text discards the whole list, as a member off the
grammar does in `TraceState::parse`, and leaves the `traceparent` standing (§3.3). A pass-through
then forwards no `tracestate`: a list that is not text cannot be kept as received text.

O5. **RPC extraction matches the header names without regard to case.** The dispatcher's guard
(`traced`) and the client's reservation (C16) already do. A broker keeps the case a producer wrote,
and a `Traceparent` beside a `traceparent` is two fields, which `TraceContext::extract` reads as a
restart.

O6. **RPC injection.** Every `traceparent` and `tracestate` entry is removed, whatever its case,
before the context's are written, so the frame carries one of each. The reservation has already
refused a caller's `traceparent`, and its `tracestate` beside a `Received` context, so on those
calls the removal finds nothing. Into an `Originated` context the caller's entries are merged by
`TraceState::prepend` (C8). A caller's `tracestate` off the grammar parses as an empty list and is
dropped, logged at `warn`, the context's list written alone: `inject` has no error to return, and
the reservation that could refuse runs before the frame, where `prepend`'s parse is not.

## Divergences

O1-D1. **`ulo_grpc::outgoing` replaces a caller's trace headers rather than refusing them, and
merges a caller's `tracestate` into an `Originated` context.** §6.3 states the reservation and the
merge for "the client", and I8 places them in the RPC client alone; for gRPC, §6.2 says only that
`outgoing` injects into the request's metadata, and S-core's `inject` doc said "in place of any the
caller set". `outgoing` already writes `grpc-timeout` over a caller's value rather than refusing
it, and `trace::inject(&TraceContext, &mut MetadataMap)` returns nothing, a contract S-core froze;
a refusal would need a changed helper and a new error from `outgoing`. What was written:
- `traceparent`: the context's, in place of a caller's.
- `Received`: the `tracestate` text as received, a caller's replaced, as §3.4 requires of a list
  passed through with its `traceparent`.
- `Originated`: a caller's entries merged by `TraceState::prepend`, as the RPC client merges them,
  so a vendor entry set on a gRPC call within a traced execution reaches the callee rather than
  being replaced.

A gRPC caller's `tracestate` within a forwarding execution is therefore dropped where the RPC
client refuses it. Making `outgoing` refuse both as the RPC client does is a change to S-core's
helper signature, for the coordinator to decide.

O1-D2. **A context whose `traceparent` text cannot travel as gRPC metadata is not written.** The
design is silent. A higher-version `traceparent` received over RPC keeps its trailing fields as
received, and an RPC header value may hold characters a metadata value refuses
(`AsciiMetadataValue::try_from` parses through `HeaderValue::from_str`,
`tonic-0.14.6/src/metadata/value.rs:375`). `inject` then writes neither header, removes a caller's,
and logs at `warn`: the callee starts a trace when it participates, and forwarding the text altered
would break §3.4. A `tracestate` that cannot be written is dropped and the `traceparent` written.

## Where the plan and the repository disagree

O3. **`crates/ulo-ws/src/gateway.rs` is not edited.** O1's table lists it, after the plan's S-core
row citing `gateway.rs:576` as a hand-written gateway's execution. Line 576 is `resolve`, a short
execution per gateway with no connection, as S-core found (C17); a hand-written gateway's
per-connection execution is `Connection::open_execution` in `connection.rs`, which opens with the
connection's context.

O7. **The RPC and WebSocket failures are recorded on `Span::current()` inside `send`,
`settle_event` and `late`, and in `pump`'s codec branch.** Each runs inside a future instrumented
with the call's span (`Call::run`, the unhandled path's task, a message's `pump`), so the current
span during their polls is the call's. Passing the span instead would widen `pump`, already at seven
parameters, past clippy's `too_many_arguments`, and every `send` call site. The connect phase and a
message's own outcome record on the span they hold.

## Requests to other owners

None.

## Not verified

- That the tree compiles: no cargo was run.
- `TraceState::parse` passed as a function value to `Option::map` (gRPC `inject`): a function with
  an `impl Trait` argument is usable as a value with its type inferred; only a turbofish is refused.
- The status rules at run time: the trace scenarios of §6.5 are the coordinator's tests.
