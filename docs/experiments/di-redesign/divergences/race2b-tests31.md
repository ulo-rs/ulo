# Divergences: race 2b, tests batch 31, a call's zero timeout and a stream's zero deadline refused before the server sees a frame (F392), every `BadRequest` attempted once, and a `deadline-ms` the server cannot read refused `bad_deadline` before any handler runs (F394)

The fifty-sixth response, signed off 2026-10-17, answers `race2b-tests30.md`. `Call::timeout(Duration::ZERO)`
and `StreamCall::deadline(Duration::ZERO)` fail when the call is awaited, first of all and before it
connects, `BadRequest` with the reason `zero_timeout` and a `ZeroTimeout` as the error's source, as a
stream's zero idle timeout does since batch 30; both methods still take a `Duration`, the move to
`Bound` being F393. Two conformance scenarios on every link hold the refusal, the server's own link
recording the opening frames it delivers. The client has no retry policy: no path in it repeats a
call, and a unit test shows a `BadRequest` from each of its three sources attempted once. A probe
with hand-written frames found that the server ran a call whose `deadline-ms` it could not read
with no deadline and said nothing, filed as F394: such a call is now refused before admission,
before any execution opens, a `req` or an `open` answered `err` of kind `bad_request` with the reason
`bad_deadline` and an `evt` rejected. A zero `deadline-ms` is readable, a deadline already passed,
and is not refused. Two findings are filed and left: the client replaces or passes through a
caller's own unreadable `deadline-ms` by whether it stamps one (F395), and the gRPC server runs a
call whose `grpc-timeout` is off the grammar with no deadline (F396).

Files changed: for F392 and the retry check, `crates/ulo-rpc/src/client.rs`,
`crates/ulo-rpc-conformance/src/cases/{app.rs, deadlines.rs}` and `crates/ulo-rpc-conformance/src/lib.rs`;
for F394, `crates/ulo-rpc/src/{dispatch.rs, transport.rs, held.rs}`, `client.rs`'s one read of the
caller's header, and a new `crates/ulo-rpc/tests/bad_deadline.rs`; in the workspace's
`FRAMEWORK_GAPS.md`, F394, F395 and F396 new and a fix note under F392, its commit written
`<commit>`. The batch started from `a242cffd`, and no commit landed during it. Versions are unchanged
at `0.1.0-alpha.1`, and `Cargo.lock` is unchanged.

## The signatures

No public signature changes. `Call::timeout(Duration)` and `StreamCall::deadline(Duration)` keep their
types (F393). `ZeroTimeout`'s public API is unchanged; its private fields add the zero as written and
what to write instead, so the client-wide and idle-timeout messages read as before.

```rust
// crates/ulo-rpc/src/transport.rs, crate-private
impl CallHeaders {
    pub(crate) fn deadline(&self) -> Result<Option<Duration>, &str>;  // was `-> Option<Duration>`; `Err` carries the unreadable value
}
```

What changes in meaning without a signature:

- A call awaited with `.timeout(Duration::ZERO)` or `.deadline(Duration::ZERO)` fails `BadRequest`,
  reason `"zero_timeout"`, its source a `ZeroTimeout` reading "`Call::timeout(Duration::ZERO)` on the
  tcp link would time out the call; leave `.timeout` unset for the client's timeout, or make the call
  on a client whose timeout is `Bound::Unbounded`" or "`StreamCall::deadline(Duration::ZERO)` on the
  tcp link would time out the stream; leave `.deadline` unset for no deadline". It connects nothing
  and sends nothing, where it timed out, was answered, or reached the server, by a race.
- A server given a `req` or an `open` whose `deadline-ms` does not parse as a `u64` after trimming
  answers `err` of kind `bad_request`, reason `"bad_deadline"`, message "the call's `deadline-ms` is
  "abc", which is not a whole number of milliseconds from 0 to 18446744073709551615" (the value quoted
  to 64 characters, ", cut short" after it past that), and acknowledges the delivery; an `evt` with
  one is rejected and logged at `warn`. No execution opens, and no guard, handler or error handler
  runs. Where it ran the call with no deadline.
