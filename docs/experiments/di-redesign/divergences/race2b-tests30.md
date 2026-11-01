# Divergences: race 2b, tests batch 30, `FuturesIo`'s count check held to its message under Miri (S1), the unsafe list's three refusals planted in a self-test the check runs (S2), `StreamCall::idle_timeout` taking a `Bound`, a per-call zero refused before any frame, and every expiry computed with `checked_add` (S3, S4, F391), and `ulo dev` failing a start whose duplicate of the held socket cannot be made (S6)

The fifty-fifth response, signed off 2026-10-17, answers `race2b-tests29.md`. `FuturesIo`'s check of
a reader's count stays before `advance`, and its test now expects the whole message, both numbers
in it; std refuses an over-counting reader too, by an assertion in `BorrowedCursor::advance_checked`
that its `read_buf` reaches rather than in `default_read_to_end` itself. The slice the reader gets
is the region hyper's cursor hands out, zeroed whole on every poll. `scripts/unsafe_allowed.py`
runs the list's rules over `scripts/unsafe_allowed_selftest.txt` before `check` and before `miri`,
and fails unless each planted line is refused with its rule's finding. `StreamCall::idle_timeout`
takes a `Bound`: `Default` the client's, `Unbounded` none, `After(d)` in place of the client's, and
`After(Duration::ZERO)` a `BadRequest` when the call is awaited, before it connects. A probe found
that a stream's `idle_timeout(Duration::MAX)` does not panic and its `.deadline(Duration::MAX)` does,
filed as F391; every bound now becomes an expiry through `checked_add`, and one that overflows is
none. `ulo dev` makes the held socket's duplicate where it decides to start a child, before stopping
anything; a duplicate that cannot be made fails that start, in red with the OS error, and the job's
spawn function, which can refuse a spawn where the spawn hook cannot, takes the duplicate once.

Files changed: for S1, `crates/ulo-server-hyper/src/compat.rs`; for S2, `scripts/unsafe_allowed.py`,
a new `scripts/unsafe_allowed_selftest.txt`, `unsafe-allowed.txt`'s header and `.github/workflows/ci.yml`'s
comment on the check; for S3 and S4, `crates/ulo-rpc/src/{client.rs, client_module.rs}`,
`crates/ulo-rpc-conformance/src/lib.rs` and its `cases/{deadlines.rs, cancel.rs, drain.rs, errors.rs,
streams.rs, threads.rs}`; for S6, `crates/ulo-cli/src/commands/dev.rs`; in the workspace's
`FRAMEWORK_GAPS.md`, F391 and F392. The batch started from `4a936dd2`; two commits landed during
it, `27e053f5` and `bbb35a6b`, each changing `DESIGNER_HANDOFF.md` alone, so no run here is affected. Versions are unchanged at `0.1.0-alpha.1`, and `Cargo.lock` is unchanged.

## The signatures

```rust
// ulo_rpc::StreamCall<Res>
pub fn idle_timeout(self, idle: Bound) -> Self;   // was `idle: Duration`; `Bound::Default` unless called

// ulo_rpc::RpcError
impl Error for RpcError {
    fn source(&self) -> Option<&(dyn Error + 'static)>;  // new: a `ZeroTimeout` for a stream's zero idle timeout
}
```

