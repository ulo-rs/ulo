# Divergences: race 2b, tests batch 32, a caller's `deadline-ms` refused `reserved_header` before the call connects (F395), a `grpc-timeout` off the grammar refused INTERNAL before any execution opens, as grpc-go refuses it (F396), and a refused event never handed over again on RabbitMQ or Kafka (S4)

The fifty-seventh response, signed off 2026-10-17, answers `race2b-tests31.md`. No ulo path copies an
incoming call's headers into an outgoing one, so `deadline-ms` is reserved: a call of any shape whose
caller set it with `.header` fails when awaited, before it connects, `BadRequest` with the reason
`reserved_header`, readable or not, the value quoted as F394 quotes one. The gRPC server read every
`grpc-timeout` itself, tonic's server not being in its path, and ran a call whose value is off the
grammar with no deadline; grpc-go 1.84.0, probed in a temporary `golang:1.25` container, answers each
such form INTERNAL without running the handler. The dispatcher now answers INTERNAL too, the message
quoting up to 64 characters of the value, before admission and before any execution opens. RabbitMQ
rejects a refused delivery with `requeue: false` and Kafka stores the offset past a refused record,
so neither hands one over again; a new conformance scenario on every link holds it, its second half
running where a link declares `holds_unserved`. Two findings are filed and left: grpc-go answers a
zero `grpc-timeout` DEADLINE_EXCEEDED without running the handler, where ulo polls it once (F399),
and `traceparent` is listed as reserved while nothing writes, reads or refuses it (F400).

Files changed: for F395, `crates/ulo-rpc/src/{client.rs, transport.rs}` and the shared quoting in
`crates/ulo-rpc/src/dispatch.rs`; the three tests that set `deadline-ms` with `.header`, moved below
the client onto a new `Stamped` link, in `crates/ulo-rpc-conformance/src/cases/deadlines.rs`,
`crates/ulo-rpc-conformance/src/lib.rs` (its re-export) and `crates/ulo-rpc-tcp/tests/deadline_answers.rs`;
for F396, `crates/ulo-grpc/src/dispatch.rs` and `crates/ulo-codegen-tests/tests/deadlines.rs`; for S4,
`crates/ulo-rpc-conformance/src/cases/events.rs` and the scenario's line in `lib.rs`; in the workspace's
`FRAMEWORK_GAPS.md`, F399 and F400 new and fix notes under F395 and F396, their commit written
`<commit>`. The batch started from `57f0e727`, and no commit landed during it. Versions are unchanged
at `0.1.0-alpha.1`, and `Cargo.lock` is unchanged.

## The signatures

```rust
// crates/ulo-rpc-conformance, new and public: a client link writing `deadline-ms: value` on every
// `req`, `open` and `evt` it sends, in place of the client's stamp
pub struct Stamped<L> { pub link: L, pub value: &'static str }
impl<L: Link> Link for Stamped<L>;
pub use cases::deadlines::Stamped;

// crates/ulo-grpc/src/dispatch.rs, crate-private
pub(crate) fn grpc_timeout(headers: &HeaderMap) -> Result<Option<Duration>, Status>;  // was `-> Option<Duration>`

// crates/ulo-rpc/src/dispatch.rs, crate-private, shared by both refusals
pub(crate) fn quote(value: &str) -> String;
```

No public signature of `ulo-rpc` or `ulo-grpc` changes. What changes in meaning without one:

- `Call::header`, `StreamCall::header` and `Emit::header` take `deadline-ms` and the call then fails
  when awaited, first of all, before the zero checks and before connecting: `BadRequest`, reason
  `"reserved_header"`, message "the call set `deadline-ms` to "abc" with `.header`, and the client
  reserves that header, writing it from the call's bound; bound the call with `.timeout`, or forward
  an execution's deadline with `.within`" (a stream's ends "bound the stream with `.deadline`, ..."
  and an event's "an event is sent with no deadline"), the value quoted to 64 characters, ", cut
  short" after it past that. Where a request dropped the value for its own stamp and a stream, an
  unbounded request or an event sent it as written.
- `Spec::limit` stamps the shorter of the calling execution's remaining time and the call's bound;
  it no longer reads a caller's `deadline-ms`.
