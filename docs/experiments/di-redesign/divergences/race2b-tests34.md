# Divergences: race 2b, tests batch 34, a passed deadline refused before anything is sent (F407), an event carrying none of its emitter's deadline (F408), and a call from a cancelled execution never sent (F409)

The fifty-ninth response, signed off 2026-10-17, answers `race2b-tests33.md`. Both of its premises
were probed before anything was built. On RPC a call made `.within` an execution whose deadline has
passed already fails `Timeout` before it connects, a request, a stream and an event alike, and the
server's link receives nothing; a conformance scenario now pins it. On gRPC `ulo_grpc::outgoing`
wrote such a deadline as `1n`, and all sixteen probe calls, eight through a plain HTTP/2 client and
eight through a tonic `Channel`, entered the handler, fifteen answered `Ok` and one `CANCELLED`
(F407); `outgoing` now answers DEADLINE_EXCEEDED for it, so the request is never built. The `emit`
premise held: the server counts `deadline-ms` from the delivery reaching its dispatcher, and an event
emitted `.within` an execution with 50 ms left (300 ms on Kafka) carried `deadline-ms: 49` (`299`),
its handler dropped `Deadline` on all four links probed; on RabbitMQ and Kafka, emitted while no
instance consumed, the handler began 92 ms and 647 ms after the emitter's deadline with the whole 49
and 299 ms still to run (F408). An event now carries no `deadline-ms`. Building the response's second
`emit` test turned up F409: an execution cancelled before a call is awaited raced the call's first
poll in an unbiased `select!`, so a request, a stream or an event could be sent while its caller was
told `Unavailable`; on RabbitMQ and Kafka every such call was reported cancelled and up to half were
delivered. The client now answers the cancellation before it polls the call. Reading the client for
the passed-deadline check found F410, filed and not built: the stamp is computed before the client
connects.

Files changed: for F407, `crates/ulo-grpc/src/client.rs` and `crates/ulo-codegen-tests/tests/clients.rs`;
for F408 and F409, `crates/ulo-rpc/src/client.rs`; for the scenarios, `crates/ulo-rpc-conformance/src/lib.rs`,
`src/cases/app.rs`, `src/cases/deadlines.rs` and `src/cases/events.rs`; in the workspace's
`FRAMEWORK_GAPS.md`, F407 to F410 new, F407 to F409 with their fix notes, their commit written
`<commit>`. The batch started from `f4860ba2`, and no commit landed during it. Versions are unchanged
at `0.1.0-alpha.1`, and `Cargo.lock` is unchanged.

## The signatures

```rust
// crates/ulo-grpc/src/client.rs, public: was `-> tonic::Request<T>`
pub fn outgoing<T>(exec: &ExecutionRef, request: tonic::Request<T>) -> Result<tonic::Request<T>, tonic::Status>;

// crates/ulo-rpc/src/client.rs, private
fn cancellation(reason: Option<CancelReason>) -> RpcError;

// crates/ulo-rpc-conformance, crate-private
pub(crate) const SLOW_EVENT: &str = "conformance.slow_event";
pub(crate) struct SlowRun { pub(crate) began: Instant, pub(crate) left: Option<Duration>, pub(crate) ended: Option<SlowEnd> }
pub(crate) enum SlowEnd { Completed, Dropped(Option<CancelReason>) }
pub(crate) type Stamp = (String, Option<String>);
impl Probe { pub(crate) fn slow_runs(&self) -> Vec<SlowRun>; }
impl Tap { pub(crate) fn stamps(&self) -> Vec<Stamp>; }
impl Client {
    pub(crate) async fn execute<R>(&self, opts: ExecOptions, f: impl AsyncFnOnce(&Execution) -> R) -> R;
    pub(crate) fn open(&self, opts: ExecOptions) -> Execution;
}
```

`ulo_grpc::outgoing` is the one public signature that changes. What changes in meaning without a
signature:

- `outgoing` answers `Err(tonic::Status)`, code DEADLINE_EXCEEDED, message "the calling execution's
  deadline has passed", for an execution whose deadline is at or before tokio's now, where it
  answered the request with `grpc-timeout: 1n`. A caller writes `outgoing(&exec, request)?`, and the
  status travels as any status a handler returns, rendered as it stands.