- `RpcError::reason` can answer `"bad_deadline"`, the server's.

## Decisions

### 1. F392: where the zero is refused, and the message it carries

- **The point.** Each builder method answers `Self`, so the await is the first point a zero can
  fail, the response's S2 shape. `Call::into_future` tests `spec.timeout == Some(Duration::ZERO)`
  before anything else, before `Spec::limit`'s refusal of a passed execution deadline and before
  connecting, as batch 30 placed the idle timeout's zero. `StreamCall::into_future` tests the idle
  timeout's zero first, then the deadline's, both before `Spec::limit`. The one check in
  `Call::into_future` covers `request` and `send_stream`, which both answer a `Call`.
- **The message.** `ZeroTimeout`'s message for the two client-wide setters and the idle timeout
  ends "write `Bound::Unbounded` to turn the timeout off", which a `Duration` setter cannot take. It
  now carries the zero as the setter takes it and what to write instead: for `Call::timeout`, leave
  it unset or use a client whose timeout is `Bound::Unbounded`, since a call cannot turn the client's
  timeout off; for `StreamCall::deadline`, leave it unset. `ZeroTimeout::refusal` builds the
  `BadRequest` for all three per-call zeros. See S3.
- **What the race was.** With the check removed, the first zero call on TCP was answered,
  `Ok(Sum { sum: 5 })` for `Call::timeout` and `Ok(())` for the stream (`broken/z01-*.txt`,
  `broken/s01-*.txt`): tokio's zero sleep is not ready at its first poll, so the call's work can
  finish first. F392 had recorded a `Timeout` with or without the `req` sent; a zero could also
  succeed.

### 2. The retry paths: the client has none

Line numbers are the batch's tree.

- **The client repeats no call.** No retry policy exists in `ulo-rpc`'s client or in any link's client
  side, and none is configurable. Every place an error is produced returns it at once:
  `Call::into_future` (`client.rs:832`), `Emit::into_future` (`client.rs:877`) and
  `StreamCall::into_future` (`client.rs:974`) return each refusal and each failure of `bounded`;
  `Spec::open` returns a link's refusal of the opening frame, `payload_too_large` among them, with no
  second send (`client.rs:516`); `Exchange::single` returns the server's `err`, any kind
  (`client.rs:694`); a stream's `replies` yields it and ends (`client.rs:1025`).
- **The one loop.** `ClientInner::register` (`client.rs:531-540`) tries twice to register a call on a
  connection, the second time on a new connection when the first's reply lane ended between
  `connection` handing it over and the registration (F364). It runs before any frame is sent and
  repeats no frame; every per-call refusal returns before it.
- **Loops that are not a call's.** UDP sends each datagram once (`ulo-rpc-udp/src/link.rs:40`). Kafka's
  `coordinator_retried` (`ulo-rpc-kafka/src/link.rs:534`, the condition at `:548`) retries the server's
  committed-offsets read on three coordinator codes. The TCP server's accept loop pauses after a
  failed accept (`ulo-rpc-tcp-core/src/link.rs:412`). The core's readiness `retries`
  (`ulo/src/binding/handle.rs:354`) belong to a binding's readiness check, which `RpcClientModule`
  does not declare.
- **The test** is "attempted exactly once", `a_bad_request_is_attempted_once` in `client.rs`, below.
  No retry path retries `BadRequest`, so nothing is filed.

### 3. F394: what each path did with an unreadable `deadline-ms`, and where it is refused