- A gRPC call whose `grpc-timeout` is off the grammar is answered INTERNAL, message "the call's
  `grpc-timeout` is "abcS", which is not at most eight ASCII digits followed by one of `H`, `M`, `S`,
  `m`, `u` and `n`" (quoted as above), before admission: no execution, no pre-dispatch stage, no
  span, no handler. Where it ran with no deadline and logged at `debug`. The health and reflection
  services, reached through the same `Dispatcher::call`, are refused the same way.
- `RpcError::reason` can answer `"reserved_header"`, the client's.

## Decisions

Line numbers are those of `57f0e727` unless a line says otherwise.

### 1. The forwarding premise: nothing copies an incoming call's headers into an outgoing one

- **The RPC client** builds a call's headers from `CallHeaders::new()` and the caller's `.header`
  calls alone (`crates/ulo-rpc/src/client.rs:447`). The only thing it takes from an execution is
  the deadline and the cancellation, through `within` on `Call`, `Emit` and `StreamCall`
  (`client.rs:822`, `:867`, `:964`), read in `Spec::limit` (`client.rs:468`). No ambient execution
  exists (`client.rs:161-162`).
- **The dispatcher** seeds an incoming call's `CallHeaders` into that call's own execution as an
  input (`crates/ulo-rpc/src/dispatch.rs:230`, `:282`) and the call's context (`dispatch.rs:255`).
  Nothing reads them back to build a call; a handler can copy them into one by hand, which is its
  own code.
- **The execution** (`crates/ulo/src/execution/`) holds a deadline, a cancellation and inputs; no
  core path names `CallHeaders` or `deadline-ms` beyond a doc line (`execution/mod.rs:83`).
- **The gRPC hub**: `ulo_grpc::outgoing` sets a tonic request's timeout from an execution's
  remaining deadline and copies no metadata (`crates/ulo-grpc/src/client.rs:108`). The server
  seeds `GrpcMetadata` into the call's execution (`crates/ulo-grpc/src/dispatch.rs:149-150`) and
  forwards none.
- **The HTTP hub**: `Embedded::forward` (`crates/ulo-http/src/embed.rs:423`) copies a host's
  request values into the app's request extensions, within one HTTP request. No HTTP path builds
  an RPC call.
- **The search**: every non-test use of `CallHeaders` and `deadline-ms` under `crates/`, every
  `RpcClient` call site, and the words `forward` and `propagat`. `traceparent`, listed beside
  `deadline-ms` as reserved, appears only in `CallHeaders`'s doc (F400). `examples/` and
  `integration-tests/` are outside the workspace's members.

So the header is reserved, and F395 builds.

### 2. F395: `reserved_header`, and why it wins over `bad_deadline`