- An `Emit` made `.within(&exec)` carries no `deadline-ms`, where it carried `exec`'s remaining
  time. Its publish is still bounded by the shorter of the client's timeout and `exec`'s remaining
  time, an `exec` whose deadline has passed still fails it `Timeout` before it connects, and `exec`'s
  cancellation still fails it up to the send.
- A request, a streamed request, a stream, a duplex call or an event made `.within` an execution
  already cancelled fails when awaited, before it connects, `Unavailable` "the calling execution was
  cancelled", or `Timeout` "the calling execution's deadline passed" for a cancellation of reason
  `Deadline`, with nothing sent. Before the batch the call raced the cancellation, and either could
  win. A stream's next read once its execution is cancelled ends the stream with that error, where
  a frame already queued could win the race before.
- The RPC conformance suite has five scenarios more, so every link's count grows by five.

## Decisions

Line numbers are those of `f4860ba2` unless a line says otherwise.

### 1. The passed-deadline probe, RPC: already refused, so pinned

- **What it read.** `Spec::limit` (`crates/ulo-rpc/src/client.rs:468`) reads the calling
  execution's deadline against the client runtime's now and returns `Timeout`, "the calling
  execution's deadline has passed", for a remainder of zero (`client.rs:482`), before `bounded`
  polls anything, on every shape: `Call`, `StreamCall` and `Emit` each call it first in
  `into_future`.
- **What it showed.** The new scenario, run on the code before any fix, passed on TCP, NATS,
  RabbitMQ and Kafka (`before/before-rpc-*.txt`): eight requests, eight streams and eight events from
  each of two executions, every one `Timeout`, and the server's link delivered only the request made
  afterwards outside both.
- **Two executions.** A standalone execution, `App::execute` with a deadline of now, is cancelled
  `Deadline` by the app before its closure runs (`crates/ulo/src/app/shared.rs:198`), and after
  F409's fix that cancellation alone refuses the calls. So the scenario adds an execution opened as
  a transport opens one, `Execution::open`, which nothing cancels at its deadline, and asserts it is
  still not cancelled, so `limit`'s check alone refuses its calls. With that check removed the
  opened execution's twenty-four calls all reached the server (`broken/b04-*.txt`).

### 2. The passed-deadline probe, gRPC: sent, so fixed (F407)

