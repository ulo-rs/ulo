# Divergences: race 2b, tests batch 27, a stream's per-frame wait renamed `.idle_timeout(..)` beside a new whole-call `.deadline(..)` that stamps `deadline-ms`, the client's timeout reaching only `request`, `send_stream` and `emit` (S1), and `unsafe` denied workspace-wide with three listed modules opting out, a check that holds the list and a Miri job over it, after `FuturesIo`'s two `unsafe` calls were replaced by a copy (S4)

The fiftieth response, signed off 2026-10-11, answers `race2b-tests25.md`. On `stream` and `duplex`
`.timeout(..)` is gone: `.idle_timeout(d)` is the longest silence the client tolerates between reply
frames and is not sent, and `.deadline(d)` bounds the whole call and is sent as `deadline-ms`, the
shortest of it, the calling execution's remaining time and a header the caller set, as a request's
timeout is. A stream has neither unless the call sets it, and the client's timeout, which
`RpcClientModule::timeout` and `RpcClient::timeout` set, now means one thing: the whole-call bound of
a `request` or a `send_stream` that sets none, and the bound on an `emit`'s publish. The old spelling
on a stream is a compile error, which a `compile_fail` doctest holds. `FuturesIo` reads into a buffer
of its own, at most 64 KiB, zeroed only as it grows and released whenever the stream has nothing to
read, and copies into hyper's; no `unsafe` is left in it. The workspace denies `unsafe_code`
through `[workspace.lints.rust]`, every member inherits the table, and the three modules whose
`unsafe` has no safe equivalent, each a call into the operating system, opt out with
`#![allow(unsafe_code)]` and are listed in `unsafe-allowed.txt`. `scripts/unsafe_allowed.py check`
fails on `unsafe` or a relaxed `unsafe_code` level anywhere else, and a new `miri` job runs it and
then Miri over the listed modules' tests. Miri runs none of them: each stops at a system call Miri
refuses, and `dev.rs` has none (F388).

Files changed: `Cargo.toml` (the workspace lint table) and the `Cargo.toml` of all fifty members
(`[lints] workspace = true`); for S1, `crates/ulo-rpc/src/{client.rs, client_module.rs}` and
`crates/ulo-rpc-conformance/src/{lib.rs, cases/app.rs, cases/deadlines.rs, cases/cancel.rs,
cases/drain.rs, cases/errors.rs, cases/streams.rs, cases/threads.rs}`; for S4,
`crates/ulo-hyper-serve/src/rt.rs`, `crates/ulo-net/src/activation.rs`,
`crates/ulo-cli/src/commands/dev.rs`, `crates/ulo-http-hyper/tests/listeners.rs`,
`.github/workflows/ci.yml`, and two new files, `unsafe-allowed.txt` and
`scripts/unsafe_allowed.py`; in the workspace's `FRAMEWORK_GAPS.md`, F388. The batch started from
`4b8a19e2`; `07edcbc6` and `6b78ef1c`, committed by another agent during the build, touch the two
DESIGN files and `transports/RESPONSE.md` alone, so the tree is `6b78ef1c` plus this batch.

## The signatures

```rust
// ulo_rpc::StreamCall<Res>, what `RpcClient::stream` and `RpcClient::duplex` answer
pub fn idle_timeout(self, idle: Duration) -> Self;        // was `timeout(self, Duration)`, the per-frame wait
pub fn deadline(self, deadline: Duration) -> Self;        // new: the whole call, sent as `deadline-ms`
// removed: pub fn timeout(self, timeout: Duration) -> Self

// ulo_rpc_conformance::cases::deadlines
stream_idle_timeout_is_per_frame       // was `stream_timeout_is_per_frame`; stamped `a_stream_s_idle_timeout_is_per_frame`
idle_timeout_ends_a_silent_stream      // new; stamped `a_stream_s_idle_timeout_ends_it_on_silence`
stream_deadline_is_the_server_s        // new; stamped `a_stream_s_deadline_is_the_server_s_deadline`
stream_with_neither_bound_runs_on      // new; stamped `a_stream_with_neither_bound_runs_past_both`
```

```toml
# Cargo.toml
[workspace.lints.rust]
unsafe_code = "deny"

# every member's Cargo.toml
[lints]
workspace = true
```

Unchanged: `Call::timeout`, `Emit`, `RpcClient::timeout`, `RpcClientModule::timeout`,
`RpcClient::new` and every other client signature; `FuturesIo`'s public API, which gains a private
field; every `Link` method; every other scenario's name.

