# Divergences: race 2b, tests batch 33, a zero `grpc-timeout` answered DEADLINE_EXCEEDED before admission (F399), both deadline encoders held to their grammar's longest value (F405, F406), and `traceparent` described as an ordinary header (F400)

The fifty-eighth response, signed off 2026-10-17, answers `race2b-tests32.md`. The gRPC server now
answers a `grpc-timeout` of zero, in any unit, DEADLINE_EXCEEDED right after the grammar check,
before admission and before any execution opens, as grpc-go answers it; against the code before the
fix a unary handler that answers without awaiting was run and answered. The encoder premise was
checked by a sweep of 4,016 durations per encoder, each read back by the server's own parser. ulo's
gRPC client writes `grpc-timeout` through tonic 0.14.6, which picks the finest unit whose amount
fits eight digits, so a positive remainder never encodes as zero and no value exceeds eight digits,
but tonic panics past `99999999H`, about 11,415 years: 776 of the sweep's durations panicked (F405).
The RPC client writes a sub-millisecond remainder as `1`, never `0`, but wrote a remainder past
`u64::MAX` milliseconds, about 584 million years, as a number its own server refuses `bad_deadline`:
253 of the sweep's durations (F406). Each encoder now holds a remainder to the longest value its
grammar holds, and both sweeps pass. `traceparent` appeared as reserved in one place in the code,
`CallHeaders`'s doc, which now describes it as an ordinary header that nothing reserves, writes or
refuses.

Files changed: for F399, `crates/ulo-grpc/src/dispatch.rs` and `crates/ulo-codegen-tests/tests/deadlines.rs`;
for F405, `crates/ulo-grpc/src/client.rs` and `crates/ulo-codegen-tests/tests/clients.rs`; for F406,
`crates/ulo-rpc/src/client.rs`; for F400, `crates/ulo-rpc/src/transport.rs` and a test in
`crates/ulo-rpc/src/client.rs`; in the workspace's `FRAMEWORK_GAPS.md`, F405 and F406 new with their
fix notes, and fix notes under F399 and F400, their commit written `<commit>`. The batch started from
`492a1b7f`, and no commit landed during it. Versions are unchanged at `0.1.0-alpha.1`, and
`Cargo.lock` is unchanged.

## The signatures

```rust
// crates/ulo-grpc/src/client.rs, private
const LONGEST: Duration = Duration::from_secs(99_999_999 * 3600);
fn timed<T>(request: tonic::Request<T>, remaining: Duration) -> tonic::Request<T>;

// crates/ulo-rpc/src/client.rs, private
fn deadline_ms(remaining: Duration) -> String;
```

No public signature of `ulo-rpc` or `ulo-grpc` changes. What changes in meaning without one:

- A gRPC call whose `grpc-timeout` is zero, `0S`, `0n`, `00000000H` or any other unit, is answered
  DEADLINE_EXCEEDED, message "the call's deadline passed", before admission: no execution, no
  pre-dispatch stage, no span, no handler, no error handler. Before the batch the execution opened
  with a deadline of now and the pipeline was polled once, so a handler that answers without
  awaiting was answered OK, and one that awaits was dropped `Deadline` and offered to the error
  handlers. The health and reflection services, reached through the same `Dispatcher::call`, are
  answered the same way.
- `ulo_grpc::outgoing` writes `99999999H` for an execution whose deadline is further off than that,
  where it panicked in tonic.
- The RPC client writes `deadline-ms: 18446744073709551615` for a remainder longer than `u64::MAX`
  milliseconds, where it wrote the full count, which the server refused `bad_deadline`.
- `CallHeaders`'s doc no longer names `traceparent` as reserved. Nothing in its behaviour changes.

## Decisions

Line numbers are those of `492a1b7f` unless a line says otherwise.

### 1. F399: where the zero is answered, and with what