- **The probe** (`probes/deadline-ms-before.txt`, source `probes/probe_deadline_ms.rs`), on `a242cffd`,
  a scripted link delivering hand-written frames: a `req` to a handler that answers at once, a `req`
  to one that waits 200 ms, and an `evt`, each with `deadline-ms` of each form.

  | `deadline-ms` | the dispatcher (`dispatch.rs:200`) | the held `cancel` (`held.rs:32`) |
  | --- | --- | --- |
  | `18446744073709551615999` | handler ran, no deadline; `res`, acknowledged; event ran, acknowledged | `HoldUntil(None)`, the fallback hold |
  | `abc` | the same | the same |
  | `-5` | the same | the same |
  | the empty value | the same | the same |
  | `0` | handler ran, 0 ns left; the quick one `res`, the waiting one `timeout`; event ran, acknowledged | a hold of 0 ns |
  | `1`, `50`, ` 50 `, `+50` | handler ran with the deadline; the waiting one `timeout` | a hold of the value |

  Nothing logs an unreadable value, by reading. A third path reads the header: the client, for a
  `deadline-ms` the caller set (`client.rs:456`), F395 below.
- **The server paths.** Every link delivers to one dispatcher, so `dispatch.rs`'s `start` is the one
  server path; `held.rs` is the client side of the NATS, RabbitMQ, MQTT and Kafka links, reading the
  `open` it sends. The dispatcher ignored each unreadable form, filed as F394 and fixed.
- **The fix.** `CallHeaders::deadline` answers `Err` with the value for one `parse::<u64>()` refuses
  after trimming. `start` reads it after routing and before admission and, for `Err`, refuses: a
  call with an id and a reply path through `refuse`, the refusal task the drain already counts,
  then acknowledges the delivery; an event is rejected and logged at `warn` with the message. Before
  admission because the refusal is permanent: a caller refused `unavailable` with `RetryAfter` first
  would retry a call the server will never run. After routing because a pattern nothing handles is
  answered as that miss, which reads no deadline. See S2 and S4.
- **The held `cancel`** reads an unreadable value as none, `CANCEL_HOLD`, as before: the server
  refuses that call, and the link's server side has already sent `opened` for it, so the hold is
  released or bounded either way. Its doc says so.
- **Zero** is readable: `deadline-ms: 0` is a deadline already passed, not a value the server cannot
  use, and it goes through the one deadline rule. The execution opens with a deadline of now; the
  dispatcher polls the handler once before the expired sleep, as at every deadline, so a handler
  answering without awaiting is answered, and one that awaits is ended and answered `timeout`
  through the error handlers. `1` behaves the same with a millisecond left. The response does not
  settle it; see S1.
- **What parses.** `str::parse::<u64>` after `trim`, as before: ` 50 ` and `+50` set 50 ms. See S1.

## The tests

- **F392, two conformance scenarios** on every RPC link, in `deadlines.rs`, the server built over
  `Tapped`, a server link delegating to the broker's own and recording the `x-conformance-mark`
  header of every `req`, `open` and `evt` it delivers, through a new `Fixture::start_over`:
  1. `a_call_s_zero_timeout_is_refused_before_the_server_sees_it`: eight `request`s to
     `conformance.add` marked `zero` at `.timeout(Duration::ZERO)` each fail `BadRequest`, reason
     `zero_timeout`, a `ZeroTimeout` the source; a ninth marked `after` at ten seconds is answered
     `Sum { sum: 5 }`; the server delivered `["after"]`.
  2. `a_stream_s_zero_deadline_is_refused_before_the_server_sees_it`: eight `stream`s to
     `conformance.count` at `.deadline(Duration::ZERO)` refused the same way; a ninth at ten seconds
     yields `[1, 2]`, or on UDP, which mounts no streaming handler, one `pattern_unhandled` error;
     the server delivered `["after"]`.

  Eight calls because, unrefused, whether a zero call's frame is sent is decided at random by the
  client's `select!`; one call that reached no server would pass. Each call names one pattern over
  one connection, so a refused call's frame, had it been sent, would have reached the server before
  the ninth's: every broker here keeps order on one subject, channel, queue or topic, Kafka's topics
  having one partition in the suite. On UDP the scenarios run, the refusal being the client's.