What changes in meaning without a signature: `RpcClient::timeout` and `RpcClientModule::timeout` no
longer reach a `stream` or a `duplex` call, which waited the client's timeout for each reply frame,
five seconds at `Bound::Default`, and now waits for each as long as it takes unless the call sets
`.idle_timeout`. A stream's connect and opening frame were bounded by the client's timeout and are
now bounded by its idle timeout and deadline. A stream given `.deadline(d)` carries `deadline-ms`,
which a stream never carried from its own bound, and its handler is cancelled with `Deadline` there.
`FuturesIo` holds up to 64 KiB per connection while the stream is readable and none while it is idle,
where it held nothing of its own and zeroed hyper's whole unfilled buffer on every poll.

Dependencies: none added; `Cargo.lock` is unchanged.

## Decisions

### 1. S1: one name per meaning, and the client's timeout reaches the calls whose meaning it has

- **`StreamCall`'s builders.** `.idle_timeout(d)` and `.deadline(d)` sit on `StreamCall`, the
  builder `stream` and `duplex` both answer, and `.timeout` is removed from it, so
  `client.stream(..).timeout(d)` fails to compile with E0599. `Call`, which `request` and
  `send_stream` answer, keeps `.timeout`; `Emit` has none of its own. No other call shape has a
  builder.
- **The client's timeout.** `RpcClient::timeout` and `RpcClientModule::timeout` set one value, the
  default for a call shape whose `timeout` means the whole call: `request` and `send_stream`, sent as
  `deadline-ms`, and `emit`, where it bounds the publish and is not sent. It is not a default for a
  stream's idle timeout or deadline. Defaulting the idle timeout to it would give the module's one
  setting two meanings again; a module that wants every stream bounded would need a setting of its
  own. See S1.
- **What the idle timeout counts.** The silence before the first reply frame counts from when the
  call is awaited, so the connect and the opening frame fall under it too, and each later silence
  counts from the frame before. A frame not arriving within it ends the stream with `Timeout`, "no
  reply frame arrived within the stream's idle timeout", and dropping the stream sends `cancel`, as
  the per-frame wait did. See S2.
- **The deadline.** `Spec::limit` takes a `Whole`: `Reply` for a call answered by one reply, its
  timeout or the client's, sent; `Publish` for an event, the client's timeout, not sent; `Stream(d)`,
  the call's deadline or none, sent. What is sent is the shortest of that bound, the calling
  execution's remaining time and a `deadline-ms` the caller set, decision 106's rule unchanged. The
  client also keeps the deadline itself, as an instant fixed when the call is awaited: each wait for a
  frame is bounded by the idle timeout or the time left to that instant, whichever is shorter, and the
  stream ends with `Timeout`, "the stream's deadline passed", when the second runs out first. A link
  that drops the `cancel` still sees the server end the call at the stamped deadline, which the new
  scenario pins.
- **The callers.** The seven suite scenarios that set a stream's `.timeout(..)` as a per-frame wait,
  in `cancel.rs`, `drain.rs`, `errors.rs` (two), `streams.rs` (two) and `threads.rs`, now set
  `.idle_timeout(..)` with the same value; the eighth is the renamed per-frame scenario. Every other
  stream in the workspace set none and waited its client's timeout per frame: the suite's other
  streams the module's ten seconds, `ulo-rpc-tcp`'s `deadline_answers.rs` its module's patience and
  `interop.rs` its client's five seconds. Each now waits without a client-side bound, the first two
  inside the `within(..)` patience each already has, and `interop.rs`'s against a server in the same
  process. The suite's `CALL_TIMEOUT` comment now says it is a request's.

### 2. S4: `FuturesIo` copies

- **What was there.** `FuturesIo<T>`'s hyper `Read` zeroed the whole unfilled part of hyper's buffer
  through `ReadBufCursor::as_mut`, cast it to `&mut [u8]` for the `futures-io` read, and `advance`d
  past what was read: two `unsafe` calls. It serves every smol-hosted HTTP and WebSocket connection,
  plain and TLS, and the conformance suites' HTTP clients.
