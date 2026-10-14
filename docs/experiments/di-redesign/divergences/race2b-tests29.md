# Divergences: race 2b, tests batch 29, `FuturesIo`'s two `unsafe` calls restored in a module of their own and run under Miri (S3), each listed entry marked `pure-rust` or `syscall` and the Miri step failing when a pure-Rust entry has no test Miri runs (S5, F388), both `pre_exec` placements moved onto `command-fds` once it passed tests of both descriptor cases (S4), a client-wide stream idle timeout (S1), and the deadline scenarios scaled from the suite's `Budget`

The fifty-fourth response, signed off 2026-10-12 with a correction to S4's premise, answers
`race2b-tests27.md`. `FuturesIo`'s hyper `Read` again zeroes hyper's unfilled buffer in place and
advances past what was read, as at `4b8a19e2`, in `crates/ulo-hyper-serve/src/compat.rs`, a module
holding `FuturesIo` alone; each `unsafe` block carries the invariant that makes it sound, and the
reader's count is checked against the slice before `advance`. The module is listed `pure-rust`,
and its four tests use no socket, so Miri runs them. Every entry in `unsafe-allowed.txt` now names
its kind; `scripts/unsafe_allowed.py check` validates the kind, and `miri` runs Miri over each
pure-Rust entry, fails when Miri executes none of its tests, and prints each `syscall` entry it
skips with the reason. Tests of the equal-descriptor case and the ordinary case were written for
`ulo dev`'s placement and the socket-activation test's child, run against the `pre_exec` code, and
then against `command-fds` 0.3.3, which passed them; both entries left the list, which now holds
`compat.rs` and `activation.rs`. `RpcClient` and `RpcClientModule` gain `.idle_timeout(Bound)`,
none by default, which a stream's own `.idle_timeout(d)` replaces. The held-cancel and
stream-deadline scenarios take their bounds from the broker's `Budget::settle`.

Files changed: for S3, `crates/ulo-hyper-serve/src/{lib.rs, rt.rs}` and a new
`crates/ulo-hyper-serve/src/compat.rs`; for S5, `unsafe-allowed.txt`, `scripts/unsafe_allowed.py`
and `.github/workflows/ci.yml`; for S4, `crates/ulo-cli/src/commands/dev.rs`,
`crates/ulo-cli/Cargo.toml`, `crates/ulo-http-hyper/tests/listeners.rs`,
`crates/ulo-http-hyper/Cargo.toml`, `Cargo.toml` (the workspace dependency) and `Cargo.lock`; for
S1, `crates/ulo-rpc/src/{client.rs, client_module.rs}` and `crates/ulo-rpc/tests/runtime.rs`; for
the timing, `crates/ulo-rpc-conformance/src/cases/{deadlines.rs, order.rs}`; in the workspace's
`FRAMEWORK_GAPS.md`, a note on F388 and F389. The batch started from `4840c03f`, and no other
commit landed during it.

## The signatures

```rust
// ulo_rpc::RpcClient
pub fn idle_timeout(self, idle: Bound) -> Result<Self, ZeroTimeout>;   // new: every stream's idle timeout unless the call sets one

// ulo_rpc::RpcClientModule<L>
pub fn idle_timeout(self, idle: Bound) -> Self;                         // new: the same for the module's client

// ulo_hyper_serve::FuturesIo<T>, now defined in `compat.rs` and re-exported where it was
pub fn new(inner: T) -> FuturesIo<T>;                                    // unchanged; the struct loses its private buffer
```

```text
# unsafe-allowed.txt: each entry `<path> <kind>`
crates/ulo-hyper-serve/src/compat.rs pure-rust
crates/ulo-net/src/activation.rs syscall <what Miri stops at>
```

```toml
# Cargo.toml, [workspace.dependencies]
command-fds = "0.3.3"
# crates/ulo-cli: `dev` enables `dep:command-fds` in place of `dep:libc`, a unix dependency
# crates/ulo-http-hyper: dev-dependency `command-fds` in place of `libc`
```

Unchanged: `StreamCall::idle_timeout`, `StreamCall::deadline`, `RpcClient::timeout`,
`RpcClientModule::timeout`, `ZeroTimeout`'s public API and its message for a timeout, every other
client signature, `FuturesIo`'s public API, every scenario's name.

What changes in meaning without a signature: a `stream` or `duplex` call that sets no idle timeout
takes its client's, where it had none; a client's is none unless `.idle_timeout(Bound::After(d))`
set it, so nothing changes for code that does not call it. `ZeroTimeout`'s message names what the
zero would end, "every call" for a timeout and "every stream" for an idle timeout. `ulo dev` hands
each child a duplicate of the held socket, which the spawned command owns and closes after the
spawn, where the `pre_exec` closure placed the held socket's own descriptor; a duplicate that cannot
be made is reported in yellow and the child starts without the socket. The held-cancel and
stream-deadline scenarios hold a broker for less time at the default budget and more on Kafka's.
`FuturesIo` holds no buffer of its own, and each read zeroes hyper's whole unfilled region again,
pending reads included, as at `4b8a19e2`.