- **The refusal.** `Spec::reserved` (`client.rs:499` on the batch's tree) refuses a call whose
  headers carry `deadline-ms`, any value, `BadRequest`, reason `reserved_header`, the first such
  value quoted by `dispatch::quote`, the function F394's `bad_deadline` now uses too, so both quote
  identically. `Call`, `Emit` and `StreamCall` each call it first in `into_future`, which covers
  `request`, `send_stream`, `emit`, `stream` and `duplex`.
- **The reason that wins.** An unreadable value reads `reserved_header`, not `bad_deadline`. The
  client refuses every caller-set value, so whether one parses decides nothing: the mistake is
  setting the header at all, and the remedy is the same for every value, `.timeout`, `.deadline`
  or `.within`. `bad_deadline` stays the server's, for a frame that reaches it below any ulo
  client. The alternative was refusing an unreadable value `bad_deadline` and a readable one
  `reserved_header`, two reasons for one mistake with one remedy.
- **Before the zero checks.** A call carrying both a caller's `deadline-ms` and a zero bound is
  refused `reserved_header`. Both refusals are permanent and before any I/O; the header check is
  one line common to the three shapes, where the zero checks are each shape's own.
- **An event.** `Emit` is refused too. An event's publish bound is never sent (`Whole::Publish`),
  so a caller's `deadline-ms` on an event was the one way to give an event's handler a deadline;
  that way is now closed. See S1.
- **What `limit` loses.** The caller's value took part in the stamp's minimum and the stamp
  replaced it (`client.rs:478-481`); both are gone, the header having been refused before `limit`
  runs.
- **Tests that set the header.** Three did: the conformance scenario `deadline_ms_fires_deadline`
  and two TCP tests in `deadline_answers.rs`, each wanting the server's deadline shorter than the
  client's wait so the server's answer arrives first. A client deadline cannot be shorter than the
  client's own wait, so each now writes the header below the client, through `Stamped`, a client
  link replacing `deadline-ms` on every opening frame, the client waiting ten seconds or its
  module's five. It is public in `ulo-rpc-conformance` so the TCP crate's tests use the same one.
  `deadline_ms_fires_deadline` now also requires the answer within five seconds, which tells the
  server's `timeout` from the client's own at ten.

### 3. F396: the grammar, the two probes, and the refusal

- **The specification.** `PROTOCOL-HTTP2.md` in grpc/grpc at commit `cf61c7d6` (2025-04-17), lines
  31-39 (`probes/spec/PROTOCOL-HTTP2.md`, its SHA-256 in `probes/spec/commit.txt`):

  ```text
  * **Timeout** → "grpc-timeout" TimeoutValue TimeoutUnit
  * **TimeoutValue** → {_positive integer as ASCII string of at most 8 digits_}
  * **TimeoutUnit** → Hour / Minute / Second / Millisecond / Microsecond / Nanosecond
    * **Hour** → "H"
    * **Minute** → "M"
    * **Second** → "S"
    * **Millisecond** → "m"
    * **Microsecond** → "u"
    * **Nanosecond** → "n"
  ```

  The designer's reading holds, with one word more: the value is a *positive* integer, so `0` is
  off the grammar as written (F399). Line 62: "If **Timeout** is omitted a server should assume an
  infinite timeout." The document says nothing of a value off the grammar.
- **How the probes sent each form.** A test binary copied into `ulo-codegen-tests` for the run and
  removed after (`probes/probe_grpc_timeout.rs`): the generated `ClockClient` over a plain hyper
  HTTP/2 client, no tonic `Channel`, each value inserted as raw `grpc-timeout` metadata, which
  tonic's client writes as given. ulo's handler recorded the header as `GrpcMetadata` saw it and
  the deadline left; grpc-go's (`probes/grpcgo/main.go`, an `UnknownServiceHandler` answering every
  path) answered the milliseconds left plus one, `0` for no deadline, and logged each run.
- **What tonic does.** Nothing: `ulo_grpc::Server` serves on `ulo-server-hyper` and does not use
  tonic's `transport::Server` (`crates/ulo-grpc/src/server.rs:2`); tonic contributes the codec
  layer alone. The header reaches `Dispatcher::call` as hyper parsed it, and `GrpcMetadata` carries
  it to the handler unchanged.

  | `grpc-timeout` | ulo at `57f0e727` | grpc-go 1.84.0 | ulo after the batch |
  | --- | --- | --- | --- |
  | `18446744073709551616S` (overflowing) | OK, handler ran, no deadline | INTERNAL, "malformed grpc-timeout: transport: timeout string is too long", handler not run | INTERNAL, handler not run |
  | `abcS` (not numeric) | the same as above | INTERNAL, "... strconv.ParseUint: parsing \"abc\": invalid syntax" | INTERNAL |
  | `10` (no unit) | the same | INTERNAL, "... timeout unit is not recognized" | INTERNAL |
  | `10x` (unknown unit) | the same | INTERNAL, "... timeout unit is not recognized" | INTERNAL |
  | `123456789S` (nine digits) | the same | INTERNAL, "... timeout string is too long" | INTERNAL |
  | the empty value | the same | INTERNAL, "... timeout string is too short" | INTERNAL |
  | `-1S` | the same | INTERNAL, "... parsing \"-1\": invalid syntax" | INTERNAL |
  | `10s` (lowercase unit) | the same | INTERNAL, "... timeout unit is not recognized" | INTERNAL |
  | `0S` | OK, handler ran, 0 ns left | DEADLINE_EXCEEDED, "context deadline exceeded", handler not run | unchanged (F399) |
  | `99999999H` | OK, the deadline set | OK, the deadline set | unchanged |
  | `5S` | OK, 5000 ms left | OK, 5000 ms left | unchanged |

  `probes/grpc-timeout-both-before.txt` holds both columns before the fix,
  `probes/grpc-go-server-log.txt` the container's log, and `probes/grpc-timeout-ulo-after.txt`
  ulo after it.
- **The code.** grpc-go refuses every malformed form with one code, INTERNAL, so interoperability
  decides it, as the response directs. The message quotes up to 64 characters of the value, the
  bytes read lossily when the value is not UTF-8.