- **The retry check,** `a_bad_request_is_attempted_once` in `client.rs`'s tests, on tokio's paused
  clock over `Refusing`, a link counting its connects and every frame it is asked to send: a stream
  at `idle_timeout(Bound::After(Duration::ZERO))`, a stream at `deadline(Duration::ZERO)` and a
  request at `timeout(Duration::ZERO)` each fail `BadRequest`, `zero_timeout`, and an hour later the
  link has made no connect and sent nothing; a request the server answers `err` of kind
  `bad_request` was sent once, and an hour later still once; on a link refusing every `req` as
  `FrameTooLarge`, a request fails `BadRequest`, `payload_too_large`, offered to the link once.
- **F394,** `crates/ulo-rpc/tests/bad_deadline.rs`, a server over a scripted link fed hand-written
  frames, its handlers counting their runs and recording the time left to their deadline:
  - one test per form, `an_overflowing_deadline_is_refused_before_the_handler_runs`
    (`18446744073709551615999`), `a_deadline_that_is_not_a_number_is_refused_before_the_handler_runs`
    (`abc`), `a_negative_deadline_is_refused_before_the_handler_runs` (`-5`) and
    `an_empty_deadline_is_refused_before_the_handler_runs` (the empty value), each delivering a `req`,
    an `open` with an `in` and an `in_end` behind it, and an `evt`: the `req` and the `open` each
    answered by one `err`, `bad_request`, `bad_deadline`, and acknowledged, the `evt` rejected, and no
    handler run;
  - `the_refusal_quotes_the_value_cut_short`: `abc` quoted whole, a hundred nines quoted to 64 and
    marked cut short;
  - `a_readable_deadline_still_sets_the_execution_s`: `5000` answered, its handler finding between
    four and five seconds left; `50` to a handler waiting a minute answered `timeout` within five
    seconds, the handler finding at most 50 ms left;
  - `a_zero_deadline_is_one_already_passed`: `0` to the waiting handler answered `timeout`, the
    handler having run once with no time left.

### Before and after