```text
# scripts/unsafe_allowed_selftest.txt: the list's grammar, each planted line under `# expect: <finding>`;
# `<whitespace>` stands for a space, a tab, a no-break space and a space
```

Unchanged: `Call::timeout(Duration)` and `StreamCall::deadline(Duration)` (see S1 below),
`RpcClient::timeout`, `RpcClient::idle_timeout`, `RpcClientModule`'s two, `ZeroTimeout`'s public API
and its two messages, `FuturesIo`'s public API, `ulo dev`'s flags, `unsafe_allowed.py`'s commands.

What changes in meaning without a signature: a call's or a client's bound whose end an `Instant`
cannot hold, `Duration::MAX` among them, is none, as `Bound::Unbounded` is, so no timer is asked of
the runtime for it and no `deadline-ms` carries it, where `.deadline(..)` with one panicked and the
timeouts stamped `deadline-ms: 18446744073709551615999`. `RpcError::reason` can answer
`"zero_timeout"`. `ulo dev` with `--listen`: a duplicate that cannot be made fails the start in red,
the running application keeps serving and the next change tries again, where batch 29 reported it in
yellow and started the child with `LISTEN_FDS=1` and nothing at descriptor 3; a restart returns once
the job has spawned the new child, where it returned at once.

## Decisions

### 1. S1: the count check, std's stance, and the slice the reader receives

- **The check** is batch 29's, `assert!(read <= offered, "a futures-io reader reported {read} bytes
  read into a {offered}-byte buffer")` before `advance`. Its test,
  `a_reader_counting_past_its_slice_panics`, a reader answering `buf.len() + 1` into a 16-byte
  buffer, now expects the whole message, "a futures-io reader reported 17 bytes read into a 16-byte
  buffer", where it expected only the part naming the buffer; it runs under Miri with the module's
  other three tests.
- **std.** In the 1.98.1 toolchain's source (`library/std/src/io/mod.rs`) and the pinned nightly's
  (`library/alloc/src/io/read.rs`, where `io` has moved), `default_read_to_end` asserts nothing
  itself. It reads through `r.read_buf(cursor.reborrow())` and through `small_probe_read`, which
  does `buf.extend_from_slice(&probe[..n])` on a 32-byte probe, so a count past 32 panics as a slice
  index. `read_buf`'s default is `default_read_buf`:

  ```rust
  pub(crate) fn default_read_buf<F>(read: F, mut cursor: BorrowedCursor<'_, u8>) -> Result<()>
  where
      F: FnOnce(&mut [u8]) -> Result<usize>,
  {
      let n = read(cursor.ensure_init())?;
      cursor.advance_checked(n);
      Ok(())
  }
  ```

  and `BorrowedCursor::advance_checked` (`library/core/src/io/borrowed_buf.rs:288` in 1.98.1, `:380` in the nightly)
  is `let init_unfilled = if self.buf.init { self.buf.buf.len() - self.buf.filled } else { 0 };
  assert!(n <= init_unfilled);`. So std takes the stance the response gives it, a panic for a reader
  reporting more than its buffer, through `read_buf`'s default rather than in `default_read_to_end`,
  and with no message. `FuturesIo`'s message is its own.
- **The slice.** hyper's `ReadBuf` records how much of its buffer is initialized (`init`,
  `hyper-1.11.1/src/rt/io.rs:170`), but `ReadBufCursor` exposes no reading of it: `as_mut` answers
  the whole unfilled region as `[MaybeUninit<u8>]`, and `advance` raises `init` to `filled`. So
  `FuturesIo` does not zero once and track; it zeroes the whole region `as_mut` answers on every
  poll, pending ones included, and the reader's `&mut [u8]` is exactly that region, every byte of it
  written by the loop before the read. The slice never reaches past initialized memory; the safety
  comment now says the slice is that region, zeroed whole every poll, and why. No S-item.

### 2. S2: three refusals, planted in a file the check runs

- **The refusals** were in `entry_findings` already: a `syscall` entry whose reason is empty after
  stripping, a `pure-rust` entry with anything after the word, and an unknown kind word, beside an
  entry with no kind and a path listed twice. The line is stripped before it is split, so a
  `syscall` entry whose reason is whitespace alone reads as having none; whitespace is Python's,
  the no-break space included.
- **The test.** `scripts/unsafe_allowed_selftest.txt` holds one planted line per rule, each under a
  comment `# expect: <part of the finding>`, and two valid lines. `selftest()` parses it with the
  list's own parser and judges it with the list's own `entry_findings`, then fails for each planted
  line not refused with a finding containing its text, for each refused with another finding too,
  and for each valid line refused at all. The whitespace-only plant is written `syscall<whitespace>`,
  which the self-test reads as a space, a tab, a no-break space and a space, so an editor stripping
  trailing whitespace cannot turn it into the empty-reason plant.
- **Why the script runs it.** `check` and `miri` each run the self-test first and fail on it. A test
  script CI ran would be a separate step, which a workflow edit can drop or a runner skip while the
  check still reports the list clean; run inside the check, the check cannot pass without having
  refused every plant in the same run, locally as in CI. The cost is a file in `scripts/` and some
  thirty lines.
- **Findings carry line numbers** separately now, `(number, finding)`, located as
  `unsafe-allowed.txt:<n>:` when printed, so the self-test reads a finding's line without parsing
  the text.

### 3. S3 and S4: the call's `Bound`, the per-call zero, and expiries through `checked_add`

- **The values.** `StreamCall` holds a `Bound`, `Default` unless `.idle_timeout(..)` is called.
  At the await, `Default` takes the client's idle timeout, `Unbounded` takes none whatever the
  client's, `After(d)` takes `d`.