Dependencies: `command-fds` 0.3.3 added, which brings no new package beyond itself (`nix` 0.31.3
and `thiserror` 2.0.21 are in the lock already); `libc` leaves `ulo-cli` and `ulo-http-hyper`.

## Decisions

### 1. S3: `FuturesIo`'s `unsafe` read path, restored in a module of its own

- **The module.** `rt.rs` held hyper's executor and timer beside `FuturesIo`, so `FuturesIo`
  moved to `compat.rs`, which holds it alone, and the listed scope is that type. `lib.rs` re-exports
  it where it was, `ulo_hyper_serve::FuturesIo`. The name is the one tokio-util gives its adaptor
  between the two trait families.
- **The code.** The read path is `4b8a19e2`'s: `ReadBufCursor::as_mut`, each byte written `0`, the
  slice cast to `&mut [u8]` for the `futures-io` read, and `advance` by what was read. Batch 27's
  buffer and its 64 KiB cap are gone, so a connection holds nothing of its own. The opt-out heads
  the module with a line naming the list and the two `unsafe` calls.
- **The safety arguments.** Above the first block: `as_mut` requires that no byte that may be
  initialized is uninitialized, and the loop only writes; the cast keeps length and layout, which
  `MaybeUninit<u8>` and `u8` share, and is valid because every element was just initialized; the
  reader receives `&mut [u8]`, through which safe code cannot uninitialize a byte. Above `advance`:
  it requires the first `n` unfilled bytes to be initialized, the block above initialized all of
  them, and the count was checked against the slice.
- **One line `4b8a19e2` did not have.** `AsyncRead` is a safe trait, so a reader that reports more
  bytes than its slice holds is a bug in safe code that `advance` must not trust: `assert!(read <=
  offered, ..)` before it. At `4b8a19e2` such a count reached `advance`, which only adds it to
  hyper's filled cursor; hyper then panicked slicing its own buffer, in the break below, rather than
  reading past it, but the argument for `advance` rested on the reader. See S1 and F389.

### 2. S5 and F388: a kind per entry, and a Miri step that counts