- **What it costs to copy.** A micro-benchmark in the scratchpad (`bench/`; `probes/p01-*.txt`, two
  runs of one binary) times one `poll_read` through four safe shapes and the old one, against a reader
  opaque to the optimiser that fills `n` bytes of what it is offered. hyper's unfilled region starts at
  8 KiB and grows on a fast stream. The old shape zeroed all of it on every poll, a pending one
  included. With every read ready, the old shape against a kept buffer zeroed only as it grows: 89 to
  90 ns against 14 to 16 ns for 512 bytes into 8 KiB; 158 to 174 ns against 162 to 180 ns for 8 KiB
  into 8 KiB; 1.0 to 1.1 µs against 1.2 to 1.35 µs for 64 KiB into 64 KiB; 2.6 to 3.3 µs against 0.31
  to 1.3 µs for 16 or 64 KiB into 400 KiB. On a reader pending every other poll, the old shape
  against the kept buffer released on each pending read and so allocated and zeroed again: 164 to
  192 ns against 107 to 118 ns at 512 bytes; 255 to 268 ns against 279 to 306 ns at 8 KiB into 8 KiB;
  0.89 to 0.91 µs against 1.01 to 1.10 µs at 16 KiB into 64 KiB; 1.44 to 1.51 µs against 2.6 to 2.7 µs
  at 64 KiB into 64 KiB; 5.3 to 6.0 µs against 1.05 to 2.74 µs into 400 KiB. A zeroed 8 KiB stack
  buffer per poll costs 90 to 96 ns at 512 bytes, as the old shape does, and caps every read at
  8 KiB.
- **The shape chosen.** A `Vec<u8>` on the wrapper, grown to `min(hyper's unfilled, 64 KiB)` and
  zeroed only as it grows; the read goes into it and `put_slice` copies it into hyper's cursor. When the
  inner read is pending the buffer is released, so a keep-alive connection waiting for its next request
  holds no more than it did. The 64 KiB cap bounds what one connection holds while readable, and a
  read asks the stream for at most 64 KiB where hyper offers more. See S3.
- **The doc comment** says what the wrapper does now; the doc comment's claim that each read zeroes
  hyper's buffer is gone with the code.

### 3. S4: `deny` with listed opt-outs, and a check that holds the list

- **Why not `forbid`.** `unsafe_code = "forbid"` in `[workspace.lints.rust]` reaches each member as
  `-F unsafe_code`, and a module's `#![allow(unsafe_code)]` under a `forbid` is itself an error, so
  `forbid` admits no per-module opt-out. The table says `deny`, and a listed module opts out at its
  top. `listener.rs` keeps its `#![forbid(unsafe_code)]`.
- **Every member inherits it.** No member had a `[lints]` table, so each gains
  `[lints] workspace = true` and nothing of its own is replaced. Crate-level lint attributes in
  source, the four `#![allow(dead_code)]` in test support modules and `listener.rs`'s `forbid`, stay.
- **What the compiler cannot hold alone,** since `deny` can be relaxed anywhere, the check does:
  `scripts/unsafe_allowed.py check`, standard-library Python that runs on a stock runner and on
  macOS's Python 3.9. It reads `cargo metadata --no-deps` for the members and fails on a member
  without `[lints] workspace = true`; on a workspace table whose `unsafe_code` is neither `deny` nor
  `forbid`; on a listed path that is no file, or that does not opt out with `#![allow(unsafe_code)]`;
  and, in every `.rs` file under a member that is not listed, on the token `unsafe` or on
  `unsafe_code` anywhere but inside `deny(..)` or `forbid(..)`, which catches `allow`, `warn`,
  `expect` and a `cfg_attr` around any of them. Comments, string literals, raw strings and character
  literals are blanked before it reads, so prose naming `unsafe` is not a finding, and a lifetime is
  not taken for a character literal. It scans test targets, build scripts and the `trybuild` inputs
  as well. A self-test planted a file with both kinds of comment, the four kinds of literal, a
  lifetime, `deny` and `forbid` with odd spacing, and five violations; the check reported exactly the
  five (`verify/check-selftest.txt`).
- **The list.** Every `unsafe` in a member was one of the three modules below or `FuturesIo`. Each
  of the three is a call into the operating system with no safe API in `std`, socket2 or libc:
  - `crates/ulo-net/src/activation.rs`: adopting the descriptors a supervisor hands down,
    `BorrowedFd::borrow_raw` to vet them and `OwnedFd::from_raw_fd` to adopt them, and on Apple
    targets `getsockopt(TCP_CONNECTION_INFO)` into a zeroed `tcp_connection_info`, where socket2 reads
    no listen state; on the three BSDs, `getsockopt(SO_ACCEPTCONN)` likewise.
  - `crates/ulo-cli/src/commands/dev.rs`: `ulo dev --listen` placing the held socket at descriptor 3
    between fork and exec, through `pre_exec`, `dup2` and `fcntl`.
  - `crates/ulo-http-hyper/tests/listeners.rs`: the socket-activation test placing its child's
    descriptor 3 the same way.

  Each already carried its safety comments, which are unchanged. The opt-out heads each file with a
  line naming the list and what has no safe API. Moving the two `pre_exec` uses onto the
  `command-fds` crate, which wraps the same call, would empty two entries by moving the `unsafe` into
  a dependency the job does not see; that is not done. See S4.