- **Where it sits.** First in `Dispatcher::call` (`crates/ulo-grpc/src/dispatch.rs:141` on the
  batch's tree), before the admission check, `Execution::open`, the inputs, the span and both
  pre-dispatch stages. Before admission for RPC's reason: the refusal is permanent, and a caller
  answered UNAVAILABLE first would retry a call the server will never run. Before routing because
  grpc-go reads headers before it routes. No layer goes in front of tonic, which never sees the
  header first, so no S-item for one.
- **Zero** is still read as a deadline already passed, as before the batch and as RPC's S1 settled
  for `deadline-ms`. grpc-go differs, filed as F399, and the spec's "positive" supports refusing it;
  the response did not rule on gRPC's zero. See S2.

### 4. S4: what "rejected" means on each broker that settles

- **AMQP.** `Ack::reject` is `basic.reject` with `requeue: false`
  (`crates/ulo-rpc-rabbitmq/src/link.rs:411-419`), lapin 4.12.0's `Acker::reject` sending
  `basic.reject`. A rejected delivery leaves the queue, routed to a dead-letter exchange where one
  is configured.
- **Kafka.** `Ack::new`'s settlement ignores whether it accepts and stores the record's offset
  either way (`crates/ulo-rpc-kafka/src/link.rs:669-675`) through rdkafka 0.39's `store_offset`,
  which calls librdkafka's `rd_kafka_offset_store`, documented to store `offset + 1` for the next
  commit (`rdkafka-sys-4.10.0+2.12.1`'s `rdkafka.h:4041`). The consumer sets
  `enable.auto.offset.store` false and `enable.auto.commit` true (`link.rs:250-251`), and commits
  the stored offsets again at the drain and at close (`link.rs:355`, `:378`). So the committed
  offset passes a refused record and a later instance of the group starts after it.
- **The others.** NATS, MQTT and Redis deliver with `Ack::none()`, nothing to settle; TCP and UDP
  have no broker.
- **Nothing to fix.** Neither settlement can redeliver a refused event. The dispatcher rejects a
  refused event (`crates/ulo-rpc/src/dispatch.rs:203`), as it rejects an event nothing handles, a
  failed handler's and an expired one's (`:275`, `:412`, `:417`, `:419`). An event over the
  in-flight limit is left unsettled on purpose, for redelivery (`:216`), and is not a refusal of
  this kind.
- **Why at-least-once remains.** Kafka's commit runs on the auto-commit interval and at the drain;
  a server killed between a refused record's store and the commit leaves it uncommitted, and the
  next instance reads it and refuses it once more. That is the at-least-once the link declares, not
  a loop: the second refusal's store is committed like the first's.

## The tests

- **F395, two unit tests in `client.rs`**, over `Refusing`, the link counting its connects and the
  frames it is asked to send, on tokio's paused clock:
  - `a_caller_s_deadline_ms_is_refused_before_the_call_connects`: each of `abc`,
    `18446744073709551615999`, `-5`, the empty value, `50` and `0`, set with `.header` on a request
    on a client stamping its timeout, a request on a client with none, a request with its own
    timeout, a streamed request, a stream with no deadline, a stream with a deadline, a duplex call
    and an event: each fails `BadRequest`, `reserved_header`, and an hour later the link has made
    no connect and sent nothing.
  - `the_reserved_header_refusal_quotes_the_value_cut_short`: `abc` quoted whole, a hundred nines
    quoted to 64 and marked cut short.
- **F396, ten tests in `crates/ulo-codegen-tests/tests/deadlines.rs`**, replacing
  `a_grpc_timeout_off_the_grammar_is_ignored`, which asserted the old behaviour, through the plain
  HTTP/2 client with raw metadata; `Quick` and `Drip` now record that they ran:
  - one per form, `an_overflowing_grpc_timeout_is_refused_before_the_handler_runs`
    (`18446744073709551616S`), `a_grpc_timeout_that_is_not_a_number_is_refused_before_the_handler_runs`
    (`abcS`), `a_grpc_timeout_without_a_unit_is_refused_before_the_handler_runs` (`10`),
    `a_grpc_timeout_with_an_unknown_unit_is_refused_before_the_handler_runs` (`10x` and `10s`),
    `a_nine_digit_grpc_timeout_is_refused_before_the_handler_runs` (`123456789S`),
    `an_empty_grpc_timeout_is_refused_before_the_handler_runs` and
    `a_negative_grpc_timeout_is_refused_before_the_handler_runs` (`-1S`): a unary call and a
    server-streaming call each answered INTERNAL quoting the value, then a call without the header
    answered, and the only handler run that last one's;
  - `the_grpc_timeout_refusal_quotes_the_value_cut_short`: a hundred nines and `S`, quoted to 64;
  - `a_grpc_timeout_on_the_grammar_still_sets_the_deadline`: `5S` answered with four to five
    seconds left, `99999999H` with the most a `u32` holds, and `50m` ending a stalled call
    DEADLINE_EXCEEDED within five seconds, its handler dropped `Deadline`;
  - `a_zero_grpc_timeout_is_one_already_passed`: `0S` to the stalling handler answered
    DEADLINE_EXCEEDED, the handler dropped `Deadline`. It pins the present behaviour, F399's
    subject.
- **S4, the conformance scenario `a_refused_event_is_never_redelivered`** on every RPC link
  (`events::refused_event_is_not_redelivered`), the server over batch 31's `Tapped`, which records
  the mark of every `evt` it delivers: an event marked `refused`, its `deadline-ms` written `abc`
  by a `Stamped` client link, reaches the server, and two settle budgets later the server's link
  has delivered `["refused"]` and the event handler has not run. On a link declaring
  `holds_unserved`, RabbitMQ and Kafka, the server then stops, a next instance starts in its place
  over a second `Tapped`, an event marked `after` is emitted once it answers, and a settle budget
  after `after` arrives the next instance has delivered `["after"]`. No link declares it not
  applicable; on the five without `holds_unserved` the first half runs alone.

### Before and after

Each break went through `batch32/scripts/brk.py`, batch 31's script unchanged: it replaces each
span, asserting it occurs once, runs the targets under bash, writes every touched file and
`Cargo.lock` back byte for byte, and compares a SHA-256 over `git diff HEAD` of `crates`,
`Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every untracked file
under `crates` and `scripts`. Specs in `batch32/specs/`, output in `batch32/broken/` with
`summary.txt` and the container list before and after each. Every restore reported `ok`, on the
tree hashed `e926e76f..` (`verify/tree-hash-before-breaks.txt`). The unit target is the two F395
tests; the gRPC target is `ulo-codegen-tests`'s `deadlines` test.

| Break | Target | Result |
| --- | --- | --- |
| c01. F395's code: a caller's `deadline-ms` not refused | unit | both failed: the first request carrying `abc` reached the link and was answered by its server, `(BadRequest, None)` where `(BadRequest, Some("reserved_header"))` was due |
| c02. only an unreadable `deadline-ms` refused | unit | `a_caller_s_deadline_ms_is_refused_before_the_call_connects` failed at `50`, the request answered by the link's server; the quoting test passed |
| c03. an event's `deadline-ms` not refused | unit | failed, "an event carrying `deadline-ms: "abc"`: ()", the event published |
| c04. a stream's `deadline-ms` not refused | unit | failed, "a stream with no deadline carrying `deadline-ms: "abc"`: ()", the stream opened |
| c05. the value quoted to 128 characters | unit; `bad_deadline.rs` | the client's quoting test failed, the hundred nines quoted whole; the server's `the_refusal_quotes_the_value_cut_short` failed the same way, the two sharing `quote` |
| g01. F396's code: an off-grammar `grpc-timeout` as none | gRPC | the seven form tests and the quoting test failed, each call answered, "expected the call to fail, got Tick { n: 0 }"; the other eleven passed |
| g02. refused INVALID_ARGUMENT | gRPC | the same eight failed, `InvalidArgument` where `Internal` was due |
| g03. the value quoted whole | gRPC | the quoting test failed, the hundred nines quoted whole |
| g04. zero refused as off the grammar | gRPC | `a_zero_grpc_timeout_is_one_already_passed` failed, answered INTERNAL where DEADLINE_EXCEEDED was due |
| g05. an on-grammar `grpc-timeout` ignored | gRPC | eleven failed, the on-grammar test, the zero test and the nine earlier deadline tests among them; the eight refusal tests passed |
| k01. RabbitMQ requeues a rejected delivery (`requeue: true`) | RabbitMQ, the scenario | failed, the server's link having delivered `refused` 7,704 times in two settle budgets |
| k02. Kafka stores no offset for a rejected record | Kafka, the scenario | failed in its second half, the next instance having delivered `["refused", "after"]` where `["after"]` was due |
| s01. `Stamped` leaves a `req` as the client sent it | TCP, `deadline_ms_fires_deadline`; `deadline_answers.rs` | the scenario failed, "the call was answered after 10.002332417s, at the client's timeout rather than the server's"; both TCP tests failed, the single reply `Err((Timeout, "the call timed out"))` where `Ok("claimed")` was due and the stream's first frame not within five seconds |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `57f0e727`.

- §5.2's grammar paragraph (line 946): `deadline-ms` is reserved to the client, which writes it from
  the call's bound and refuses a call whose caller set it, `BadRequest`, `reason:
  "reserved_header"`, at the await before connecting, on every shape, an event's included; "and a
  `deadline-ms` the caller set" leaves the stamp's minimum. `traceparent`'s place in the same
  sentence waits on F400.
- §5.2's scenario list (line 966): `a_refused_event_is_never_redelivered`, its second half where
  the link declares `holds_unserved`; `deadline_ms_fires_deadline` writes its value through a
  client link below the client.
- §5.4's client paragraph (line 1096): the reserved header and its refusal beside the zero bounds'.
- §6.1's deadlines bullet (line 1169): "a value off that grammar is logged at `debug` and ignored,
  and the call runs without a deadline" goes; such a call is answered INTERNAL, the value quoted to
  64 characters, before admission and before any execution opens, grpc-go's code for the same
  value; a zero is a deadline already passed (F399).
- §12's refusals table (after line 1465, beside batch 31's zero-bound row): a caller's
  `deadline-ms` on any call, runtime before any I/O, `RpcError` of kind `BadRequest`, `reason:
  "reserved_header"`; and a `grpc-timeout` off the grammar, runtime, INTERNAL, no execution.
- Decision 122 (line 1632) and a decision after it: the client refuses every caller-set
  `deadline-ms` as `reserved_header`, which wins over `bad_deadline` because the mistake is setting
  the header, not its value; the gRPC server refuses an off-grammar `grpc-timeout` INTERNAL for
  interoperability with grpc-go, probed at 1.84.0.
- §5.3's `Ack::new` line (line 1008) and its table's RabbitMQ and Kafka rows (line 1067 on): a rejection is
  `basic.reject` with `requeue: false` on AMQP and the offset stored past the record on Kafka, so a
  refused event is handed over once.

## Needs sign-off

Numbered from S1 for this batch.

### S1. An event's `deadline-ms` is refused like a call's, so no event carries a deadline

Decision 2. `Emit` refuses a caller's `deadline-ms` as the call shapes do. An event's publish bound
is never sent, so the header set by hand was the only way an event's handler ran under a deadline,
and the server honoured it; that way is closed. The alternatives are exempting `Emit`, so the header
keeps meaning "the handler's deadline" on events alone, or an `Emit::deadline(Duration)` that sends
one, which would be a new bound on the client.

### S2. A zero `grpc-timeout` is still read as a deadline already passed

Decision 3. `0S` sets a deadline of now and the pipeline is polled once, as RPC's zero is under the
fifty-seventh response's S1. grpc-go answers DEADLINE_EXCEEDED without running the handler, and the
specification's `TimeoutValue` is a positive integer. Filed as F399 and not changed, the response
having ruled on RPC's zero and not gRPC's. The alternatives are grpc-go's DEADLINE_EXCEEDED before
the pipeline, which departs from RPC's S1, and INTERNAL as off the grammar.

### S3. The off-grammar refusal is written without an execution, so no error handler and no span sees it

Decision 3. As RPC's `bad_deadline` (batch 31's S2) and gRPC's own capacity refusal: the call never
opens an execution, so no guard, error handler or call span runs, and the server logs nothing
(F398 covers that for RPC). The alternative is opening an execution to offer it to the global error
handlers, which a `#[catch]` could then reshape.

### S4. `Stamped` is public in `ulo-rpc-conformance`

Decision 2. A client link writing a fixed `deadline-ms` is the only way left to give a server a
deadline shorter than the client's wait, which three tests need, two of them in `ulo-rpc-tcp`'s own
`tests/`. The crate is `publish = false`, so the type reaches no user. The alternative is a copy in
each test file.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch32/`: `suites/` for the RPC
suites, with `summary.txt` and the container lists before and after each, `verify/` for the checks,
the workspace test, docs and clippy, `broken/` and `specs/` for the breaks, `probes/` for the
specification, the two probes, their sources and the grpc-go container's log, `scripts/`. Every run
below ran on the tree hashed `e926e76f..`, the hash read before the breaks, before the verification
and after it (`verify/tree-hash-before-breaks.txt`, `verify/final-tree-hash.txt`,
`verify/final-tree-hash-end.txt`).

- **The RPC suites, once each,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 43 passed and 3 ignored each (2.02 s, 2.02 s,
  2.07 s); the smol TCP link's three 43 and 3 each (2.02 s, 2.02 s, 2.04 s); UDP 40 and 6 (1.32 s).
  With `--features integration --test conformance` and the OpenSSL flags: NATS 45 and 1 (4.57 s);
  Redis 44 and 3 (6.61 s); MQTT 50 and 1 (6.88 s); Kafka 46 and 1 (54.00 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 45 and 1 (23.52 s). Each count is batch 31's and
  `a_refused_event_is_never_redelivered`, which passed on all twelve, as did the rewritten
  `deadline_ms_fires_deadline`.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `deadlines` 19, `builtins` 6,
  `clients` 5, `compile_fail` 1, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6, all passed;
  `ulo-grpc` (`verify/ulo-grpc-tests.txt`) has no tests of its own, 0 unit and its 5 doctests
  ignored, its behaviour being tested through `ulo-codegen-tests`.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 18 unit, `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 1 and 5 ignored, then the
  `compile_fail` one.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 456 files scanned, 2 listed, 0 findings, exit 0
  (`verify/unsafe-check.txt`).
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, 0 failures, 0 errors (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, both with `CFLAGS=-I/opt/homebrew/opt/openssl/include
  LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: exit 0, `ulo`'s 17 known warnings in `crates/ulo/src`
  and no location elsewhere (`verify/check-all-features.txt`, `verify/check-defaults.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in `target/msrv-1.88`: exit 0, the same 17 and no other
  (`verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags:
  exit 0, 1,197 passed, 0 failed, 141 ignored, over 198 `test result` lines
  (`verify/workspace-test.txt`). Batch 31's run had 1,179 over the same 197 binaries and 198
  lines; its log counted 200 by including two test names containing the phrase. The 18 more: the
  two F395 unit tests; the ten F396 tests less the one they replace; and the new scenario in each
  of the seven conformance binaries that build without `integration`, TCP's three, the smol TCP
  link's three and UDP's.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`,
  `ulo-grpc` and `ulo-rpc-conformance`, the last at its defaults and with `tokio,smol`: exit 0 each
  (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-conformance -p ulo-grpc -p ulo-codegen-tests -p
  ulo-rpc-tcp --all-targets --all-features --no-deps --locked`, with the OpenSSL flags: exit 0, 11
  warning locations outside `crates/ulo/src` (`verify/clippy.txt`), none on a line the batch
  changed: batch 31's nine in `ulo-rpc` and `ulo-rpc-conformance`, and in `ulo-grpc`
  `result_large_err` at `extract.rs:105` and `collapsible_if` at `dispatch.rs:819`.
- **The grpc-go probe's container:** `golang:1.25` (`sha256:699337d6..`) pulled for the batch, run as
  `ulo-batch32-grpcgo` with port 55051 published on `127.0.0.1` and the source mounted read-only,
  building grpc-go 1.84.0 and protobuf-go 1.36.12; removed with `docker rm -f`, and the image with
  `docker rmi`. The image list after matches the one before as a set
  (`verify/docker-images-start.txt`, `verify/docker-images-after-golang.txt`), and no volume was
  created. After removing them, `docker builder prune -f` was run as well and removed 2.524 GB of
  the host's build cache, which the batch had not created: `docker run` builds nothing. That cache
  is regenerated by the next image build that needs it; it was not the batch's to remove.
- **Containers:** at the batch's start twelve containers, the user's four of `backend-next`,
  postgres:18, seaweedfs, redis:7 and mailpit, among them, every one exited
  (`verify/docker-start.txt`). Every suite's and every break's list after it matches its list
  before, and the list at the end matches the start's (`suites/*.docker-{before,after}.txt`,
  `broken/*.docker-{before,after}.txt`, `verify/docker-end.txt`). No permission check refused an
  action.
- **Disk:** 199 GB free at the start, 191 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`).