- **The list's grammar.** `<path> <kind>`, the kind `pure-rust`, which takes nothing after it, or
  `syscall <reason>`, the reason the rest of the line and required. The header states both. `check`
  reports an entry with no kind, an unknown kind, a `syscall` entry with no reason, a `pure-rust`
  entry with one, and a path listed twice, each with the line number; `miri` refuses to run on any
  of them (break k01's `--dry-run`).
- **What `miri` does.** It prints each `syscall` entry, "skipped, a system call Miri cannot
  execute: <path>: <reason>", then runs one `cargo +<toolchain> miri test --locked` per pure-Rust
  entry as batch 27's derivation built it, a module's tests filtered by its path, `compat::` under
  `--lib`. It streams Miri's output and counts libtest's lines `test <name> ... ok` and `FAILED`,
  `- should panic` included, whose name starts with the module's path, or every one for a test
  target; an ignored test is not counted. It reports "Miri executed N of its tests" per entry and
  fails on a non-zero exit or on N = 0, "Miri executed none of its tests; a pure-Rust entry needs at
  least one that runs under Miri". With no pure-Rust entry listed it prints that Miri has nothing to
  run and passes; the list has one.
- **The tests Miri runs.** Four in `compat.rs`, over a `futures-io` source that fills at most a
  chunk a read and is pending on every other poll, into hyper `ReadBuf`s over fresh
  `MaybeUninit::uninit()` memory, each reading every byte hyper is told was filled as a value:
  `every_byte_reaches_hyper_s_buffer_in_order` (5,000 bytes in reads of at most 700 into buffers of
  100, 1,024 and 4,096 bytes); `a_read_lands_after_what_hyper_s_buffer_holds` (a 48-byte buffer
  filled 10 bytes a read, each read leaving what was there);
  `the_reader_may_read_what_it_is_offered` (a source that sums every byte of the slice before
  writing, as `&mut [u8]` lets any reader do); and `a_reader_counting_past_its_slice_panics`
  (`should_panic` on the new check). The third is the one a missing zeroing fails under Miri while
  the plain run passes; the first two read the byte batch 27's `p03` advanced onto.
- **The `syscall` entries.** `activation.rs`, whose reason is batch 27's finding: its tests open a
  socket, which Miri refuses under isolation, and without isolation Miri stops at
  `getsockopt(SO_TYPE)`. Its two tests keep their `cfg_attr(miri, ignore)`. The listener test's
  attributes stay as well, the file no longer listed.
- **The CI job** is the same two steps on the same pin; the comments say what each step now fails
  on.

### 3. S4: the tests first, then `command-fds`

- **A placement that can be tested.** Each site's placement became a function,
  `place_fd(command: &mut std::process::Command, source: OwnedFd, target: RawFd)`, the `pre_exec`
  code unchanged inside it but for the target, which was the constant 3: `ulo dev` calls it with
  `LISTEN_FD`, through `tokio::process::Command::as_std_mut`, and the socket-activation test's
  `spawn_child` with 3. The command owns `source` until it is dropped, which is what
  `command-fds`'s `FdMapping` requires, so the tests are the same text before and after the switch.
  `ulo dev` therefore hands each spawn a `try_clone` of the held socket; see S6.
- **The tests,** in `dev.rs`'s new `tests` module and in `listeners.rs`, each pair run in a child,
  the test binary started again as an ignored `placement_child` test that reads `/dev/fd/<n>`'s
  metadata and reports whether descriptor `n` is a socket with the inode the parent read through
  its own `/dev/fd`: the ordinary case, a socket at a descriptor other than 3 placed at 3; the
  equal case, a socket placed at the descriptor it already is in the parent, where std's
  `FD_CLOEXEC` stays set unless the placement clears it; and a control, a duplicate held open in
  the parent across the spawn and not placed, which the child must not find. No unsafe is in the
  tests, so they need no listing.
- **Against the `pre_exec` code** the six passed. Making the equal branch `dup2` onto itself failed
  the equal test at both sites, the child finding the socket's descriptor closed, "Bad file
  descriptor (os error 9)", and removing the placement failed both placement tests at both sites;
  the controls passed throughout (breaks p01–p04).
- **`command-fds`.** 0.3.3, released 2026-04-10 from `google/command-fds`, copyright the Android
  Open Source Project, Apache-2.0, nine releases since 2021 and commits to 2026-09-29; it declares
  no `rust-version`, is edition 2024, and depends on `nix` 0.31 and `thiserror` 2, both in the lock,
  and on tokio only behind a `tokio` feature not enabled here. Its `map_fds`, which runs in the
  `pre_exec` hook, clears `FD_CLOEXEC` with `fcntl(F_SETFD)` when a mapping's parent descriptor is
  its child descriptor, and moves a parent descriptor that collides with another mapping's target to
  a temporary first. cargo's resolver locked it as "the latest Rust 1.88 compatible version", and
  `cargo +1.88 check` builds both crates (Verification).
- **Against `command-fds`** the same six passed; mapping to the target plus one failed `dev.rs`'s
  two placement tests and dropping the mapping failed `listeners.rs`'s (breaks c01, c02). Both
  entries left the list and both files their opt-out; `unsafe_allowed.py check` scans them as
  unlisted files and finds no `unsafe`. `dev.rs`'s `place_fd` maps with `.expect(..)`: one mapping
  cannot collide with another, the only error `fd_mappings` returns.

### 4. S1: a client-wide idle timeout, none by default

- **Where it lives.** `RpcClient` holds `idle: Option<Duration>` on the handle beside `timeout`,
  so `.idle_timeout(..)` on one clone leaves the others' as they were, as `.timeout(..)` does.
  `RpcClientModule` stores a `Bound` and passes it to `RpcClient::of_module`, which now takes the
  timeout and the idle timeout; `RpcClient::new` gives `Bound::Default` for both.
- **What `Bound` means here.** `After(d)` is `d`; `Default` and `Unbounded` are both none, since
  the response sets the default to no idle timeout. `After(Duration::ZERO)` would end every stream
  before its first frame and is refused as `ZeroTimeout`, by `RpcClient::idle_timeout` where it is
  written and by the module when the app wires, as the timeout's zero is; a module with both zero
  reports the timeout. `ZeroTimeout` gains a private field naming what the zero ends, so the
  timeout's message is unchanged and the idle timeout's says "every stream". See S3.
- **The override.** `StreamCall::into_future` takes `idle.or(client.idle)`: the call's own
  `.idle_timeout(d)` replaces the client's, longer or shorter, and the client's applies when the
  call sets none. A call cannot turn the client's off but by a longer one; see S4.
- **The sentence.** `RpcClient::idle_timeout`'s doc comment says that a lost connection fails a
  waiting stream `Unavailable` through the reply lane whatever the idle timeout, so the idle
  timeout catches only a server that is still connected and has gone silent; the module's says it
  in a clause. `RpcClient`'s type doc and `StreamCall`'s say where a stream's idle timeout comes
  from.

### 5. The deadline scenarios, from `Budget::settle`

- **What scales.** Each bound is a multiple of the broker's `Budget::settle`, the time a one-way
  effect takes to show, 500 ms at the default and 3 s on Kafka. Stream-deadline scenarios
  (`deadlines.rs`): the idle timeout is `settle` and the per-frame scenario reads for three of
  them, where they were 1 s and 2.5 s; the silence between the paused stream's items is four idle
  timeouts, 2 s at the default, where it was 4 s, in both the silence and the neither-bound
  scenarios; the neither-bound client's timeout is half an idle timeout, 250 ms, where it was
  300 ms; a stream's deadline is `settle` and a fifth, 600 ms at the default as before. Held-cancel
  scenarios (`order.rs`): the short hold and the late `opened`'s delay are half of `settle`, 250 ms,
  where they were 300 ms; the held call's deadline is `settle` and a fifth, 600 ms as before. Each
  scenario asserts the same order of events with the same margins expressed in `settle`.
- **Why those factors.** The held call's deadline must outlast the `open` reaching the server and
  its `opened` reaching the client, after which the scenario cancels and expects the `cancel`
  held, so it is not shortened below `settle`; the stream deadline must let a tick cross the
  broker. The silence has to exceed the idle timeout by more than the broker's latency, and
  `idle * 4` leaves three `settle`s.
- **Kafka** holds a broker longer: the two silences are 12 s and the per-frame read 9 s, where they
  were 4 s and 2.5 s, and the held call's deadline 3.6 s. See S5.

### 6. The benchmark and the local Miri runs

- **The benchmark,** batch 27's `p01` with two rows added that time the crate's own `FuturesIo`
  through `ulo-hyper-serve` as a path dependency (`bench/`, `probes/p01-*`). On a quiet host, load
  near 3, the crate's rows sit within run-to-run noise of batch 27's copy of the old shape in every
  case: 66 to 96 ns against 72 to 104 ns for 512 bytes into 8 KiB; 130 to 141 ns against 125 to 151
  ns for 8 KiB into 8 KiB; 0.78 to 0.80 µs against 0.82 to 1.0 µs for 64 KiB into 64 KiB; with a
  pending poll between reads, 1.01 to 1.04 µs against 1.00 to 1.04 µs at 64 KiB into 64 KiB, where
  batch 27's buffer, released on each pending read, took 1.83 to 1.96 µs; and 2.4 to 2.8 µs for 16
  KiB into a 400 KiB region, where the kept buffer took 0.29 to 0.30 µs. Batch 27's own binary, run
  beside them, gives its old-shape figures as it did then or lower: 72 ns, 122 ns, 0.86 µs and 1.01
  µs, against batch 27's 89, 158 to 174 ns, 1.0 to 1.1 µs and 1.44 to 1.51 µs. Two earlier runs, at
  load 8 to 10 with the host's indexer and a browser busy, read two to four times slower on every
  row alike and are kept as `run1` and `run2`.
- **The pinned nightly and a shared target directory.** `rustup toolchain install nightly-2026-10-11
  --profile minimal --component miri,rust-src` installed the CI pin beside `nightly`, which is the
  same build (`6f5da406b 2026-10-10`). The first install was stopped before it finished and was
  then completed, not removed. With `nightly` having built `target/miri` in batch 27, the pinned
  toolchain failed building `ring` there, "can't find crate for `libc`" and for `cfg_if`
  (`miri/first-pinned-ring-failure.txt`, `second-pinned-ring-failure.txt`); in a target directory
  of its own it built and ran the four tests (`miri/job-miri-pinned-own-target.txt`), and so did
  `nightly` in `target/miri` (`miri/job-miri-nightly.txt`). CI starts from an empty target
  directory with the pin alone. The breaks ran Miri as `nightly`.
- **A launch stall.** Each test binary built anew sat in `_dyld_start` for about 90 s before
  running, with `syspolicyd` taking CPU meanwhile (`broken/m02-plain-hang-sample.txt`); the cause
  is not traced, and `target/debug/deps` holds 1.26 million entries. A break harness reads the
  stall as a hang, and it is not one. The workspace test ran at about 85 s per binary.

## The tests

- **S3, `compat.rs`'s four unit tests,** decision 2, run plainly and under Miri; the HTTP and
  WebSocket suites on smol hosts serve every connection through `FuturesIo`.
- **S5, the Miri step and the check,** through the breaks below.
- **S4, the placement tests,** decision 3: `dev.rs`'s `a_socket_is_placed_at_descriptor_3`,
  `a_socket_already_at_its_target_survives_the_exec` and
  `without_a_placement_the_child_does_not_find_the_socket`, and in `listeners.rs`
  `the_child_s_socket_is_placed_at_descriptor_3`,
  `the_child_s_socket_already_at_its_target_survives_the_exec` and
  `without_a_placement_the_child_does_not_find_the_socket`, each pair's child the ignored
  `placement_child`. The socket-activation tests, which hand the child its socket through the same
  `place_fd`, pass on both versions.
- **S1, `ulo-rpc`'s client unit tests,** over the silent link and a new `SeveredLink` whose reply
  lane the test ends: `a_client_wide_idle_timeout_ends_a_silent_stream` (100 ms on the client, a
  stream and a duplex setting none end `Timeout`, "idle timeout");
  `a_call_s_idle_timeout_overrides_the_client_s` (a 30 s call on a 50 ms client still waiting after
  500 ms, and a 100 ms call on a 30 s client ending within 5 s);
  `the_default_idle_timeout_never_ends_a_quiet_stream` (a module's client at `Bound::Default` and a
  client from `RpcClient::new`, each stream still waiting after an hour on tokio's paused clock,
  where any timer the client armed would fire first); and
  `a_lost_connection_fails_a_stream_with_no_idle_timeout` (`Unavailable` once the lane ends, with
  `Bound::Unbounded`). In `tests/runtime.rs`, through an app:
  `a_module_s_idle_timeout_reaches_its_client_s_streams` (a stream on the module's client ends at
  exactly 250 ms of paused time at `After(250 ms)`, and has not ended after an hour at `Default`)
  and `a_zero_idle_timeout_is_refused_on_the_client_and_the_module` (both messages).
- **The timing,** the scaled scenarios themselves on every suite, and two of batch 27's breaks
  re-run against them on TCP.

### Before and after

Each break went through `batch29/scripts/brk.py`, batch 27's harness unchanged but for its paths:
it replaces each span (asserting it occurs once), runs the target under bash, writes every touched
file and `Cargo.lock` back byte for byte, and compares a hash over `git diff HEAD` of `crates`,
`Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every untracked file
under `crates` and `scripts`. Specs in `batch29/specs/`, output in `batch29/broken/` with
`summary.txt`. Every restore reported `ok`. The tree moved between groups of breaks, so the hash
differs by group: `a09cd852..` for the Miri and list breaks, before the CI comments; `e4d94fbd..`
for the `pre_exec` breaks, after them; `30628c03..` from the switch to `command-fds` on. The
verification ran on that tree with one doc comment reworded, `7e2f5ba8..`. Where a target is two
commands, the summary's exit is the second's, and the table reads each from its output.

| Break | Target | Result |
| --- | --- | --- |
| m01. every `compat.rs` test `cfg_attr(miri, ignore)` | `unsafe_allowed.py miri` | exit 1: the four reported "ignored, planted", "Miri executed 0 of its tests, exit 0", then "Miri executed none of its tests; a pure-Rust entry needs at least one that runs under Miri"; the three `syscall` entries printed as skipped |
| m02. the zeroing loop removed | `compat.rs`'s tests plainly, then `miri` | the plain run passed 4 of 4; Miri stopped in `the_reader_may_read_what_it_is_offered`, "reading memory at alloc413127[0x0..0x1], but memory is uninitialized", "Miri failed, exit 1" |
| m03. batch 27's `p03`: no zeroing, and `advance(read + 1)` for a read short of its slice | the same | the plain run failed 3 of 4 on content; Miri stopped in `a_read_lands_after_what_hyper_s_buffer_holds`, "memory is uninitialized at [0xa..0xb]", the byte past the first 10-byte read |
| m04. the reader's count unchecked | `compat.rs`'s tests | `a_reader_counting_past_its_slice_panics` failed: hyper panicked first, "range end index 17 out of range for slice of length 16", "panic did not contain expected string" |
| k01. kind `pure` | `check`; `miri --dry-run` | both exit 1, "unsafe-allowed.txt:17: `pure` is no kind; write `pure-rust` or `syscall`" |
| k02. no kind | `check` | exit 1, "has no kind; write `pure-rust`, or `syscall` and what Miri stops at" |
| k03. `syscall` with no reason | `check` | exit 1, "a `syscall` entry gives the reason Miri cannot execute it after the kind" |
| k04. `pure-rust` with a reason | `check` | exit 1, "a `pure-rust` entry takes no reason, since Miri runs its tests" |
| k05. a path listed twice | `check` | exit 1, "is listed again, first at line 17" |
| p01. `dev.rs`'s equal branch made `dup2` onto itself, on the `pre_exec` code | `dev.rs`'s placement tests | the equal test failed, the child reporting "descriptor 4 is closed: Bad file descriptor (os error 9)"; the ordinary test and the control passed |
| p02. `dev.rs` placing nothing | the same | both placement tests failed, descriptors 3 and 4 closed; the control passed |
| p03. `listeners.rs`'s equal branch made `dup2` onto itself | its placement tests | the equal test failed, "descriptor 6 is closed"; the others passed |
| p04. `listeners.rs` placing nothing | the same | both placement tests failed; the control passed |
| c01. `dev.rs` on `command-fds`, mapped to the target plus one | `dev.rs`'s placement tests | both placement tests failed, descriptors 3 and 6 closed; the control passed |
| c02. `listeners.rs` on `command-fds`, no mapping | its placement tests | both placement tests failed; the control passed (a first spec did not compile and is kept as `c02-first-spec-did-not-compile.txt`) |
| i01. a stream ignoring the client's idle timeout | `ulo-rpc`'s client tests; `tests/runtime.rs` | `a_client_wide_idle_timeout_ends_a_silent_stream` failed, "did not happen within 5s"; `a_module_s_idle_timeout_reaches_its_client_s_streams` failed, `None` where `Some(250ms)` was due |
| i02. the shorter of the call's and the client's idle timeout | the same | `a_call_s_idle_timeout_overrides_the_client_s` failed, "a stream given a 30 s idle timeout ended within 500 ms, at the client's 50 ms"; the runtime tests passed |
| i03. `Bound::Default` as five seconds | the same | `the_default_idle_timeout_never_ends_a_quiet_stream` failed, "a module's client's quiet stream ended within an hour"; the module test failed, `Some(5s)` where `None` was due |
| i04. a reply lane's end failing no waiting call | `a_lost_connection_fails_a_stream_with_no_idle_timeout` | failed, "the lost connection's failure did not happen within 5s" |
| i05. the module's idle timeout not passed to its client | `tests/runtime.rs` | the module test failed, `None` where `Some(250ms)` was due |
| i06. the module taking a zero idle timeout | the same | `a_zero_idle_timeout_is_refused_on_the_client_and_the_module` failed, "an `RpcClientModule` with a zero idle timeout wired" |
| i07. batch 27's "a stream's idle timeout ignored", on the scaled scenarios | TCP's silence and per-frame scenarios | the silence scenario failed, "expected an error of kind Timeout, got: Ok(1)", after a 2 s silence; the per-frame one passed |
| i08. batch 27's "a stream's idle timeout bounding the whole call", on the scaled scenarios | the same | the per-frame scenario failed "501.67ms after it opened, its idle timeout 500ms", "the stream's deadline passed"; the silence one passed |
| the restored tree | every target above | every test passed, the check reported no finding |

## Left for the transports DESIGN fold

Line numbers are those of `4840c03f`.

- §0's principle 7 (line 15): each entry in `unsafe-allowed.txt` names its kind, `pure-rust` or
  `syscall` with the reason; two are listed, `fw-hyper-serve`'s `compat.rs`, `FuturesIo`'s
  `ReadBufCursor::as_mut` and `advance`, pure Rust, and `fw-net`'s `activation.rs`, a system call;
  the two `pre_exec` placements are `command-fds`'s `fd_mappings`, so `fw-cli`'s `dev.rs` and
  `fw-http-hyper`'s `tests/listeners.rs` carry no `unsafe`; the `miri` job runs Miri over each
  pure-Rust entry's tests, fails when it executes none, and prints each system-call entry it skips
  with the reason.
- §3.7's `FuturesIo` line (line 596) and decision 112 (line 1616): hyper's `Read` over `futures-io`
  zeroes hyper's unfilled region in place and advances past what was read, the reader's count
  checked against its slice first; it holds no buffer; it lives in `compat.rs`, listed `pure-rust`,
  and its tests run under Miri. Decision 112's benchmark paragraph describes the copy, which is
  gone.
- Decision 113 (line 1617): the list's kinds; `command-fds` in place of the two `pre_exec`
  entries, which the decision records as not taken; the Miri step's count.
- §9 (line 1336): the development command places the held socket at descriptor 3 through
  `command-fds`, from a duplicate the spawned command owns, rather than through a `pre_exec` of its
  own.
- §5.4's client paragraph (lines 1094 and 1096) and decision 111 (line 1615): `.idle_timeout(Bound)`
  on `RpcClient` and `RpcClientModule`, none at `Default` and `Unbounded`, the default for every
  `stream` and `duplex` that sets none, which a call's `.idle_timeout(d)` replaces; decision 111
  records it as not built. A lost connection fails a waiting stream `Unavailable` whatever the idle
  timeout.
- §12's refusals table (line 1436) and decision 31 (line 1535): `RpcClientModule::idle_timeout(
  Bound::After(Duration::ZERO))` is refused at `wire()` beside the module's timeout, and
  `RpcClient::idle_timeout` with the same zero where it is written.
- The X table after X38 (line 1405): `RpcClient::idle_timeout(Bound)` and
  `RpcClientModule::idle_timeout(Bound)`.
- §5.2's conformance paragraphs (lines 960 and 964): the held-cancel and stream-deadline
  scenarios take their bounds from the broker's `Budget::settle`.

## Needs sign-off

Numbered from S1 for this batch.

### S1. `FuturesIo` checks a reader's count before `advance`

Decision 1. One `assert!` that `4b8a19e2`'s code did not have, so `advance`'s safety argument rests
on a check rather than on a safe trait's contract; a reader reporting more than its slice panics in
`FuturesIo`, naming both numbers. The alternative is the code exactly as it was, whose argument
assumed every `futures-io` reader honest.

### S2. An entry's kind is a word on its line, and a `syscall` entry's reason the rest of it

Decision 2. `pure-rust` takes nothing after it, so a pure-Rust entry's justification stays in the
comment above it, as before. The alternative is a reason on every entry, or the kind in the comment
block, which `check` would then have to parse out of prose.

### S3. An idle timeout of `Bound::Default` is none, and a zero one is refused

Decision 4. `Default` reads the response's default, no idle timeout, and `After(Duration::ZERO)`
fails as `ZeroTimeout`, "would time out every stream", where it is written or at `wire()`. The
alternative is to take a zero as given, a client whose streams all end before their first frame,
which nothing would report until the first stream.

### S4. A call cannot turn the client's idle timeout off

Decision 4. `StreamCall::idle_timeout` takes a `Duration`, so on a client with an idle timeout a
call can choose a longer or shorter one but not none. The alternative is `StreamCall::idle_timeout`
taking a `Bound`, `Unbounded` turning it off for the call, which changes a signature batch 27 set.

### S5. The deadline scenarios scale with `settle`, and container start is most of RabbitMQ's time

Decision 5 and Verification. At the default budget the scaling removes about 3.6 s of waiting
across the seven scenarios: 1 s from the per-frame read, 0.5 s from the silence scenario's idle
timeout, 2 s from the neither-bound silence, and 50 ms from each short hold. On the suites whose
scenarios share one broker that shows directly: TCP from 4.01 s to 2.01 s, NATS from 9.17 s to
4.88 s. Each RabbitMQ scenario starts a container of its own, and no scenario there took under 9 s
in six runs; the whole suite took 71.57, 91.20 and 140.66 s at `4840c03f` and 90.28, 106.26 and
92.37 s after, so this host cannot show 3.6 s of scenario time spread over six slots. Kafka, whose
bounds grow, took 37.56 s, against 45.80 s in batch 27. The alternative for Kafka is a cap,
`settle.min(..)`, which brings back a fixed number.

### S6. `ulo dev` hands each child a duplicate of the held socket

Decision 3. `command-fds` takes the parent descriptor by value, so each spawn gets a `try_clone`,
which the spawned command closes after the spawn. A duplicate that cannot be made, out of
descriptors, is reported in yellow and the child starts with `LISTEN_FDS=1` and nothing placed at
descriptor 3, which its activation refuses rather than adopting whatever is there; not probed. The
alternative is to fail the restart, which the spawn hook's callback, returning nothing, cannot do.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch29/`: `suites/` for the
RPC suites (with `summary.txt`, the container lists and the RabbitMQ binaries and per-scenario
runs), `verify/` for the hosts, checks, workspace test, docs and clippy, `miri/` for the Miri
runs, `broken/` and `specs/` for the breaks, `probes/` for the read benchmark and `command-fds`'s
release history, `bench/` for the benchmark's source, `scripts/`. Every run below ran on the tree
hashed `7e2f5ba8..` by `brk.py`'s measure (`verify/final-tree-hash.txt`), `30628c03..` with
`idle_of`'s doc comment reworded.
The scenarios' doc comments were reworded once more after the docs run below found private
intra-doc links in them; on that tree, hashed `be566e94..`, the docs, clippy, the unsafe check and
{{AFTER-FIX}} ran again and passed (`verify/after-doc-fix/`).

- **The RPC suites, once each,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 35 passed and 3 ignored each (2.01 s, 2.01 s,
  2.02 s); the smol TCP link's three 35 and 3 each (2.01 s, 2.01 s, 2.02 s); UDP 32 and 6 (1.31 s).
  With `--features integration --test conformance` and the OpenSSL flags: NATS 37 and 1 (4.88 s);
  Redis 36 and 3 (7.18 s); MQTT 42 and 1 (7.30 s); Kafka 38 and 1 (37.56 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 37 and 1 (90.28 s). Every count is batch 27's. Batch 27 took 4.01 s
  on each TCP suite, 9.17 s on NATS, 9.83 s on Redis, 9.02 s on MQTT and 45.80 s on Kafka.
- **RabbitMQ's time,** the suite's binary built at `4840c03f` before any edit and the one the run
  above built, each run directly with `-Zunstable-options --report-time` under
  `ULO_CONFORMANCE_PARALLEL=6` (`suites/rabbitmq-{before,after}-*.txt`, each with its container
  list): before, 120.00 s with three scenarios failing their boot budget, the IDE's workspace check
  having held the host at load averages of 17 to 20, then 71.57 s, 91.20 s and 140.66 s, all
  passing; after, 106.26 s and 92.37 s, beside the 90.28 s above. The two binaries alternated in the
  last four runs. No scenario took under 9.1 s in any run, the container's start; the seven scaled
  scenarios took 9.1 to 46.4 s before and 10.8 to 24.4 s after. CI is the reference for the suite's
  time; see S5.
- **The HTTP and WebSocket suites on a tokio and a smol host** (`verify/hosts-summary.txt`):
  `ulo-http-hyper`'s suite on tokio 36 and 2, on smol 36 and 2, its TLS tests 2 on each; the
  WebSocket suite in `ulo-ws-hyper`'s `conformance` 31 and `conformance_smol` 31, and in `ulo-ws`'s
  `conformance` 31, `conformance_smol` 31 and `conformance_pipe` 31; `ulo-hyper-serve` 4 unit in
  `compat.rs`, 3 in `rt.rs`, 4 in `serve.rs`; `ulo-listen-smol` 2; `listeners.rs` 7 and 2 ignored;
  `ulo-net` 2 unit and 3; `ulo-rpc` 10 unit, 3, 11 and 5 in its test targets, its doctests 1 and 5
  ignored, then the `compile_fail` one; `ulo-cli` 3 and 1 ignored.
- `cargo check --workspace --all-targets --all-features --locked` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`verify/check-*.txt`, by batch 25's `diag.py`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in a target directory of its own: exit 0, the same 17 and no
  other, `command-fds` 0.3.3 among the crates checked (`verify/check-1.88.txt`).
- {{WORKSPACE-TEST}}
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0,
  50 members, 54 entries, 33 checked, no failure and no error (`verify/runtime-levels.txt`);
  `ulo-cli` is tokio-bound and `command-fds` reaches `ulo-http-hyper` as a dev-dependency only.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: 455 files scanned, 2 listed,
  no finding; `dev.rs` and `listeners.rs` are scanned as unlisted files.
- **The Miri step** on the final tree: with `MIRI_TOOLCHAIN=nightly-2026-10-11` in a target
  directory of its own, `activation.rs` printed as skipped with its reason, then `cargo
  +nightly-2026-10-11 miri test --locked -p ulo-hyper-serve --lib -- compat::`, 4 passed in 27.14 s
  of Miri's clock, "Miri executed 4 of its tests, exit 0" (`miri/job-miri-final-pinned.txt`).
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`,
  `ulo-rpc-conformance` (at its defaults and with `tokio,smol`), `ulo-hyper-serve` and
  `ulo-http-hyper`. The first run failed `ulo-rpc-conformance` at both feature sets on eight
  links from public scenario docs to the private helpers `idle_of`, `silence_of`, `short_hold` and
  `call_deadline` (`verify/doc-*.txt`); with those links written as words each exits 0
  (`verify/after-doc-fix/doc-*.txt`). `ulo-cli` has no library.
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-conformance -p ulo-hyper-serve -p ulo-cli -p
  ulo-http-hyper --all-targets --all-features --no-deps --locked`, the crates whose source changed,
  with the OpenSSL flags: exit 0, 26 warning locations outside `crates/ulo/src`, none on a changed
  line or in a new file, by batch 25's `changed_lines.py`, which reported two planted locations,
  one on a changed line of `client.rs` and one in `compat.rs`, and not a planted unchanged one
  (`verify/after-doc-fix/clippy-*.txt`, `checker-selftest*.txt`). `cargo +1.98.1 clippy -p
  ulo-macro-lints --all-targets --no-deps --locked -- -D warnings`: exit 0.
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, running before the
  broker suites and after each, beside the same eight exited containers, and no other
  (`suites/docker-*.txt`, `rabbitmq-*.docker-after.txt`). Every container a suite started was
  removed by it. No permission check refused an action.
- **Disk:** 34 GB free at the start; {{DISK}} at the end, after the pinned nightly, its Miri
  builds in a target directory of their own and the rebuilds the lock's change caused.