- **The `miri` job** pins `nightly-2026-10-11`, the nightly installed for the local run, as
  `macro diagnostics` and `generated code lints` pin theirs, and that job's comment now counts three
  pins. It runs the check before the cache, then `scripts/unsafe_allowed.py miri`, which derives one
  `cargo miri test --locked` per entry from `cargo metadata`: a module of a library or binary target
  as that target's tests filtered by the module's path, `activation::` under `--lib` and
  `commands::dev::` under `--bin ulo`; a file that is a test target as the whole target,
  `--test listeners`. With nothing listed it prints that Miri has nothing to run and passes.
- **What Miri can run.** Run on each listed module's tests (decision 4), each stops at its first
  system call: `socket`, which Miri refuses under isolation, or `kqueue` for the two port-0 tests on
  macOS. Each such test carries `#[cfg_attr(miri, ignore = ..)]` naming what Miri refused, so the job
  passes and reports them ignored. `dev.rs` has no tests. Miri does check pure-Rust `unsafe`: listed
  for a probe, `FuturesIo`'s two calls as at `4b8a19e2` passed a Miri run of a read test, and with the
  zeroing removed and the cursor advanced one byte past each read, Miri reported reading
  uninitialised memory where the test read that byte (decision 4). See S5 and F388.

### 4. The local Miri run

`rustup toolchain install nightly --profile minimal --component miri,rust-src` installed
`1.101.0-nightly (6f5da406b 2026-10-10)`, Miri `0.1.0 (6f5da406b5 2026-10-10)`, which is
`nightly-2026-10-11`; no permission check refused it.

- **The job's two steps,** run locally with `MIRI_TOOLCHAIN` unset, so on `nightly`
  (`miri/job-check.txt`, `miri/job-miri.txt`): the check scanned 454 files, 3 listed, no finding; Miri ran three commands,
  `ulo-net --lib -- activation::` (2 tests, both ignored), `ulo-cli --bin ulo -- commands::dev::` (0
  tests) and `ulo-http-hyper --test listeners` (5 tests, all ignored, `child_serves_an_inherited_socket`
  by its own `ignore`), each exiting 0.
- **Before the ignores** (`miri/first-*.txt`): the activation run aborted at `socket`, "not available
  when isolation is enabled"; with `MIRIFLAGS=-Zmiri-disable-isolation` it got past `socket`, `bind`,
  `listen`, `borrow_raw` and `FD_CLOEXEC` and aborted at `getsockopt(SO_TYPE)`, "option 0x1008 is
  unsupported for level SOL_SOCKET". The listener run aborted at its first test's `socket`. Since
  Miri ends the binary at the first refusal, each ignored test was then run alone with
  `--include-ignored --exact` (`miri/each/`): the four `socket` refusals and, for the two port-0
  tests, `kqueue`, "can't call foreign function `kqueue` on OS `macos`". Each `ignore` names what
  that run showed.
- **The Linux runner** was not reproduced: `--target x86_64-unknown-linux-gnu` failed building
  ring's C code for that target (`miri/first-ulo-net-linux.txt`). On Linux the tokio and smol
  pollers are epoll, which Miri emulates, so the port-0 tests would stop at `socket` there; the
  `ignore` text says both.
- **`FuturesIo` under Miri, two probes** in `broken/`: `p01` restored its two `unsafe` calls as at
  `4b8a19e2`, listed `rt.rs` and opted it out, and added a test reading 20,000 bytes through a 64 KiB
  hyper buffer: the check passed with four listed, the job's command list gained
  `ulo-hyper-serve --lib -- rt::`, and Miri passed the test in 46 s. `p03`, run in a scratch copy of
  the tree because the verification runs held the real one, removed the zeroing and advanced the
  cursor one byte past each read, with the test summing every byte it was handed: the plain run
  failed on the length, and Miri stopped at "reading memory at alloc636966[0x4e20..0x4e21], but memory
  is uninitialized", byte 20,000, the one past the read. `p02`, the same plant without the sum, failed
  its length assertion under both before any byte was read, so it shows nothing about Miri; it is
  kept beside the others.