- **What it read.** `outgoing` (`crates/ulo-grpc/src/client.rs:108`) wrote a passed deadline as
  `1n` through `timed`'s floor, and the server opens a `1n` call with a nanosecond left and polls the
  pipeline once (batch 33's decision 1).
- **What it showed.** `probes/grpc-passed-deadline.txt`: eight `Quick` calls through a plain HTTP/2
  client, each carrying `grpc-timeout: 1n`, all answered `Ok(1)`, and eight through a tonic
  `Channel`, seven `Ok(1)` and one `CANCELLED`; the server counted sixteen entries into `Quick`. A
  `Channel` enforces `grpc-timeout` itself, which loses the race to an answer that does not await.
- **Where it fails.** `outgoing` is the one place ulo touches an outgoing gRPC request; the call
  itself is tonic's generated client over a `Channel` whose type ulo does not wrap, so no layer of
  ulo's sees the call to refuse it. Failing in `outgoing` stops the call before the request exists,
  so nothing connects and nothing is sent.
- **The error.** `tonic::Status`, code DEADLINE_EXCEEDED, the code the callee would have answered
  and the type the tonic call itself fails with, so one `?` covers both in a handler or anywhere
  else; the message is the RPC client's for the same refusal. A ulo `Classify` error was the
  alternative, and it would have made the caller handle two error types for one call.
- **`timed`'s floor.** `timed` still holds a remainder to at least one nanosecond, and batch 33's
  sweep still asserts a zero is written `1n`; `outgoing` no longer hands it a zero, so the floor
  guards only the encoder. Its test's message reads "a zero remainder", where it read "a deadline
  already passed".

### 3. The `emit` probe: the server counts from delivery, so fixed (F408)

- **What it read.** The dispatcher opens an event's execution with a deadline of
  `shared.timer.now() + after` (`crates/ulo-rpc/src/dispatch.rs:221`), `now` being when the delivery
  reaches `start`, so `deadline-ms` is time remaining from receipt. `Spec::limit` left out only the
  client's own timeout for `Whole::Publish` (`client.rs:478`, `:490`), so an event made
  `.within(&exec)` stamped `exec`'s remaining time.
- **What it showed.** The first `emit` test on the code before the fix (`before/before-rpc-*.txt`),
  50 ms left of the emitter's deadline, 300 ms on Kafka, scaled as a tenth of the broker's settle
  budget, and a handler running four times that: on TCP and NATS, which hold nothing, the handler
  began 0.3 to 0.5 ms after the emit with 48.9 ms left and was dropped `Deadline`; on RabbitMQ and
  Kafka, which declare `holds_unserved`, the event was emitted with no instance serving, and the next
  instance began the handler 142 ms and 947 ms after the emit, 92 and 647 ms past the emitter's
  deadline, with 48.9 and 298.9 ms left, and dropped it `Deadline`. The frame carried
  `deadline-ms: 49`, and `299` on Kafka. `before/rabbitmq-new-scenarios.txt`,
  `before/nats-new-scenarios.txt` and `before/kafka-emitter-deadline.txt` record the same from the
  probe's first runs.
- **The fix.** `limit` stamps nothing for `Whole::Publish` (`client.rs:492` on the batch's tree).
  The publish's own bound stays the shorter of the client's timeout and the execution's remaining
  time; see S3.
- **Docs.** `Emit::within` had no doc and now says what it forwards; `Whole::Publish`'s and
  `limit`'s docs say an event is given no deadline and why.

### 4. F409: the cancellation raced the call's first poll

- **How it surfaced.** The response's second test, an event emitted `.within` an execution
  cancelled before the emit is awaited, failed on the code before any fix: of eight such events the
  server's link delivered three or four on each link, three of them answered `Ok(())` on TCP and
  NATS. A temporary probe scenario then made eight requests and eight streams the same way on TCP
  (`probes/tcp-cancelled-request.txt`, from a scenario since removed): every request failed
  `Unavailable`, three streams opened, and the server's link delivered six requests and three
  streams.
- **Why.** `bounded` (`client.rs:786`) races the work against the execution's cancellation in
  `futures_util::select!` (`client.rs:809`), which picks at random among the arms ready at once.
  Cancelled before the call is awaited, both arms are ready at the first poll, and the work's arm,
  polled first on an open connection, hands the frame to the link within that poll. `limit` refuses
  a passed deadline first, so only another reason reached the race.
- **The fix.** `bounded` answers the cancellation before it polls the work (`client.rs:798` on the
  batch's tree), through `cancellation`, which both that check and the `select!` arm now call, so
  the two answer alike. It covers every shape, since every shape goes through `bounded`, and each
  read of a stream's next frame; see S2.
- **Tests.** The response's emit test, and a scenario for requests and streams,
  `a_call_in_a_cancelled_execution_is_refused_before_the_server_sees_it`.

### 5. F410: filed, not built

Reading the client for decision 1 found that `limit` computes the stamp when the call is awaited and
`Spec::open` connects afterwards (`client.rs:524`), so a call on a new connection gives the server up
to the connect's length beyond the caller's wait; `outgoing` and a lazily connected tonic channel
have the same order. The response asked about a deadline already passed when the call is sent, which
the RPC client refuses at the await, and moving the stamp after the connect is a change to the
client's flow it did not ask for, so it is filed for the designer and not built.

### 6. How the scenarios are built

- **Server-side first.** Each new scenario asserts what the server's link delivered before it
  asserts what the client was told, and its failure message carries every call's outcome, so a run
  against a break reports the server's side even when the client's answers also differ.
- **`Tapped` records `deadline-ms`.** The tap now keeps each delivered mark with its frame's
  `deadline-ms`, which `Tap::stamps` answers, so the first `emit` scenario asserts the frame carried
  none; `Tap::marks` answers what it answered before.
- **A handler that reports how it ended.** `conformance.slow_event` runs for the milliseconds it is
  given, on the app's clock, recording when it began, the time its execution's deadline left it then,
  and whether it completed or was dropped, with the cancel reason, through a guard's `Drop`.
- **Holding links.** On a link declaring `holds_unserved` the first `emit` scenario stops the
  fixture's server, emits, waits twice the emitter's remaining time and starts the next instance over
  a new `Tapped`, as `refused_event_is_not_redelivered` and the drain scenario do; elsewhere it emits
  to the serving server.
- **Executions on the client.** `Client::execute` runs a closure in a standalone execution of the
  client's app, and `Client::open` opens one as a transport does; `client_over` reads the client's
  root module for it.
- **gRPC.** The client tests' `Clock` counts entries into `Quick`, the handler answering without
  awaiting, and `counted` starts a server with the count. `Stall` was there only for the
  passed-deadline half of `outgoing_forwards_the_execution_s_remaining_deadline`, which moved to the
  new test, and it is removed. `forward` builds the request through `outgoing` in each test, and the
  before-run wraps that one line.

## The tests

- **The RPC passed deadline, one conformance scenario in `crates/ulo-rpc-conformance/src/cases/deadlines.rs`:**
  `a_call_past_its_execution_s_deadline_is_refused_before_the_server_sees_it`, decision 1.
- **F407, one test in `crates/ulo-codegen-tests/tests/clients.rs`:**
  `outgoing_refuses_a_passed_deadline_so_nothing_reaches_the_server`, a call through a plain HTTP/2
  client and one through a tonic `Channel` from an execution whose deadline has passed, each
  DEADLINE_EXCEEDED, and `Quick` entered once, by a call made afterwards outside the execution.
  `outgoing_forwards_the_execution_s_remaining_deadline` keeps its forwarded 2 s deadline.
- **F408, the response's three tests as conformance scenarios in `src/cases/events.rs`:**
  `an_event_carries_none_of_its_emitter_s_deadline` (the first), `an_event_emitted_in_a_cancelled_execution_is_not_sent`
  (the second, eight events, then one outside the execution, which alone reaches the server two
  settle budgets on), and `an_event_sent_before_its_emitter_is_cancelled_is_handled` (the third, the
  execution cancelled once the publish returns, the handler run to completion).
- **F409, one conformance scenario in `src/cases/deadlines.rs`:**
  `a_call_in_a_cancelled_execution_is_refused_before_the_server_sees_it`, eight requests and eight
  streams, each `Unavailable`, then a request outside the execution, delivered alone. The second
  `emit` scenario holds the same check for events.

### Before and after

Each fix was tested before it was made: the batch's tests against the clients as `f4860ba2` has
them, through `batch34/scripts/brk.py`, batch 33's script unchanged, with specs that put back
`f4860ba2`'s `crates/ulo-rpc/src/client.rs` and `crates/ulo-grpc/src/client.rs` whole and, for gRPC,
wrap `forward`'s call in `Ok(..)` for the old signature (`batch34/specs-before/`, output in
`batch34/before/`). The breaks then went through the same script, which replaces each span,
asserting it occurs once, runs the targets under bash, writes every touched file and `Cargo.lock`
back byte for byte, and compares a SHA-256 over `git diff HEAD` of `crates`, `Cargo.toml`,
`Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every untracked file under `crates`
and `scripts`. Specs in `batch34/specs/`, output in `batch34/broken/` with `summary.txt` and the
container list before and after each. Every restore, the two before-runs' and the four breaks',
reported `ok`, on the tree hashed `7b5c0616..` (`verify/tree-hash-before-breaks.txt`) for the breaks
and `4df6494a..` for the before-runs, which ran before the batch's last edits: the unused `Stall`
handler removed, `Tap`'s record type named for clippy, and three doc comments reworded.

| Run | Target | Result |
| --- | --- | --- |
| before F407: the gRPC client as it stood | `clients` | `outgoing_refuses_a_passed_deadline_so_nothing_reaches_the_server` failed, both calls answered `Ok(1)` and `Quick` entered three times; the other six passed (`before/before-grpc-0.txt`) |
| before F408 and F409: the RPC client as it stood, TCP | the five scenarios | three failed: the first `emit` scenario, its handler dropped `Deadline` with `deadline-ms: 49`; the cancelled emits, four of eight delivered; the cancelled calls, nine of sixteen delivered. The passed-deadline scenario and the third `emit` scenario passed (`before/before-rpc-0.txt`) |
| the same, NATS | the five scenarios | the same three failed, four events and twelve calls delivered; the same two passed (`before/before-rpc-1.txt`) |
| the same, RabbitMQ | the five scenarios | the same three failed: the handler begun 142 ms after the emit with 48.9 ms left; four events and six calls delivered, every one answered `Unavailable`; the same two passed (`before/before-rpc-2.txt`) |
| the same, Kafka | the five scenarios | the same three failed: the handler begun 947 ms after the emit with 298.9 ms left, `deadline-ms: 299`; three events and eight calls delivered, every one answered `Unavailable`; the same two passed (`before/before-rpc-3.txt`) |
| b01. `outgoing` without its passed-deadline check | `clients` | the new test failed, both calls answered `Ok(1)` and `Quick` entered three times; the other six passed |
| b02. `limit` stamping an event again | TCP and RabbitMQ, the first `emit` scenario | failed on both, the handler dropped `Deadline` with `deadline-ms: 49`, on RabbitMQ begun 137 ms after the emit |
| b03. `bounded` without its cancellation check | TCP, the cancelled scenarios and the passed-deadline one | both cancelled scenarios failed, calls and events delivered; the passed-deadline scenario passed, `limit` refusing it |
| b04. `limit` without its passed-deadline check | TCP, the passed-deadline scenario | failed: the opened execution's twenty-four calls reached the server, six requests answered and eight events published; the standalone execution's were refused by F409's check |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `f4860ba2`.

- §5.2's grammar paragraph (line 946): "an event carries the calling execution's remaining time
  alone, its timeout bounding the publish" becomes: an event carries no `deadline-ms`, `.within`
  forwarding the execution's cancellation up to the send and its remaining time bounding only the
  publish, since nobody waits on an event and the server counts the value from delivery (F408).
- §5.2's conformance paragraph (line 962): the waiting handlers the suite lists gain `slow_event`,
  and `Tapped` records each delivered mark's `deadline-ms`; the scenario count grows by five.
- §5.4's client paragraph (line 1096): `.within(&exec)` refuses an execution already cancelled
  before any I/O, as it refuses a passed deadline, so nothing is sent for it (F409); on an `Emit` it
  stamps nothing.
- §6.3's client paragraph (line 1190): `fw_grpc::outgoing(&exec, request)` answers
  `Result<tonic::Request<T>, tonic::Status>`, DEADLINE_EXCEEDED for a deadline already passed, so the
  call is never made; the "a passed deadline is sent as `1n`" clause goes (F407).
- Decision 106 (line 1619): an event is not stamped, the execution's remaining time included.
- Decision 126 (line 1639): `timed`'s one-nanosecond floor now guards the encoder alone, `outgoing`
  refusing a zero remainder before it.
- A decision after 126: a call is never sent from an execution whose deadline has passed or that is
  already cancelled, on RPC by `limit` and `bounded` and on gRPC by `outgoing`, F392's line; and an
  event borrows no budget, the fifty-ninth response's ruling, with the probe's numbers.

## Needs sign-off

Numbered from S1 for this batch.

### S1. `ulo_grpc::outgoing` answers a `Result`

Decision 2. Refusing the call before anything is sent needs a seam that runs before the request
reaches tonic, and `outgoing` is the only one ulo has; the generated client and its `Channel` are
tonic's. The return type changes from `tonic::Request<T>` to `Result<tonic::Request<T>, tonic::Status>`,
and every caller adds a `?`. The error is a `tonic::Status`, the type the call itself fails with, and
one `?` covers both. Keeping the signature and sending `1n` is the defect; a second function beside it would leave
the defective one in place.

### S2. F409's check covers every shape, and each read of a stream

Decision 4. The response named an `emit` from a cancelled execution; the defect is in `bounded`,
which every call shape goes through, and the probe showed requests and streams delivered while their
callers were told `Unavailable`. The check runs on every shape, and on each wait for a stream's next
frame, where a frame already queued no longer wins against a cancellation that has fired. Checking
on `Emit` alone was the alternative, and it would have left the same defect on the other four shapes.

### S3. An event's publish is still bounded by the execution's remaining time

Decision 3. The ruling says `.within` on `emit` forwards cancellation up to the send. A deadline
passing is the execution's cancellation with the reason `Deadline`, and the client still observes it
locally: the publish's wait is the shorter of the client's timeout and the execution's remaining
time, and an execution whose deadline has passed fails the emit `Timeout` before it connects, as the
passed-deadline scenario asserts. The alternative was to ignore the deadline on an `emit` and rely on
the execution's cancellation firing at it, which happens only where something arms it: the app's
`execute` does, and an execution opened with `Execution::open` and no transport enforcing it never
fires, so its emits would be sent whatever its deadline.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch34/`: `probes/` for the
probes, `before/` for each test run against the code before its fix and after it, `suites/` for the
RPC suites, with `summary.txt` and the container lists before and after each, `verify/` for the
checks, the workspace test, docs and clippy, `broken/` and `specs/` for the breaks, `specs-before/`
for the before-runs, `scripts/`. Every run below ran on the tree hashed `7b5c0616..`, the hash read
before the breaks, before the verification and after it (`verify/tree-hash-before-breaks.txt`,
`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`).

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 48 passed and 3 ignored each (2.32 s, 2.32 s,
  2.36 s); the smol TCP link's three 48 and 3 each (2.34 s, 2.34 s, 2.35 s); UDP 45 and 6 (1.34 s).
  With `--features integration --test conformance` and the OpenSSL flags: NATS 50 and 1 (4.81 s);
  Redis 49 and 3 (6.95 s); MQTT 55 and 1 (7.19 s); Kafka 51 and 1 (54.41 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 50 and 1 (26.09 s). Each count is batch 33's and five, the five new
  scenarios, none declared not applicable on any link.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `deadlines` 20, `builtins` 6,
  `clients` 7, `compile_fail` 1, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6, all passed;
  `ulo-grpc` (`verify/ulo-grpc-tests.txt`) 1 unit test and 6 doctests ignored, the sixth
  `outgoing`'s new `ignore` example.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 21 unit, `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 1 and 5 ignored, then the
  `compile_fail` one. **`ulo-rpc-conformance`'s** (`verify/rpc-conformance-tests.txt`): 2 unit, `harness`
  5, and 2 doctests ignored.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 456 files scanned, 2 listed, 0 findings, exit 0
  (`verify/unsafe-check.txt`).
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, 0 failures, 0 errors (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, both with `CFLAGS=-I/opt/homebrew/opt/openssl/include
  LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: exit 0, `ulo`'s 17 known warnings and no location
  outside `crates/ulo/src` (`verify/check-all-features.txt`, `verify/check-defaults.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in `target/msrv-1.88`: exit 0, the same 17 and no other
  (`verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags:
  exit 0, 1,239 passed, 0 failed, 142 ignored, over 198 lines starting `test result:`
  (`verify/workspace-test.txt`). Batch 33's run had 1,203 and 141 over 198. The 36 more: the five
  scenarios in each of the seven suites the workspace test runs, TCP's three, UDP's and smol's three,
  the broker suites being behind `integration`, and the gRPC test; the one more ignored is
  `outgoing`'s doc example.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`, `ulo-grpc`
  and `ulo-rpc-conformance` (`--all-features`): exit 0 each (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-grpc -p ulo-codegen-tests -p ulo-rpc-conformance
  --all-targets --all-features --no-deps --locked`, with the OpenSSL flags: exit 0, 11 warning
  locations outside `crates/ulo/src` (`verify/clippy.txt`): batch 33's ten, `client.rs:212` in
  `ulo-rpc` being batch 33's `type_complexity` at the same line, and `result_large_err` on
  `ulo-rpc-conformance`'s `bound` (`cases/app.rs:587`), a function the batch did not change, the
  crate entering this clippy run for the first time. The one the batch introduced, `type_complexity`
  on `Tap`, was fixed by naming `Stamp`.
- **Containers and images:** at the batch's start twelve containers, the user's four of
  `backend-next`, postgres:18, seaweedfs, redis:7 and mailpit, among them, every one exited
  (`verify/docker-start.txt`), and 25 images (`verify/docker-images-start.txt`). Every probe's,
  before-run's, break's and suite's container list after it matches its list before, the last of
  them, after the RabbitMQ suite, holding the start's twelve names. The batch's own containers were
  the suites' testcontainers, each removed by its scenario. After the verification run Docker
  stopped answering: `docker ps`, `docker images` and `docker version` each hung with no output and
  were stopped, so no container or image list was taken at the end. The batch pulled no image, ran
  no `docker ... prune` and started, stopped or removed nothing of the user's. No permission check
  refused an action.
- **Disk:** 190 GB free at the start, 187 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`).