- **Where a per-call zero fails.** The response asked for the point at which a per-call
  `.timeout(Bound::After(ZERO))` fails today. There is none: `Call::timeout` takes a `Duration`,
  and `Call::timeout(Duration::ZERO)` is not refused. A probe on the starting build sent the `req`
  with `deadline-ms: 1`, then a `cancel`, and failed the call `Timeout` (`probes/max-probe-output.txt`).
  `StreamCall::idle_timeout` answers `Self`, so its zero can first fail at the await. It fails there
  first of all, before the deadline's check and before connecting, as a binary payload on a JSON link
  does: `RpcError` of kind `BadRequest`, reason `"zero_timeout"`, message `ZeroTimeout`'s
  ("`StreamCall::idle_timeout(Bound::After(Duration::ZERO))` on the tcp link would time out the
  stream; write `Bound::Unbounded` to turn the timeout off"), and the `ZeroTimeout` as the error's
  `source`. `ZeroTimeout`'s private `ends` now holds the phrase, `"every call"`, `"every stream"` or
  `"the stream"`, so the two existing messages read as before. See S1 and S2, F392.
- **The probe.** On the starting build, a stream's `.idle_timeout(Duration::MAX)` does not panic, on
  tokio or on smol, and is still waiting after 300 ms; nor do `RpcClient::idle_timeout(After(MAX))`,
  `RpcClient::timeout(After(MAX))` and `Call::timeout(Duration::MAX)`, each of which only hands the
  duration to `Timer::sleep`, and tokio's and `async-io`'s sleeps compute their instant with
  `checked_add`. `StreamCall::deadline(Duration::MAX)`, and `Duration::from_secs(u64::MAX / 2)`,
  panic at `client.rs:916:42`, "overflow when adding duration to instant", before any frame, on both
  runtimes. The two timeouts stamped `deadline-ms: 18446744073709551615999`, which the server's
  `parse::<u64>()` refuses, so it set no deadline. Filed as F391.
- **The rule.** Each bound becomes an expiry at one of two places, each reading the runtime's clock
  once and testing `now.checked_add(bound)`: `Spec::limit`, for a call's `.timeout`, the client's
  timeout and a stream's `.deadline`, and `Limit::frame`, for a stream's idle timeout, the call's or
  the client's, before each wait. A bound that overflows is dropped there, as `Unbounded`: no sleep is
  asked of the runtime and `limit` stamps no `deadline-ms` for it. `limit` now answers the stream's
  end instant beside the wait, computed with `checked_add` from the same reading, where
  `into_future` added it with `+` after a second reading. `Limit::frame` takes that end and `now`,
  where it took the time left.
- **Not changed: the server's and the held cancel's sums.** `dispatch.rs:202` and `held.rs:32` add a
  `deadline-ms` to an instant with `+`. The header parses as a `u64` of milliseconds, and
  `Instant::now() + Duration::from_millis(u64::MAX)`, about 5.8 × 10⁸ years, did not panic in the
  probe, while `Instant::now() + Duration::MAX` did, so no header reaches an overflow there on this
  host; the response's rule names the client's bounds, and these are not.
- **The conformance scenarios** are five, on every link, in `deadlines.rs`, their bounds the
  broker's `Budget::settle` (`idle_of`) and four of it (`silence_of`), as batch 29's are; the
  scenario list in `lib.rs` names each. See Tests.
- **The overflow tests** are unit tests in `client.rs`, since a conformance scenario cannot wait
  long enough to tell no timer from a long one. Their first version waited forty years on tokio's
  paused clock, on the premise that a timer armed for `Duration::MAX`, which tokio's `sleep` turns
  into thirty years from now, would fire first. A probe showed it does not: under a forty-year
  `timeout` on the paused clock, `sleep(30 years)` and `sleep(Duration::MAX)` each lose to the
  timeout (`probes/paused-clock-max-sleep.txt`), and two of the first breaks crashed the test
  process, SIGBUS and SIGSEGV, rather than failing an assertion (`broken/superseded/`), the cause not
  traced. The tests now run on a `Recording` runtime that passes every `sleep` to tokio and records
  its duration: each asserts the call or stream is still waiting after an hour and that the client
  asked for no sleep, and the three whole-call ones that the opening frame carried no `deadline-ms`.
- **`.deadline`** takes a `Duration`, as does `Call::timeout`. Both are listed below, not changed.

### 4. S6: the duplicate made where `ulo dev` decides to start, and a spawn that can refuse

- **The premise.** In `command-fds` 0.3.3, `pub struct FdMapping { pub parent_fd: OwnedFd, pub
  child_fd: RawFd }` (`src/lib.rs:98-101`), and `fd_mappings(&mut self, mut mappings:
  Vec<FdMapping>)` (`:130`) moves them into the `pre_exec` closure it installs; its doc says the
  `Command` owns the descriptors until it is dropped. So the duplicate is an owned descriptor at
  configuration time and can be made anywhere before.
- **The watcher's API.** `Job::set_spawn_hook` (watchexec-supervisor 5.4.1, `job/job.rs:320`) takes
  `Fn(&mut CommandWrap, &JobTaskContext<'_>) + Send + Sync`, returning `()`: it cannot refuse a
  start. `Job::set_spawn_fn` (`:369`, added in 5.4.0) takes `Fn(&mut tokio::process::Command) ->
  std::io::Result<tokio::process::Child> + Send + Sync`, passed to `CommandWrap::spawn_with` after the
  spawn hook and process-wrap's layers (`job/state.rs:96-98`); an `Err` goes to the job's error
  handler and leaves the job not running (`job/task.rs`, `try_with_handler!`). So the API can refuse a
  spawn, through the spawn function, and the fallback the response gave for one that cannot is not
  needed.
- **Where the duplicate is made.** `start`, which `supervise` calls once a build succeeds and which
  decides between restarting the job and making a new one, makes the duplicate first, before
  stopping or restarting anything, through `Plan::duplicate`, a `fn(&TcpListener) ->
  io::Result<TcpListener>` that is `TcpListener::try_clone` in the command and the seam a test fails.
  A failure returns from `start` with "cannot duplicate the held socket for `<binary>`, so it was not
  started; the running application keeps serving, and the next change tries again: <OS error>", the
  middle clause only when one is running, which `supervise` prints in red through `failure(..)`, as it
  prints every other failed start. No job is touched, so the running application, if any, keeps
  serving, and the next successful build calls `start` again.
- **The slot.** `Handoff`, an `Arc<Mutex<Option<TcpListener>>>` per job, kept in `Running`. `start`
  fills it with the duplicate and then starts or restarts the job; each fill drops, and so closes, a
  duplicate no spawn took. The job's spawn function, `spawn_with_socket`, takes it once: with a
  duplicate it places it at descriptor 3 through `place_fd`, sets `LISTEN_FDS=1` and
  `LISTEN_FDNAMES`, and spawns, the command owning the descriptor and closing it once the child has
  spawned; with none it answers `Err`, "no duplicate of the held socket was made for this start",
  which the job's error handler prints in red, and no child starts. The spawn hook sets `ULO_DEV=1`
  and removes `LISTEN_PID`, `LISTEN_FDS` and `LISTEN_FDNAMES` from every child, so the two socket
  variables are set in one place, by the spawn that places the socket. See S3 below.
- **Restarts are awaited.** `start` awaits the restart's `Ticket`, which resolves once the job has
  processed the restart's `Start`, so its spawn has taken the duplicate before the next restart can
  fill the slot. Unawaited, back-to-back restarts filled the slot several times before the job's
  first spawn read it, and the later spawns found it empty (break s05). The first start awaits
  `job.start()`'s ticket for the same reason. `supervise` already awaited a stop of up to
  `STOP_GRACE` when the binary changed; a restart now waits the same way.

## The tests

- **S1,** `a_reader_counting_past_its_slice_panics`'s full expected message; `compat.rs`'s four
  tests under Miri on the pinned nightly.
- **S2,** the self-test in every `check` and `miri` run, and a break of each rule.
- **S3 and S4, the response's five,** conformance scenarios on every RPC link, each bound from
  `Budget::settle`, `idle` being one settle and the silence four:
  1. `a_stream_s_unbounded_idle_timeout_outlasts_the_client_s`: a client whose idle timeout is
     `idle`, a call at `Bound::Unbounded`, silent for four `idle`s between its two items, yields both
     and ends cleanly, no sooner than the silence;
  2. `a_stream_s_default_idle_timeout_is_the_client_s`: the same client, a call at `Bound::Default`,
     ends `Timeout` between `idle` less 50 ms and the silence's end;
  3. `a_stream_s_shorter_idle_timeout_replaces_the_client_s`: a client whose idle timeout is eight
     settles, longer than the silence, a call at `After(idle)`, ends `Timeout` at `idle` as in 2;
  4. `a_stream_s_zero_idle_timeout_is_refused_before_any_frame`: through a client link counting its
     connects and the frames it sends, a stream at `After(Duration::ZERO)` fails `BadRequest`, reason
     `zero_timeout`, a `ZeroTimeout` its source, with no connect and no frame; a unary call through
     the same link then answers and moves both counts, which shows the counter counts. It runs on UDP
     too, the refusal being the client's;
  5. `a_stream_s_longest_idle_timeout_does_not_panic`: a client whose idle timeout is `idle`, a call
     at `After(Duration::MAX)`, silent for four `idle`s, yields both items and ends cleanly.

  The four streaming ones are refused at startup on UDP, as the other streaming scenarios are.
- **The overflow, per expiry site,** unit tests in `client.rs` on tokio's paused clock and the
  `Recording` runtime: `an_overflowing_client_timeout_is_no_timeout` (`RpcClient`'s timeout at
  `After(Duration::MAX)`, a request), `an_overflowing_call_timeout_is_no_timeout`
  (`Call::timeout(Duration::MAX)`), `an_overflowing_stream_deadline_is_no_deadline`
  (`.deadline(Duration::MAX)`, F391), `an_overflowing_client_idle_timeout_is_no_idle_timeout` and
  `an_overflowing_call_idle_timeout_is_no_idle_timeout` (the call's at `After(MAX)` on a client of
  100 ms). The existing client tests changed only to write `Bound::After(..)`.
- **S6,** in `dev.rs`'s tests, beside batch 29's placement tests:
  - `a_failed_duplicate_starts_no_child_and_names_the_os_error`: the plan's duplicate fails with
    `io::Error::from_raw_os_error(24)`, EMFILE on Linux and macOS; the first `start` returns an error
    whose report is red and contains "Too many open files (os error 24)", makes no job, and no child
    writes a line within 500 ms; with the duplicate restored the next `start` starts a child, its line
    `start socket 1`; failing it again, a restart returns an error naming the OS error and "the
    running application keeps serving", and no second line appears. The application is a shell
    script that appends `start <socket|none> <LISTEN_FDS> <pid>` to a file, `socket` when
    `/dev/fd/3` exists, and `exec`s `sleep 30`;
  - `a_spawn_with_no_duplicate_left_for_it_starts_nothing`: `spawn_with_socket` over an empty
    handoff answers the error and sets no `LISTEN_FDS`; filled, it spawns `/usr/bin/true` with
    `LISTEN_FDS=1` and leaves the handoff empty;
  - `restarts_leave_the_descriptor_count_unchanged`: the test binary run again as the ignored
    `restart_leak_child`, which starts the application, counts its own open descriptors through
    `/proc/self/fd` or `/dev/fd`, restarts it five times, waiting for each child's line, counts again,
    and asserts the counts equal and every line `start socket 1`. In a process of its own because
    the other tests open descriptors while it counts. On the tree: 11 before, 11 after;
  - `back_to_back_restarts_leave_a_child_running_with_the_socket`: four restarts with no wait
    between them, then, once no line has been added for 500 ms, the last line's process is alive
    (`kill -0`) and its line is `start socket 1`.

### Before and after

Each break went through `batch30/scripts/brk.py`: it replaces each span (asserting it occurs once),
runs the target under bash, writes every touched file and `Cargo.lock` back byte for byte, and
compares a SHA-256 over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`,
`scripts` and `unsafe-allowed.txt` and every untracked file under `crates` and `scripts`. Specs in
`batch30/specs/`, output in `batch30/broken/` with `summary.txt`. Every restore reported `ok`. The
breaks up to s05 ran on the tree hashed `28a66b2e..`; the overflow tests were then rewritten onto
the recording runtime, and o01–o04 and k02b ran again on `db8f3e08..`, the tree every verification
run below used. The first o01–o04 outputs are in `broken/superseded/`.

| Break | Target | Result |
| --- | --- | --- |
| s1a. the count check removed | `compat.rs`'s tests | `a_reader_counting_past_its_slice_panics` failed: hyper panicked first, "range end index 17 out of range for slice of length 16" at `hyper-1.11.1/src/rt/io.rs:261`, "panic did not contain expected string" |
| s1b. the message naming the buffer alone | the same | the same test failed, its panic "a futures-io reader reported more bytes than its 16-byte buffer" |
| k01. a `syscall` entry with no reason accepted | `check`; `miri --dry-run` | both exit 1: "self-test: scripts/unsafe_allowed_selftest.txt:11: a planted line was not refused with `gives the reason Miri cannot execute it`; found: nothing", and the same for line 15, the whitespace plant |
| k02. the reason not stripped and the line split on single spaces | `check` | exit 0: the line is stripped before it is split, so the plant's whitespace was gone before the reason was read; not a break of the rule (`k02-first-spec-line-strip-still-removed-it.txt`) |
| k02b. the line stripped on the left alone, split on single spaces, the reason not stripped | `check` | exit 1, line 15 alone not refused, the empty-reason plant still refused |
| k03. a `pure-rust` entry's reason accepted | `check` | exit 1, line 19 not refused with `takes no reason` |
| k04. an unknown kind accepted | `check` | exit 1, line 23 not refused with `is no kind` |
| k05. a missing kind accepted | `check` | exit 1, line 27 not refused with `has no kind` |
| k06. a path listed twice accepted | `check` | exit 1, line 35 not refused with `is listed again` |
| k07. a `syscall` reason shorter than 80 characters refused | `check` | exit 1, "a valid line was refused", line 31 |
| i01. a call's `Default` as no idle timeout | TCP's five new scenarios; `client.rs`'s tests | `a_stream_s_default_idle_timeout_is_the_client_s` failed, "expected an error of kind Timeout, got: Ok(1)"; `a_client_wide_idle_timeout_ends_a_silent_stream` failed, "did not happen within 5s" |
| i02. a call's `Unbounded` as the client's | TCP's five | `a_stream_s_unbounded_idle_timeout_outlasts_the_client_s` failed, "the stream failed 502.66ms after it opened", the idle timeout |
| i03. the client's idle timeout winning over the call's | the same | the shorter-replaces scenario failed, `Ok(1)` where `Timeout` was due, and the `Duration::MAX` one, the client's `idle` ending it at 501.71 ms |
| i04. the per-call zero not refused | the same | the zero scenario failed, "expected an error of kind BadRequest, got: Ok(())", the stream opening |
| i05. the zero refused after the call opened | the same | the zero scenario failed, connects and frames `(1, 2)` where `(0, 0)` was due |
| o01. the idle timeout not checked against the clock | `client.rs`'s tests; TCP's five | the two idle overflow tests failed, the sleeps asked of the runtime `[18446744073709551615.999999999s, ..]` where none were due; TCP's five passed, tokio's sleep taking `Duration::MAX` without a panic |
| o02. the idle timeout's end added with `+` | the same | the two idle overflow tests and `a_stream_s_longest_idle_timeout_does_not_panic` panicked, "overflow when adding duration to instant", `client.rs:727` |
| o03. the whole-call bound not checked | `client.rs`'s tests | the two timeout tests failed, a sleep of `Duration::MAX` asked; the deadline test failed, `deadline-ms` `18446744073709551615999` where none was due |
| o04. F391's form: the bound not checked and the end added with `+` | the same | the three whole-call tests panicked, "overflow when adding duration to instant", `client.rs:464` |
| s01. a failed duplicate ignored | `ulo-cli`'s tests | `a_failed_duplicate_starts_no_child_and_names_the_os_error` failed, `start` answering `Ok(())` where an error was due |
| s02. batch 29's fallback: a failed duplicate reported in yellow, the spawn placing nothing and setting `LISTEN_FDS=1` | the same | the failure test and `a_spawn_with_no_duplicate_left_for_it_starts_nothing` failed, the latter spawning a child |
| s03. an empty handoff spawned with `LISTEN_FDS=1` | the same | `a_spawn_with_no_duplicate_left_for_it_starts_nothing` failed, a `Child` where an error was due |
| s04. a planted leak: the spawn placing a clone of the duplicate and never closing the duplicate | the same | `restarts_leave_the_descriptor_count_unchanged` failed: "12 descriptors open before 5 restarts, 17 after" |
| s05. a restart not awaited | the same | `back_to_back_restarts_leave_a_child_running_with_the_socket` failed, the last child to start not running, one line written |
| the restored tree | every target above | every test passed, the check reported no finding |

## Left for the transports DESIGN fold

Line numbers are those of `4a936dd2`.

- §0's principle 7 (line 15) and decision 113 (line 1620): the check runs the list's rules over
  `scripts/unsafe_allowed_selftest.txt` first, a planted line per rule, and fails unless each is
  refused; a `syscall` reason of whitespace alone is none.
- §3.7's `FuturesIo` line (line 598) and decision 112 (line 1619): the region the reader gets is the
  one `as_mut` answers, zeroed whole each poll, `ReadBufCursor` exposing no record of what hyper
  holds initialized; the count check's message names both numbers; std's `read_buf` refuses an
  over-count through `advance_checked`.
- §5.4's client paragraph (line 1096), X38 (line 1407), decisions 111 (line 1618) and 118 (line
  1625): `StreamCall::idle_timeout(Bound)`, `Default` the client's, `Unbounded` none, `After(d)` in
  place of the client's; 118's alternative "`StreamCall::idle_timeout` taking a `Bound`" is now the
  design, and its "a call cannot turn the client's off" goes. "The per-call values are `Duration`s"
  holds of `.timeout` and `.deadline` alone.
- §5.4 and decision 106 (line 1613): a bound whose end an `Instant` cannot hold is none, stamped
  nowhere and timed by no sleep, at the two places a bound becomes an expiry.
- §12's refusals table (after line 1464, the binary payload's row): a stream's
  `.idle_timeout(Bound::After(Duration::ZERO))`, runtime before any I/O, `RpcError` of kind
  `BadRequest`, `reason: "zero_timeout"`, its source a `ZeroTimeout`; and `RpcError::source`.
- §9 (line 1338) and decision 117 (line 1624): the duplicate is made where `fw dev` decides to start
  a child, before it stops anything, and a failure fails that start in red with the OS error, the
  running application serving on; the job's spawn function takes it once from a per-job slot and
  refuses a spawn finding none; the socket's two variables are set there alone; a restart returns
  once its spawn has run. The sentence on a duplicate the process cannot make, and 117's on the
  spawn hook being unable to fail the restart, go.

## Needs sign-off

Numbered from S1 for this batch.

### S1. `Call::timeout` and `StreamCall::deadline` take a `Duration`, and neither refuses zero

F392. Listed as the response asked for `.deadline`, and `Call::timeout` for the same reason: §3.4's
two shapes for one name, `.timeout(Bound)` on the client and `.timeout(Duration)` on the call, and a
per-call zero that times out at once instead of being refused, its `req` sent or not by the race in
`bounded`'s `select!`. The alternative is both taking a `Bound` and refusing a zero where the
stream's idle timeout is refused, which this batch did not do.

### S2. A stream's zero idle timeout is a `BadRequest` at the await, its `ZeroTimeout` the source

Decision 3. The response's point to match does not exist, so the refusal sits where the client's
other refusal of a call as written sits, a binary payload on a JSON link, at the await before
connecting, and the error the await answers is an `RpcError`; `RpcError` gains `Error::source` to
carry the `ZeroTimeout`. The alternatives are `idle_timeout` answering `Result<Self, ZeroTimeout>`,
which changes the signature the response set, or an `RpcError` carrying the message and reason
without the type.

### S3. The spawn function, not the spawn hook, takes the duplicate

Decision 4. The response says the hook takes the slot once. The hook cannot refuse a start; the
job's spawn function can, so it takes the slot, and a spawn finding it empty starts nothing. The
alternative is the hook taking it, with no way to stop a child that finds the slot empty from
starting, then without the socket and with the two variables removed.

### S4. A restart waits for its spawn

Decision 4. `start` awaits the restart, up to `STOP_GRACE` and the spawn, so one slot serves
back-to-back restarts. The alternative is a queue of duplicates, which leaves the restart
unawaited and holds one duplicate per restart not yet spawned.

### S5. A failed duplicate leaves the running application serving

Decision 4. The duplicate is made before anything is stopped, so a failed start stops nothing, as a
failed build does. The alternative is stopping the old child first and failing after, which leaves
nothing serving until the next change.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch30/`: `suites/` for the
RPC suites (with `summary.txt` and the container lists before and after each), `verify/` for the
hosts, checks, workspace test, docs and clippy, `miri/` for the Miri run, `broken/` and `specs/` for
the breaks, `probes/` for the three probes and their crate, `scripts/`. Every run below ran on the
tree hashed `db8f3e08..` by `brk.py`'s measure (`verify/final-tree-hash.txt`). Clippy then found the
new `let _ = job.set_spawn_fn(..)` (`let_underscore_future`), written `drop(..)` with a comment; on
that tree, `394d556a..`, `ulo-cli`'s clippy and tests ran again in a target directory of their own
while the workspace test held the shared one (`verify/after-drop/`).

- **The RPC suites, once each,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 40 passed and 3 ignored each (2.01 s, 2.01 s,
  2.02 s); the smol TCP link's three 40 and 3 each (2.01 s each); UDP 37 and 6 (1.32 s). With
  `--features integration --test conformance` and the OpenSSL flags: NATS 42 and 1 (20.09 s); Redis
  41 and 3 (11.31 s); MQTT 47 and 1 (13.02 s); Kafka 43 and 1 (60.58 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 42 and 1 (195.32 s). Each count is batch 29's and the five new
  scenarios, which passed on all twelve. Batch 29 took 4.88 s on NATS, 7.18 s on Redis, 7.30 s on
  MQTT, 37.56 s on Kafka and 90.28 s on RabbitMQ; the host's load during these runs is not
  recorded.
- **The HTTP and WebSocket suites on a tokio and a smol host** (`verify/hosts/summary.txt`):
  `ulo-http-hyper`'s suite on tokio 36 and 2, on smol 36 and 2, its TLS tests 2 on each, its
  `listeners.rs` 7 and 2 ignored; the WebSocket suite in `ulo-ws-hyper`'s `conformance` and
  `conformance_smol` 31 each, and in `ulo-ws`'s `conformance`, `conformance_smol` and
  `conformance_pipe` 31 each; `ulo-server-hyper` 4 unit in `compat.rs`, 3 in `rt.rs`, 4 in
  `serve.rs`; `ulo-listener-smol` 2; `ulo-rpc` 15 unit, 3, 11 and 5 in its test targets, its doctests
  1 and 5 ignored, then the `compile_fail` one; `ulo-cli` 7 and 2 ignored.
- **The Miri step** on the tree: `MIRI_TOOLCHAIN=nightly-2026-10-11 python3
  scripts/unsafe_allowed.py miri` in a new target directory, `batch30/target-miri`: the self-test's
  line, `activation.rs` printed as skipped with its reason, then `cargo +nightly-2026-10-11 miri test
  --locked -p ulo-server-hyper --lib -- compat::`, 4 passed in 27.01 s of Miri's clock, "Miri
  executed 4 of its tests, exit 0" (`miri/job-miri-pinned-own-target.txt`). A fresh target directory
  built `ring` without batch 29's failure.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 455 files scanned, 2 listed, no finding
  (`verify/unsafe-check.txt`), on both trees.
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, no failure and no error (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, `ulo`'s 17 known warnings in `crates/ulo/src` and no location elsewhere
  (`verify/check-*.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in a target directory of its own: exit 0, the same 17 and no
  other (`verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast` with the OpenSSL flags: exit 0,
  1,157 passed, 0 failed, 141 ignored, over 199 `test result` lines (`verify/workspace-test.txt`).
  Batch 29's record of the same run, the one file of its `verify/` left in the scratchpad, holds
  1,110 passed over 172 lines. By target: 35 more in the TCP, UDP and smol TCP conformance binaries,
  five scenarios on each of seven; 9 more unit tests, `client.rs`'s five and `dev.rs`'s four; and 3
  in `ulo-rpc`'s and `ulo-runtime-smol`'s doctests, which that record does not list.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`,
  `ulo-server-hyper` and `ulo-rpc-conformance`, the last at its defaults and with `tokio,smol`: exit 0
  each (`verify/doc-*.txt`). `ulo-cli` has no library.
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-conformance -p ulo-server-hyper -p ulo-cli
  --all-targets --all-features --no-deps --locked`, with the OpenSSL flags: exit 0, 25 warning
  locations outside `crates/ulo/src`, one on a changed line, `dev.rs`'s `let _ =
  job.set_spawn_fn(..)`, by `batch30/scripts/changed_lines.py`, which in the same output passed over
  the two unchanged `let _ = job.set_*` lines beside it (`verify/clippy.txt`). After the `drop(..)`,
  `ulo-cli`'s clippy has 15 locations, none on a changed line, and its tests 7 passed and 2 ignored
  (`verify/after-drop/`).
- **Containers:** at the batch's start the user's four, seaweedfs, mailpit, postgres:18 and
  redis:7, were exited, beside eight other exited containers (`verify/docker-start.txt`). By the
  first suite run, half an hour later, the four were running, started by something outside this
  batch; nothing here starts them. Every suite's list after it matches its list before: twelve
  containers, the four running, and none the suites started left behind
  (`suites/*.docker-{before,after}.txt`, `verify/docker-end.txt`). No permission check refused an
  action.
- **Disk:** 32 GB free at the start, 8 GB at the end, after a fresh Miri target directory, the
  probe crate's builds and the rebuilds the client's change caused.