- **Where.** `Dispatcher::call` (`crates/ulo-grpc/src/dispatch.rs:148` on the batch's tree) tests
  for `Some(Duration::ZERO)` right after the grammar check and before admission, the line batch 32's
  off-grammar refusal sits on and the fifty-eighth response's S3 accepts. `parse_timeout` reads a
  zero amount as `Duration::ZERO` in every unit, so one test covers `0H` to `0n`; `00000000S` is
  eight digits and on the grammar.
- **What.** `status::deadline_exceeded()`, the status a deadline passing during the call answers when
  no error handler claims it, message "the call's deadline passed". grpc-go's message is "context
  deadline exceeded"; the code is what interoperates, and one message for one condition on one
  server was preferred to copying grpc-go's text.
- **No error handler.** The call never opens an execution, so a `Timeout` reaches no error handler,
  where before the batch an error handler could claim the zero's expiry. The test's `StallClaimed`
  handler, whose error handler answers UNAVAILABLE for a `Timeout`, is answered DEADLINE_EXCEEDED.
- **A positive amount** stays as it was: `1n` opens an execution with a nanosecond left and the
  pipeline is polled once, as RPC's `deadline-ms: 1` is. The response ruled on zero alone.
- **A stale doc block.** Batch 32 left the old `grpc_timeout` doc ("A value off that grammar is
  ignored, logged at `debug`, and the call runs without a deadline") above the `QUOTED` constant,
  where it read as that constant's doc (`dispatch.rs:456-458` at `492a1b7f`). It is removed, and the
  current function's doc says how a zero is read.

### 2. The encoder premise on gRPC: tonic writes it, and panics past `99999999H` (F405)

- **What writes it.** `outgoing` (`crates/ulo-grpc/src/client.rs:108`) is the one place ulo writes
  `grpc-timeout`; it calls tonic's `Request::set_timeout`, whose `duration_to_grpc_timeout`
  (`tonic-0.14.6/src/request.rs:397-429`) tries `n`, `u`, `m`, `S`, `M` and `H` in that order and
  writes the first whose truncated amount is at most 99,999,999, ending in `.expect("duration is
  unrealistically large")`.
- **Rule 1 holds.** A remainder under 100 ms is written in nanoseconds, exact, so no positive
  remainder is ever smaller than its unit and none encodes as zero. A passed deadline is sent as
  `1n` by `outgoing`'s own floor.
- **Rule 2 holds up to `99999999H`.** One hour is written `3600000m`, 24 hours `86400000m`, 100
  days `8640000S`. Past 99,999,999 hours tonic panics. Reachable, by reading: the RPC server opens an
  execution `deadline-ms` from now, up to `u64::MAX` milliseconds, with an unchecked `+` that
  decision 121 records as not overflowing (`crates/ulo-rpc/src/dispatch.rs:221`), and a handler
  forwarding that deadline with `outgoing` panicked; so did any execution given such a deadline
  through `ExecOptions::deadline`, which `outgoing_forwards_a_deadline_beyond_the_grammar_as_its_longest_value`
  does.
- **The fix.** `timed` holds the remainder between one nanosecond and `99999999H` before tonic
  writes it. The alternative was sending no `grpc-timeout` for such a deadline, which the spec reads
  as an infinite timeout; holding it keeps a bound and differs from no bound only after 11,415 years.
- **Truncation.** Within the grammar tonic truncates, so the callee's deadline is up to one unit
  earlier than the caller's: exact under 100 ms, up to a microsecond under 100 s, a millisecond
  under about 27.8 hours, a second under about 3.2 years. grpc-go's encoder
  (`internal/grpcutil/encode_duration.go` at v1.84.0, read from the repository) tries the same units
  in the same order but rounds each amount up, `div` adding one for any remainder, and writes `0n` for
  a zero or negative duration, where `outgoing` writes `1n`. See S2.

### 3. The encoder premise on RPC: sound below `u64::MAX` milliseconds (F406)

- **What writes it.** `Spec::limit` (`crates/ulo-rpc/src/client.rs:485`) wrote
  `stamped.as_millis().max(1)`, a `u128`, now `deadline_ms` (`client.rs:435` on the batch's tree).
- **Sub-millisecond.** `max(1)` writes a positive remainder under a millisecond as `1`, never `0`,
  as the response requires. A larger remainder is truncated to whole milliseconds, up to one
  millisecond short.
- **Past `u64::MAX` milliseconds.** The server reads `deadline-ms` as a `u64`
  (`crates/ulo-rpc/src/transport.rs:247`). `limit` drops a bound an `Instant` cannot hold, F391's
  fix, but on macOS an `Instant` holds about 292 billion years and the header about 584 million, so
  a stream's `.deadline(Duration::from_secs(1 << 62))` was sent as `4611686018427387904000` and
  would have been refused `bad_deadline`. `deadline_ms` writes `u64::MAX` for it, the longest value
  the server reads, on the same reasoning as decision 2's fix.

### 4. F400: one site in the code

A search of the repository outside `target/` for `traceparent`, case-insensitive, finds it in the
code once, `CallHeaders`'s doc (`crates/ulo-rpc/src/transport.rs:208-213` on the batch's tree). The doc now names
`deadline-ms` alone as reserved and says a caller sets `traceparent` with `.header` to propagate
trace context, and nothing in ulo reserves, writes or refuses it. Beyond the code it appears in
transports DESIGN §5.2 (line 946), left for the fold, `RESPONSE.md` and batch 32's log. The docs
site's `content/` does not mention it. Nothing refuses it, and a test now holds that.

### 5. How the sweeps are built

- **Unit tests beside the encoders.** Both encoders are private functions and both decoders are
  `pub(crate)`, so each sweep is a unit test in the encoder's crate, decoding with the server's own
  parser: `dispatch::grpc_timeout` on gRPC and `CallHeaders::deadline` on RPC. It is `ulo-grpc`'s
  first unit test.
- **The durations.** Sixteen named ones per encoder, among them 1 ns, 999 ns, 1 µs + 1 ns, 1 ms −
  1 ns, 1 s, 1 min, 1 h, 24 h, 100 days, each encoder's longest value and one nanosecond, one unit
  past it, and `Duration::MAX`; then 4,000 drawn by a xorshift generator with a fixed seed, half as
  nanoseconds and half as seconds and nanoseconds, each shifted right by a random amount so every
  magnitude a `Duration` holds is drawn.
- **What each asserts.** On gRPC: at most eight ASCII digits and one of `H M S m u n`; read back
  positive; and read back less than one unit away, or as `99999999H` for a longer duration; a zero
  remainder written `1n`. On RPC: ASCII digits alone; read back positive; and read back less than a
  millisecond away, or as `u64::MAX` milliseconds for a longer duration.
- **Every failure reported.** The gRPC sweep catches a panic per duration, so one run reports every
  duration tonic panics on rather than the first, and each sweep's assertion counts its failures and
  shows the first ten.
- **The seams.** A sweep calls the encoder directly, so each fix also has a test through the
  function that calls it: `outgoing` over an execution whose deadline is 400 billion seconds off,
  and `limit` over two streams, one with a 500 µs deadline written `1` and one with a 2^62 s
  deadline written `18446744073709551615`.

## The tests

- **F399, two tests in `crates/ulo-codegen-tests/tests/deadlines.rs`**, replacing
  `a_zero_grpc_timeout_is_one_already_passed`, which asserted the old behaviour, through the plain
  HTTP/2 client with raw metadata: `a_zero_grpc_timeout_is_answered_deadline_exceeded_before_the_handler_runs`
  (`0S`) and `a_zero_grpc_timeout_in_any_unit_is_answered_deadline_exceeded_before_the_handler_runs`
  (`0H`, `0M`, `0m`, `0u`, `0n`, `00000000S`). Each value goes to `Quick`, a unary handler answering
  without awaiting, `Drip`, a streaming one, and `StallClaimed`, whose error handler would claim a
  `Timeout` as UNAVAILABLE; each is answered DEADLINE_EXCEEDED, then a call without the header is
  answered, and the only handler run is that last one's.
- **F405, two tests:** `every_remaining_time_is_written_on_the_grammar_and_never_as_zero` in
  `crates/ulo-grpc/src/client.rs`, the sweep; and
  `outgoing_forwards_a_deadline_beyond_the_grammar_as_its_longest_value` in
  `crates/ulo-codegen-tests/tests/clients.rs`, an app's execution with a deadline 400 billion
  seconds off, whose request `outgoing` writes as `99999999H`.
- **F406, two unit tests in `crates/ulo-rpc/src/client.rs`:**
  `every_remaining_time_is_written_as_milliseconds_the_server_reads_and_never_as_zero`, the sweep;
  and `a_deadline_is_sent_as_milliseconds_the_server_reads`, the two streams over `SilentLink`.
- **F400, one unit test in `crates/ulo-rpc/src/client.rs`:** `a_caller_s_traceparent_is_sent_as_set`,
  a request and a stream carrying a W3C `traceparent` set with `.header`, the request timing out
  unanswered rather than refused and both opening frames carrying the value as set. `SilentLink`
  now records one named header, `deadline-ms` unless built with `SilentLink::recording`, so this test
  reads `traceparent` through it.

### Before and after

Each fix was tested before it was made: the F399 test against the dispatcher as it stood, and each
sweep against its encoder factored out unchanged, then the fix applied
(`batch33/before/`). The breaks then went through `batch33/scripts/brk.py`, batch 32's script
unchanged: it replaces each span, asserting it occurs once, runs the targets under bash, writes every
touched file and `Cargo.lock` back byte for byte, and compares a SHA-256 over `git diff HEAD` of
`crates`, `Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every
untracked file under `crates` and `scripts`. Specs in `batch33/specs/`, output in `batch33/broken/`
with `summary.txt` and the container list before and after each. Every restore reported `ok`, on the
tree hashed `eaa6c283..` (`verify/tree-hash-before-breaks.txt`).

| Run | Target | Result |
| --- | --- | --- |
| before F399: the test written, the dispatcher as it stood | gRPC `deadlines` | both zero tests failed, the unary call answered, "expected the call to fail, got Tick { n: 1 }"; the other 18 passed (`before/f399-deadlines.txt`) |
| before F405: the sweep over `timed` as `outgoing` wrote it | `ulo-grpc` unit | failed, "776 of the sweep's durations failed", each "panicked: Some(\"duration is unrealistically large\")", the first at 360,000,000,000 s, `99999999H` and one hour (`before/grpc-encoder-sweep.txt`) |
| before F406: the sweep and the seam test over `deadline_ms` as `limit` wrote it | `ulo-rpc` unit | both failed: "253 of the sweep's durations failed", each "which the server cannot read", and the 2^62 s stream sent `4611686018427387904000` (`before/rpc-encoder-sweep.txt`) |
| g01. F399's check removed | gRPC `deadlines` | both zero tests failed, "expected the call to fail, got Tick { n: 1 }"; the other 18 passed |
| g02. a zero answered INTERNAL | gRPC `deadlines` | both zero tests failed, `Internal` where `DeadlineExceeded` was due |
| e01. `timed` not held to `99999999H` | `ulo-grpc` unit; `clients` | the sweep failed, 776 durations; `outgoing_forwards_a_deadline_beyond_the_grammar_as_its_longest_value` failed, panicking in tonic, "duration is unrealistically large"; the other five client tests passed |
| e02. `timed` without its one-nanosecond floor | `ulo-grpc` unit; `clients` | the sweep failed, a zero remainder written `"0n"` where `"1n"` was due; the six client tests passed, the passed-deadline call in `outgoing_forwards_the_execution_s_remaining_deadline` answered DEADLINE_EXCEEDED by F399's server either way |
| r01. `deadline_ms` not held to `u64::MAX` | `ulo-rpc` unit | both encoder tests failed, 253 durations and `4611686018427387904000`; the other 19 passed |
| r02. `deadline_ms` without its floor of `1` | `ulo-rpc` unit | both encoder tests failed, 693 sub-millisecond durations written `"0"`, and the 500 µs stream sent `"0"`; the other 19 passed |
| t01. `traceparent` refused as `deadline-ms` is | `ulo-rpc` unit | `a_caller_s_traceparent_is_sent_as_set` failed, `BadRequest` where `Timeout` was due; the other 20 passed |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `492a1b7f`.

- §5.2's grammar paragraph (line 946): "and `traceparent` (W3C Trace Context)" leaves the reserved
  list; `traceparent` is an ordinary header a caller sets with `.header` to propagate trace context,
  which nothing reserves, writes or refuses (F404 holds the built-in propagation). The stamp is whole
  milliseconds, a remainder under one written `1` and one past `u64::MAX` written `u64::MAX`.
- §5.4's client paragraph (line 1096): the same two bounds on the stamp beside the zero bounds'.
- §6.1's deadlines bullet (line 1169), on top of batch 32's entry for the off-grammar refusal: a
  zero `grpc-timeout`, in any unit, is answered DEADLINE_EXCEEDED before admission and before any
  execution opens, grpc-go's answer, so no handler or error handler runs.
- §6.3's client paragraph (line 1190 on): `outgoing` writes the remaining time through tonic,
  between one nanosecond and `99999999H`, tonic choosing the finest unit that fits eight digits and
  truncating.
- §12's refusals table (beside the row at line 1480, and batch 32's off-grammar row): a zero
  `grpc-timeout`, runtime, DEADLINE_EXCEEDED, no execution.
- Decision 121 (line 1631): a bound an `Instant` holds past `u64::MAX` milliseconds is stamped
  `u64::MAX`; and a decision after batch 32's: a zero `grpc-timeout` is answered DEADLINE_EXCEEDED
  before admission for interoperability with grpc-go, probed at 1.84.0, where RPC's zero
  `deadline-ms` still runs its handler (F401).

## Needs sign-off

Numbered from S1 for this batch.

### S1. A remainder past the grammar is held to its longest value rather than sent as no deadline

Decisions 2 and 3. `outgoing` writes `99999999H` and the RPC client `u64::MAX` for a longer
remainder. The alternative on both is sending no header, which the gRPC specification reads as an
infinite timeout and RPC's server as no deadline; it differs from holding only after 11,415 years on
gRPC and 584 million on RPC, and it would make the longest bound the one bound not sent.

### S2. Within the grammar, both encoders truncate, so a callee's deadline is up to one unit early

Decisions 2 and 3. tonic truncates `grpc-timeout` to its unit and the RPC client truncates
`deadline-ms` to whole milliseconds, each above its floor; grpc-go's `grpc-timeout` encoder rounds up
(decision 2). Truncating has the
callee give up no later than the caller, by less than one unit; rounding up would have the callee
run up to one unit after the caller has stopped waiting. Neither response asked for a change, and
the gRPC side is tonic's, which ulo would replace with its own encoder to round up.

### S3. A zero `grpc-timeout` reaches no error handler

Decision 1. The zero is answered before an execution opens, so no `#[catch]` or error handler sees
a `Timeout` for it, where before the batch an error handler could claim it as it claims any passed
deadline. This is the line the fifty-eighth response drew ("before admission, with no execution
opened"), recorded because it changes what an application's timeout handling observes on gRPC alone.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch33/`: `before/` for each
test run against the code before its fix and after it, `suites/` for the RPC suites, with
`summary.txt` and the container lists before and after each, `verify/` for the checks, the workspace
test, docs and clippy, `broken/` and `specs/` for the breaks, `scripts/`. Every run below ran on the
tree hashed `eaa6c283..`, the hash read before the breaks, before the verification and after it
(`verify/tree-hash-before-breaks.txt`, `verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`).

- **The RPC suites, once each, all twelve,** since the RPC encoder changed, one target at a time,
  `--locked`, libtest's time (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 43 passed and 3
  ignored each (2.02 s, 2.02 s, 2.06 s); the smol TCP link's three 43 and 3 each (2.02 s, 2.02 s,
  2.05 s); UDP 40 and 6 (1.32 s). With `--features integration --test conformance` and the OpenSSL
  flags: NATS 45 and 1 (4.82 s); Redis 44 and 3 (6.69 s); MQTT 50 and 1 (7.20 s); Kafka 46 and 1
  (49.03 s); RabbitMQ with `ULO_CONFORMANCE_PARALLEL=6` 45 and 1 (22.67 s). Each count is batch 32's;
  the batch adds no scenario.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `deadlines` 20, `builtins` 6,
  `clients` 6, `compile_fail` 1, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6, all passed;
  `ulo-grpc` (`verify/ulo-grpc-tests.txt`) 1 unit test, the sweep, and its 5 doctests ignored.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 21 unit, `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 1 and 5 ignored, then the
  `compile_fail` one.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 456 files scanned, 2 listed, 0 findings, exit 0
  (`verify/unsafe-check.txt`).
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, 0 failures, 0 errors (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, both with `CFLAGS=-I/opt/homebrew/opt/openssl/include
  LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: exit 0, `ulo`'s 17 known warnings, their 24 location
  lines all in `crates/ulo/src` (`verify/check-all-features.txt`, `verify/check-defaults.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in `target/msrv-1.88`: exit 0, the same 17 and no other
  (`verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags:
  exit 0, 1,203 passed, 0 failed, 141 ignored, over 198 lines starting `test result:`
  (`verify/workspace-test.txt`). Batch 32's run had 1,197 over 198. The six more: the zero tests'
  two less the one they replace, the `outgoing` seam test, `ulo-grpc`'s sweep, and in `ulo-rpc` the
  sweep, the stream seam test and the `traceparent` test.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc` and
  `ulo-grpc`: exit 0 each (`verify/doc-ulo-rpc.txt`, `verify/doc-ulo-grpc.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-grpc -p ulo-codegen-tests --all-targets --all-features
  --no-deps --locked`, with the OpenSSL flags: exit 0, 10 warning locations outside
  `crates/ulo/src` (`verify/clippy.txt`), none on a line the batch changed: in `ulo-rpc` three
  `type_complexity`, two `result_large_err` and three `wrong_self_convention`; in
  `ulo-grpc` `result_large_err` at `extract.rs:105` and `collapsible_if` at `dispatch.rs:822`, batch
  32's line 819 moved by the three lines F399 adds above it.
- **Containers and images:** at the batch's start twelve containers, the user's four of
  `backend-next`, postgres:18, seaweedfs, redis:7 and mailpit, among them, every one exited
  (`verify/docker-start.txt`). Every suite's and every break's list after it matches its list
  before, the list at the end matches the start's (`verify/docker-end.txt`), and the image list at
  the end matches the start's (`verify/docker-images-start.txt`, `verify/docker-images-end.txt`). The
  batch pulled no image, ran no container of its own and ran no `docker ... prune`. No permission
  check refused an action.
- **Disk:** 191 GB free at the start, 190 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`).