Each break went through `batch31/scripts/brk.py`: it replaces each span, asserting it occurs once,
runs the targets under bash, writes every touched file and `Cargo.lock` back byte for byte, and
compares a SHA-256 over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`, `scripts`
and `unsafe-allowed.txt` and every untracked file under `crates` and `scripts`. Specs in
`batch31/specs/`, output in `batch31/broken/` with `summary.txt`. Every restore reported `ok`, on the
tree hashed `e199567c..`, the tree every verification run below used (`verify/final-tree-hash.txt`).
The TCP target is its conformance suite filtered to the two new scenarios and batch 30's zero idle
timeout scenario; the unit target is `a_bad_request_is_attempted_once`.

| Break | Target | Result |
| --- | --- | --- |
| z01. F392's code: a call's zero not refused | TCP; unit | `a_call_s_zero_timeout_is_refused_before_the_server_sees_it` failed, "expected an error of kind BadRequest, got: Ok(Sum { sum: 5 })"; the unit test failed, `(Timeout, None)` where `(BadRequest, Some("zero_timeout"))` was due |
| z02. a call's zero refused after its `req` is sent | the same | the call scenario failed, the server having delivered `["zero" ×8, "after"]`; the unit test failed, `(1, ["req", "cancel"])` where `(0, [])` was due |
| s01. F392's code: a stream's zero deadline not refused | the same | `a_stream_s_zero_deadline_is_refused_before_the_server_sees_it` failed, "got: Ok(())"; the unit test failed, `(Timeout, None)` |
| s02. a stream's zero deadline refused after its `req` is sent | the same | the stream scenario failed, `["zero" ×8, "after"]`; the unit test failed, `(1, ["req", "cancel"])` |
| r01. a link's `FrameTooLarge` retried once | unit | failed, "a request the link refused as too large", `(1, ["req", "req"])` |
| r02. a server's `bad_request` answer retried once | unit | failed, "a request the server answered `bad_request`", `(1, ["req", "req"])` |
| r03. a zero idle timeout connecting before its refusal | unit | failed, "three calls refused before connecting", `(1, [])` |
| d01. F394's code: an unreadable `deadline-ms` as none | `bad_deadline.rs` | the four forms and the quoting failed, each request answered `[Res { id: 1, .. }]`; readable and zero passed |
| d02. a refused event acknowledged | the same | the four forms failed, "an event whose `deadline-ms` is .. was not rejected", `ack: Some(true)` |
| d03. a refused request left unsettled | the same | the four forms and the quoting failed, the acknowledgment `None` |
| d04. zero refused as unreadable | the same | `a_zero_deadline_is_one_already_passed` failed, answered `bad_request` where `timeout` was due |
| d05. a readable `deadline-ms` ignored | the same | the readable and zero tests failed, the waiting handler unanswered within five seconds |
| d06. the value quoted to 128 characters | the same | `the_refusal_quotes_the_value_cut_short` failed, the hundred nines quoted whole |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `a242cffd`.

- §5.2's grammar paragraph (line 946): "A `deadline-ms` that does not parse sets no deadline" goes; a
  `req` or `open` whose `deadline-ms` does not parse as a `u64` is answered `err` of kind
  `bad_request`, `reason: "bad_deadline"`, before admission and before any execution opens, and
  acknowledged; an `evt` is rejected and logged at `warn`; zero is a deadline already passed.
- §5.2's scenario list (line 966): the two zero-bound scenarios, the server's link recording what it
  delivers.
- §5.4's client paragraph (line 1096) and X38 (line 1407): `Call::timeout(Duration::ZERO)` and
  `StreamCall::deadline(Duration::ZERO)` fail at the await, before connecting, `BadRequest`,
  `zero_timeout`, a `ZeroTimeout` the source; the client has no retry policy, and every `BadRequest`
  is final.
- §12's refusals table (after line 1464, the binary payload's row, beside batch 30's row for a
  stream's zero idle timeout): a call's `.timeout(Duration::ZERO)` and a stream's
  `.deadline(Duration::ZERO)`, runtime before any I/O, `RpcError` of kind `BadRequest`, `reason:
  "zero_timeout"`, its source a `ZeroTimeout`; and, after line 1478, a call whose `deadline-ms` the
  server cannot read, runtime, an `err` of kind `bad_request`, `reason: "bad_deadline"`, an event
  rejected.
- Decision 61 (line 1568) and decision 111 (line 1618): a per-call zero is refused at the await, on
  all three per-call bounds.

## Needs sign-off

Numbered from S1 for this batch.

### S1. A zero `deadline-ms` is a deadline already passed, and what parses is `u64::from_str` after trimming

Decision 3. Zero is readable and goes through the deadline rule unchanged: the handler is polled
once before the expired sleep, as at every deadline, so a handler that answers without awaiting is
answered. The alternatives are refusing zero `bad_deadline`, which calls a well-formed value
malformed, and answering `timeout` without polling the handler, which treats `0` unlike `1`, whose
handler is polled the same way with a millisecond left. On parsing: ` 50 ` and `+50` set 50 ms, as
before the batch; the alternative is ASCII digits alone, refusing both.

### S2. The refusal is written without an execution, so no error handler sees it

Decision 3. A `bad_deadline` refusal goes through `refuse`, as the over-limit and draining refusals
do: no execution, no guard, no error handler, no call span. The alternative is a `CallError` offered
to the global error handlers through `ulo::recover`, as a pattern nothing handles is, which needs an
execution opened outside admission and lets a `#[catch]` reshape it.

### S3. A per-call zero's message names the zero as the setter takes it