## The tests

- **S1, `ulo-rpc`'s client unit tests,** over a link whose server never answers and which records
  each opening frame's `deadline-ms`:
  `a_stream_sends_its_deadline_and_neither_its_idle_timeout_nor_the_client_s_timeout` (a request on a
  client timing out at 300 ms sends `300`; a stream's and a duplex's deadlines send `2000` and `1500`;
  a stream given only an idle timeout, a stream given neither and a duplex given neither send none);
  `a_silent_stream_ends_at_its_idle_timeout`; `a_stream_ends_at_its_deadline_before_a_longer_idle_timeout`;
  and `a_stream_with_neither_bound_outlives_the_client_s_timeout` (still waiting after ten times the
  client's 50 ms). Every wait for an item is bounded at five seconds, so a break fails a test rather
  than hanging the binary.
- **S1, the `compile_fail` doctest** on `StreamCall`, `client.stream::<_, u64>(..).timeout(..)` with
  E0599, beside a doctest that compiles `.idle_timeout(..).deadline(..)`.
- **S1, the RPC suite's four stream-bound scenarios,** on every link, each answering by refusal at
  startup where the link carries no streamed shape, as `cancel_mid_stream` does, and on a new
  `conformance.pause` handler that yields `0`, falls silent for the milliseconds it is sent, yields
  `1` and ends: `a_stream_s_idle_timeout_is_per_frame` (renamed; ticks 20 ms apart read for 2.5 s under
  a one-second idle timeout); `a_stream_s_idle_timeout_ends_it_on_silence` (a four-second silence after
  the first item under a one-second idle timeout ends with `Timeout`, no sooner than the second and
  before the silence ends, and the stream then yields nothing); `a_stream_s_deadline_is_the_server_s_deadline`
  (a stream of ticks given a 600 ms deadline, through the `MutedCancel` link, ends with `Timeout` after
  at least one tick, and the server's ticking stream is ended with `Deadline` within the deadline and
  the broker's settle budget); `a_stream_with_neither_bound_runs_past_both` (on a clone of the client
  timing out at 300 ms, the paused stream yields both items across its four-second silence and ends
  cleanly).
- **S4, `ulo-hyper-serve`'s `rt.rs` unit tests:**
  `every_byte_reaches_hyper_s_buffer_in_order_at_most_64_kib_a_read` (300,000 bytes read through
  hyper buffers of 100 bytes, 8 KiB and 256 KiB in turn) and `a_pending_read_releases_the_buffer`.
  The HTTP and WebSocket suites on smol hosts run every connection through it.
- **S4, the check** and the compiler, through the breaks below.

### Before and after

Each break went through `batch27/scripts/brk.py`, batch 26's script with `scripts` and
`unsafe-allowed.txt` added to its hash and the command run under bash: it replaces each span
(asserting it occurs once), runs the target, writes every touched file and `Cargo.lock` back byte for
byte, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`,
`scripts` and `unsafe-allowed.txt` and every untracked file under `crates` and `scripts` and the list
itself. Specs in `batch27/specs/`, output in `batch27/broken/` with `summary.txt`.

Every restore reported `ok`, the hash `c8389529..` for every break below. A first round ran on
`0d8ef918..`, before the unit tests' waits were bounded: its first three breaks are kept under
`broken/first-layout/`, and its fourth, whose unbounded unit test hung until it was killed, under
`broken/superseded/`.

| Break | Target | Result |
| --- | --- | --- |
| a stream's deadline not sent (`Whole::Stream` treated as `Publish`) | the client's unit tests; TCP's `a_stream_s_deadline_is_the_server_s_deadline` and `a_stream_with_neither_bound_runs_past_both` | the stamping test failed, `[("req", Some("300")), ("req", None), ("open", None), ..]`; the deadline scenario failed, "the ticking stream was still running 1.70s after a stream with a 600 ms deadline opened"; the neither scenario passed |
| the client's timeout as a stream's idle timeout, its meaning before | the unit tests; TCP's neither and idle scenarios | `a_stream_with_neither_bound_outlives_the_client_s_timeout` failed and the neither scenario failed "301.67ms after it opened", each with "no reply frame arrived within the stream's idle timeout"; the idle scenario passed |
| the client's timeout as a stream's deadline | the unit tests; TCP's neither scenario | the outlives test and the stamping test failed, the second reading `Some("300")` on all three streams given neither; the scenario failed "302.62ms after it opened: .. the stream's deadline passed" |
| a stream's idle timeout ignored | the unit tests; TCP's `a_stream_s_idle_timeout_ends_it_on_silence` | `a_silent_stream_ends_at_its_idle_timeout` failed, "the idle timeout did not happen within 5s"; the scenario failed, "expected an error of kind Timeout, got: Ok(1)", the second item after the four-second silence |
| a stream's idle timeout bounding the whole call | the unit tests; TCP's `a_stream_s_idle_timeout_is_per_frame` | the idle unit test failed, its stream ended by "the stream's deadline passed"; the scenario failed "1.001s after it opened, its idle timeout one second" |
| a stream's deadline sent but not kept by the client | the unit tests | `a_stream_ends_at_its_deadline_before_a_longer_idle_timeout` failed, "the deadline did not happen within 5s" |
| `StreamCall::timeout` restored | `ulo-rpc`'s doctests | the `compile_fail` doctest failed, "Test compiled successfully, but it's marked `compile_fail`" |
| an `unsafe` block planted in `ulo-rpc`'s `held.rs` | the check; `cargo check -p ulo-rpc` | the check failed, "crates/ulo-rpc/src/held.rs:15: `unsafe` outside the modules unsafe-allowed.txt lists"; the build failed, "usage of an `unsafe` block" |
| the same with `#![allow(unsafe_code)]` heading `held.rs` | the same | the check failed with two findings, the `unsafe` at line 17 and "`unsafe_code` relaxed" at line 11; the build passed, which is what the check is for |
| the same block with `ulo-rpc`'s `[lints] workspace = true` removed | the same | the check failed, "crates/ulo-rpc/Cargo.toml: no `[lints] workspace = true`, so the workspace's `unsafe_code` level does not reach `ulo-rpc`", and the `unsafe`; the build passed |
| `activation.rs`'s opt-out removed | the check; `cargo check -p ulo-net` | the check failed, "listed, but does not opt out with `#![allow(unsafe_code)]`"; the build failed at the module's four `unsafe` sites compiled on macOS |
| the workspace's `unsafe_code` set to `allow`, the block planted | the check; `cargo check -p ulo-rpc` | the check failed, "`[workspace.lints.rust]` does not set `unsafe_code` to \"deny\" or \"forbid\"", and the `unsafe`; the build passed |
| a listed path that is no file | the check | failed, "unsafe-allowed.txt:18: `crates/ulo-rpc/src/gone.rs` is no file" |
| `FuturesIo` copying one byte short of each read | `ulo-hyper-serve`'s unit tests; `ulo-http-hyper`'s smol suite | both `rt.rs` tests failed, the first "299986 of 300000"; 34 of the suite's 36 scenarios failed |
| `FuturesIo` keeping its buffer while pending | `ulo-hyper-serve`'s unit tests | `a_pending_read_releases_the_buffer` failed, "an idle connection held its read buffer", capacity 8 |
| the restored tree | every target above | every test passed, the check reported no finding, and every build passed |

## Left for the transports DESIGN fold

Line numbers are those read on `6b78ef1c`.

- §5.4's client paragraph (line 1093): the builders carry `.header`, `.within` and, by shape,
  `.timeout(Duration)` on `request` and `send_stream`, the whole call, sent as `deadline-ms`, or
  `.idle_timeout(Duration)` and `.deadline(Duration)` on `stream` and `duplex`, the first the longest
  silence between reply frames and not sent, the second the whole call and sent; neither applies
  unless set. The module's `timeout` is the default for `request` and `send_stream` and the bound on
  `emit`'s publish, and does not reach a stream. The example at line 1077 is unchanged.
- Decision 106 (line 1604): a stream's whole-call bound is its `.deadline(..)`, stamped by the same
  rule; its `.idle_timeout(..)` is not stamped; the per-frame `.timeout(d)` it describes is gone.
  Decision 61 (line 1559) and X29 (line 1391): what `RpcClient::timeout` covers, as above.
- §5.2's conformance paragraph (line 959) and its list: the renamed scenario, the three new ones and
  the `conformance.pause` handler.
- §3.7's `FuturesIo` (line 595) and decision 93 (line 1591): hyper's `Read` over `futures-io` copies
  from a buffer of its own, at most 64 KiB, released while the stream is idle; no `unsafe` is left in
  `fw-hyper-serve`.
- §0's principles or §8: `unsafe_code` is denied workspace-wide, every member inheriting the
  workspace's lints; a module opts out only with `#![allow(unsafe_code)]` and an entry in
  `unsafe-allowed.txt`, which the `miri` job's check holds and whose modules' tests it runs under
  Miri; the three entries and why each has no safe API.
- The X table after X37 (line 1399): `StreamCall::idle_timeout`, `StreamCall::deadline`, and the
  removal of `StreamCall::timeout`.

## Needs sign-off

Numbered from S1 for this batch.

### S1. A stream has no idle timeout unless the call sets one

Decision 1. The client's timeout reaches `request`, `send_stream` and `emit` and no stream, so a
stream to a server that goes silent without closing waits until the stream is dropped, the calling
execution is cancelled, or the link reports the loss. The alternative is a client-wide default idle
timeout, an `.idle_timeout(Bound)` on `RpcClient` and `RpcClientModule` beside `.timeout(Bound)`,
five seconds at `Default` as the per-frame wait was, which keeps today's protection against a silent
server at the cost of a second module setting.

### S2. The idle timeout counts the silence before the first frame, connect included

Decision 1. A server that never answers a stream's opening frame is held to the same bound as one
that falls silent later, and a link whose connect hangs is too. The alternative is counting only
between reply frames, which leaves the connect and the first answer bounded by the deadline alone.

### S3. `FuturesIo` keeps a buffer of up to 64 KiB while a stream is readable

Decision 2. With reads ready back to back the copy costs less than the zeroing it replaces, or about
the same, except a full 64 KiB read into a 64 KiB buffer, 1.2 times; with a pending poll between
reads, where the buffer is released and zeroed again, it costs less on small reads and into a large
region and up to 1.8 times more on 8 to 64 KiB reads into a region of the same size. The buffer is
released whenever the read is pending. The alternatives are
an 8 KiB stack buffer per poll, as the first-read count uses, which holds nothing between polls and
caps every read at 8 KiB, and keeping the two `unsafe` calls listed and under Miri, which a probe
shows Miri can check.

### S4. Three modules are listed, the two `pre_exec` uses among them

Decision 3. Each has no safe API in `std` or the crates already depended on. The alternative for
`dev.rs` and `listeners.rs` is `command-fds`, which offers descriptor placement as a safe API over the
same `pre_exec`, leaving `activation.rs` alone on the list and the `unsafe` in a dependency.

### S5. The `miri` job runs, and Miri checks none of the listed `unsafe`

Decision 3 and F388. Every listed module's tests reach a system call Miri refuses, and are ignored
under Miri, so today the job's value is the check and the Miri step is
in place for a pure-Rust module listed later. The alternatives are a sanitizer job over the same
tests, AddressSanitizer on the same nightly, which runs their system calls natively, or tests
written to reach the listed `unsafe` without a socket, which `activation.rs`'s `vet` cannot do as
written.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch27/`: `suites/` for the
RPC suites (with `summary.txt` and the container lists after each broker), `verify/` for the hosts,
checks, workspace test, docs and clippy, `tree/`, `miri/` for the local Miri runs, `broken/` and
`specs/` for the breaks and probes, `probes/` for the read benchmark's output and `bench/` for its
source, `scripts/`, and `wt/`, the scratch copy `p03` ran in. Every run below ran on the tree hashed
`c8389529..` by `brk.py`'s measure (`verify/final-tree-hash.txt`), the hash every break restored to.
Two doc comments were reworded after them, `FuturesIo`'s and `StreamCall::deadline`'s, and nothing
else; on the result, hashed `db1f375e..`, the unit tests of `ulo-rpc` and `ulo-hyper-serve`, `ulo-rpc`'s
doctests, both crates' docs under `-D warnings`, their clippy with no location on a changed line, and
the check were run again and passed (`verify/after-comments/`).

- **The RPC suites, once each,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 35 passed and 3 ignored each (4.01 s, 4.01 s,
  4.02 s); the smol TCP link's three 35 and 3 each (4.01 s, 4.01 s, 4.02 s); UDP 32 and 6 (1.32 s).
  With `--features integration --test conformance` and the OpenSSL flags: NATS 37 and 1 (9.17 s);
  Redis 36 and 3 (9.83 s); MQTT 42 and 1 (9.02 s); Kafka 38 and 1 (45.80 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 37 and 1 (112.72 s). Every count is batch 26's plus the three new
  scenarios. Kafka took 30.72 s and RabbitMQ 72.86 s in batch 26; the new scenarios hold an
  environment for about 1 s, 1 s and 4 s, which does not account for RabbitMQ's 40 s, and the cause
  was not traced. Each ran once. The first suites' start overlapped the build of probe `p03` in the
  scratch copy.
- **The HTTP and WebSocket suites on a tokio and a smol host** (`verify/hosts-summary.txt`):
  `ulo-http-hyper`'s suite on tokio 36 and 2, on smol 36 and 2, its TLS tests 2 on each; the
  WebSocket suite in `ulo-ws-hyper`'s `conformance` 31 and `conformance_smol` 31, and in `ulo-ws`'s
  `conformance` 31, `conformance_smol` 31 and `conformance_pipe` 31; `ulo-hyper-serve` 2 unit, 3 in
  `rt.rs`, 4 in `serve.rs`; `ulo-listen-smol` 2; `listeners.rs` 4 and 1 ignored; `ulo-net` 2 unit and 3; `ulo-rpc`
  6 unit, 3, 9 and 5 in its test targets, and its doctests 1 and 5 ignored, then the `compile_fail`
  one.
- `cargo check --workspace --all-targets --all-features --locked` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`verify/check-*.txt`, by batch 25's `diag.py`). The workspace lint changes every member's flags, so
  both rebuilt every member.
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`: exit 0, the same 17 and no other (`verify/check-1.88.txt`);
  `[workspace.lints]` and `[lints] workspace = true` are Cargo 1.74's.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 1,099 passed, 0 failed, 138
  ignored across 197 test results (`verify/workspace-test.txt`, `workspace-test-count.txt`). Batch
  26's 1,070 plus 29: the four client unit tests, the two `StreamCall` doctests, of which the
  `compile_fail` one is reported as a result of its own, the two `rt.rs` unit tests, and three
  scenarios on each of the seven hermetic RPC suites. The `cfg_attr(miri, ignore)` attributes do not
  apply outside Miri, so the ignored count is unchanged.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`tree/summary.txt`):
  nothing to print, or "did not match any packages", for `ulo`, `ulo-transport`, `ulo-net`,
  `ulo-http`, `ulo-rpc`, `ulo-ws`, `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`,
  `ulo-runtime-conformance`, `ulo-smol`, `ulo-rpc-tcp-core`, `ulo-rpc-tcp-smol`, and
  `ulo-rpc-conformance` at its defaults and with `smol`. The controls print tokio:
  `ulo-rpc-conformance` with `tokio`, and `ulo-rpc-tcp`. `ulo-hyper-serve` and `ulo-listen-smol`
  print tokio through hyper, as F383 records of every hyper-based crate; `FuturesIo`'s change adds no
  dependency.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`,
  `ulo-rpc-conformance` (at its defaults and with `tokio,smol`), `ulo-hyper-serve`, `ulo-net` and
  `ulo-http-hyper`: each exits 0 (`verify/doc-*.txt`). `ulo-cli` has no library.
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-conformance -p ulo-hyper-serve -p ulo-net -p ulo-cli
  -p ulo-http-hyper --all-targets --all-features --no-deps --locked`, the crates whose source changed,
  with the OpenSSL flags: exit 0, 26 warning locations outside `crates/ulo/src`, none on a changed line
  or in a new file, by batch 25's `changed_lines.py`, which reported two planted locations, one on a
  changed line of `client.rs` and one in the new script, and not an unchanged one
  (`verify/clippy-touched.txt`, `clippy-changed.txt`, `checker-selftest.txt`). The one clippy found
  on a new line during the build, a complex type in the new test link, is a type alias now.
  `cargo +1.98.1 clippy -p ulo-macro-lints --all-targets --no-deps --locked -- -D warnings`: exit 0.
- **The unsafe check** on the final tree: 454 files scanned, 3 listed, no finding
  (`miri/job-check.txt`), and its self-test (`verify/check-selftest.txt`) as decision 3 describes.
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, running before the
  broker suites and after each, beside the same eight exited containers, and no other
  (`runs/docker-before.txt`, `suites/docker-after-*.txt`, `verify/docker-after.txt`). Every
  container a suite started was removed by it. No permission check refused an action, the nightly
  install included.
- **Disk:** 48 GB free at the start; 35 GB at the end, after the nightly toolchain, Miri's builds
  under `target/miri` and the scratch copy's, and the docs and clippy builds in `target-b26doc`.