Decision 1. The `ZeroTimeout` for `Call::timeout` and `StreamCall::deadline` reads `Duration::ZERO`
and says what to write instead of `Bound::Unbounded`, which neither takes until F393. The alternative
is batch 30's one wording, which would point the caller at a spelling that does not compile there.

### S4. A refused request is acknowledged, a refused event rejected

Decision 3. The request was answered, so a broker redelivering it would have it refused again; an
event has no one to tell, and is rejected as an event nothing handles is, so it cannot loop. The
alternative is leaving both unsettled, as the over-limit refusal leaves a request.

### S5. F395: the client's handling of a caller's own unreadable `deadline-ms`

Not changed. The client drops such a header where it stamps a deadline and passes it through where
it does not, so the same mistake is ignored on a request and refused `bad_deadline` on a stream or an
unbounded request. The candidate is the client refusing it at the await, before connecting,
`BadRequest`, `bad_deadline`.

### S6. F396: the gRPC server ignores a `grpc-timeout` off the grammar

Not changed; the response named RPC's server. The same argument applies there, and the candidate is
refusing the call before its handler runs, the code to choose.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch31/`: `suites/` for the RPC
suites, with `summary.txt` and the container lists before and after each, `verify/` for the checks,
the workspace test, docs and clippy, `broken/` and `specs/` for the breaks, `probes/` for the two
probes and their source, `scripts/`. Every run below ran on the tree hashed `e199567c..`.

- **The RPC suites, once each,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 42 passed and 3 ignored each (2.02 s each); the
  smol TCP link's three 42 and 3 each (2.01 s, 2.01 s, 2.02 s); UDP 39 and 6 (1.31 s). With
  `--features integration --test conformance` and the OpenSSL flags: NATS 44 and 1 (4.79 s); Redis 43
  and 3 (6.63 s); MQTT 49 and 1 (7.13 s); Kafka 45 and 1 (52.69 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 44 and 1 (25.61 s). Each count is batch 30's and the two new scenarios,
  which passed on all twelve.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 16 unit, `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 1 and 5 ignored, then the
  `compile_fail` one.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 456 files scanned, 2 listed, 0 findings, exit 0
  (`verify/unsafe-check.txt`).
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, 0 failures, 0 errors (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, `ulo`'s 17 known warnings in `crates/ulo/src` and no location elsewhere
  (`verify/check-all-features.txt`, `verify/check-defaults.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in `target/msrv-1.88`: exit 0, the same 17 and no other
  (`verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast` with the OpenSSL flags: exit 0,
  1,179 passed, 0 failed, 141 ignored, over 200 `test result` lines (`verify/workspace-test.txt`).
  Batch 30's run had 1,157 over 199. The 22 more: the two new scenarios in each of the seven
  conformance binaries that build without `integration`, TCP's three, the smol TCP link's three and
  UDP's; `a_bad_request_is_attempted_once`; and `bad_deadline.rs`'s seven, its binary the new line.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc` and
  `ulo-rpc-conformance`, the last at its defaults and with `tokio,smol`: exit 0 each (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-conformance --all-targets --all-features --no-deps
  --locked`, with the OpenSSL flags: exit 0, 9 warning locations outside `crates/ulo/src`
  (`verify/clippy.txt`). One is on a changed line, `app.rs:528`, `result_large_err` on `bound`'s
  return type, whose parameters the batch changed; the same lint at the same function on `a242cffd`,
  run in a detached worktree, is `verify/clippy-head-conformance.txt`.
- **Containers:** at the batch's start twelve containers, the user's four, postgres:18, seaweedfs,
  redis:7 and mailpit, among them, every one exited (`verify/docker-start.txt`). Every suite's list
  after it matches its list before, the same twelve, none the suites started left behind
  (`suites/*.docker-{before,after}.txt`, `verify/docker-end.txt`). No permission check refused an
  action.
- **Disk:** 216 GB free at the start, 194 GB at the end.
