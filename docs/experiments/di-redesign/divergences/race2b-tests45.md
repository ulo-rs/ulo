# Divergences: race 2b, tests batch 45, a drop during a running `close` leaves that close its servers (F426's residual case)

The sixty-sixth response's point 3, signed off 2026-10-18, changes batch 42's S2: a drop while a
`close` on a handle runs the shutdown elsewhere no longer releases the servers unclosed. The
`App` value's drop now leaves them to every `close` under way, and that close's close step closes
each one and releases them. What marks "a shutdown is running" is not the phase: a count of the
`close` calls under way, taken at each one's first poll, beside a flag the drop sets, both under one
`std::sync::Mutex` in the shutdown cell. The phase only moves forward and reads `Stopping` for a
shutdown whose runner was dropped and never resumed, and a drop that read it left the servers to
nobody (p01). A `close` dropped before its close step, with the `App` value gone and no other
`close` under way, releases the servers as it drops, unclosed, as the drop would have. With the
drop releasing unconditionally, the three new tests fail; with the count never cleared, the third
and batch 42's abandoned-shutdown test fail; with the drop unwrapping a poisoned lock, batch 42's
fourth fails. One window remains and is stated, not closed: a `close` first polled after the drop
is a `close` after the drop (S2).

Files changed: `crates/ulo/src/lifecycle/shutdown.rs`, `crates/ulo/src/app/mod.rs`,
`src/app/handle.rs` and `src/app/shared.rs`; the tests in `crates/ulo-runtime-tokio/tests/app.rs`.
`Cargo.lock` is unchanged. The batch started from `d2e6b26a`, made no commit and did not edit the
workspace's `FRAMEWORK_GAPS.md`; the F426 note is at the end of this log for the coordinator to
paste. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

No public signature changes.

```rust
// crates/ulo/src/lifecycle/shutdown.rs, crate-private
struct Release { app_dropped: bool, closing: usize }   // new: ShutdownCell's `release` field, behind a std Mutex
struct Closing<'a>(&'a AppShared);                     // new: one `close` under way; its Drop may release
pub(crate) fn app_dropped(shared: &AppShared);         // new: what `Owner::drop` calls
```

What changes in meaning without a signature:

- A drop of the `App` value while a `close` on a handle is under way, counted from that close's
  first poll, leaves the servers to it: its drain and close step reach every server, each server's
  `close` runs, and the close step's end releases them. Batch 42's tree released them at the drop,
  and that close closed none.
- A `close` under way at the drop, itself dropped before its close step with no other `close`
  under way, releases the servers as it drops, unclosed. A server's drop can then run on the thread
  dropping that `close` future, where before it ran on the thread dropping the app or its `serve`.
- A `close` polled once and then neither polled again nor dropped holds the servers for as long as
  it is kept. So does one stuck in a hook that never returns on an app without a `Timer`, whose
  hooks are unbounded.

## Decisions

Line numbers are those of the batch's tree.

### 1. What the brief asked to settle, by probe

The probes and breaks ran in a mirror, `git archive d2e6b26a` with this batch's `crates/ulo` and
`crates/ulo-runtime-tokio` copied in by `rsync --checksum` and checked with `diff -r` against the
real tree before each step, and `cargo clean -p ulo -p ulo-runtime-tokio` before the fix's first
measured run. `batch45/scripts/brk.py` (batch 42's script pointed at the mirror) copies each file it
edits aside to `batch45/aside/`, copies it back after the runs and `cmp`s the two; every restore
reported `identical`.

- **The unchanged core against the new tests.** `p00-*-0.txt`, `d2e6b26a` with this batch's tests:
  all three fail. A drop inside the before-shutdown hook dropped both servers at once
  (`["drop a", "drop b"]` logged at the app's drop); a drop during the drain left them alive, since
  the drain holds its own copy of the list, and the close step then found the list empty and closed
  neither (`["before-shutdown", "drain a", "drain b", "drop a", "drop b", "store dropped"]`).
- **Whether the phase can mark a running shutdown.** It is set early enough. A handle's `close`
  advances it to `Stopping` in its own first poll: `close` triggers, `run_sequence` enters the
  `OnceCell`'s initializer, which async-lock 3.4.2 calls and polls in the same poll once its
  compare-exchange wins (`once_cell.rs:633-654`, read), and `sequence` takes the uncontended
  progress lock and advances the phase (`shutdown.rs:204`) before any suspension. It cannot tell a
  live runner from a dropped one: `advance` never moves back, and a runner dropped inside the
  before-shutdown hook leaves the phase at `Stopping`. p01 (`p01-*-0.txt`) made the drop return
  without releasing from `Stopping` up to `Closed`: the first two new tests passed, and batch 42's
  `an_app_whose_shutdown_is_abandoned_drops_its_server` and this batch's third failed, the server
  never dropping (`the server outlived the abandoned shutdown`; `` `drop a` was not logged once
  after the abandoned close: ["before-shutdown"] ``). The phase also reads `Destroying` through the
  shutdown step after the close step has released, which does not matter for a release that is a
  `mem::take`.
- **What marks it instead.** A `Closing` value, taken as the first statement of
  `shutdown::close` (`shutdown.rs:144`), before the trigger and before any await, and held until
  that close's future ends or drops. Each `close` counts, the ones waiting on another's sequence
  among them: a waiter takes over the `OnceCell`'s initializer when the runner's future drops, and
  resumes the sequence where it stood (p05a). With the count taken inside `sequence` instead, a
  runner's abandonment counted down to zero while a waiter was still to resume, released the
  servers, and the waiter's close step closed none (p05b).
- **The window that remains.** Claiming at the first poll closes the in-poll window the brief
  names: the claim and the drop's check take the same lock, so of a first poll on one thread and
  the drop on another, one runs entirely before the other. p04 (`p04-*-0.txt`), 500 races per run
  on a two-worker runtime, a `close` spawned and the app dropped after a spin of varying length,
  three runs: 486, 478 and 486 races had every server closed, 14, 22 and 14 had both released
  unclosed, and none had one server closed without the other, a server left undropped, or the store
  outliving the last handle. What a claim at the first poll cannot cover is a `close` future made
  before the drop and first polled after it, which is a `close` after the drop and closes nothing.
  Covering it needs the claim at the call, `AppHandle::close` written as a `fn` returning the future
  rather than an `async fn`, which changes a public signature; not done (S2). The first p04 run, with
  no spin, had the drop win all 1,500 races and exercised nothing; it is in `superseded/`.

### 2. The fix: the drop leaves the servers to a `close` under way

`ShutdownCell` gains `release: Mutex<Release>` (`crates/ulo/src/lifecycle/shutdown.rs:57`),
`Release` holding `app_dropped` and `closing` (`:63`). `Owner::drop` (`crates/ulo/src/app/mod.rs:161`)
calls `app_dropped` (`shutdown.rs:177`): under the lock it sets `app_dropped` and reads whether
`closing` is zero, and only then, outside the lock, calls `release_servers`. `Closing::claim`
(`:156`) increments `closing`; `Closing`'s drop (`:162`) decrements it and, when the app has dropped
and the count reached zero, releases. The close step's release (`:224`) is unchanged, so a close
that reaches it releases there, and the later releases, by its `Closing` and by a drop after it,
take an empty `Vec`. `release_servers` (`:402`) is unchanged.

`serve` and `App<Connected>::close` go through `shutdown::close` too, and own the `App` value's hold
themselves. In `serve` the pinned `closing` future is declared after the moved hold, so its
`Closing` drops first: dropped mid-sequence, the count falls before the hold's drop reads it, and
batch 42's `an_app_whose_shutdown_is_abandoned_drops_its_server` passes unchanged.

The drop awaits nothing and waits on no other task. It takes the release lock to set a flag and
read a count, and the list's lock to empty it, each through `unwrap_or_else(PoisonError::into_inner)`.
Nothing holds the release lock across an await or runs user code under it, so it is never held for
longer than those two field accesses, and no path poisons it. A server's own drop is user code and
runs on the thread dropping the app, or on the thread ending the last `close` under way; a panic
there is the server's, as before.

The docs on `App` (`crates/ulo/src/app/mod.rs:49`), `AppHandle` (`crates/ulo/src/app/handle.rs:33`),
`AppHandle::close` (`:77`), `AppShared::servers` (`crates/ulo/src/app/shared.rs:45`),
`release_servers`, the `release` field and the shutdown module's step 6 replace the batch-42 text
that said a drop during another task's shutdown releases the servers and that close closes none.

### 3. The tests

`crates/ulo-runtime-tokio/tests/app.rs`, beside batch 42's four. `Recording` (`:708`) is a server
keeping the `AppHandle` its `prepare` is handed, as every shipped server does, whose `drain` and
`close` log that they ran and then wait at a `Gate` (`:677`), and whose drop is logged; `Gated`
(`:761`) is a root module whose singleton logs `store dropped` as it drops and whose
before-shutdown hook logs and waits at its gate. `paired` (`:795`) binds two servers, `a` then `b`,
with the named gates closed. Each test runs `close` on a handle clone in a spawned task, so the drop
and the close are on different tasks as in the response's case.

- `an_app_dropped_before_a_running_close_s_close_step_leaves_it_every_server`: hook and close
  gates closed. The close reaches the hook (phase `Stopping`), the app drops, and no server has
  closed or dropped. The hook gate opens; `b`'s `close` starts and waits, `a`'s has not started, and
  the store is alive. The close gate opens; the close answers `Ok`; the store is alive while the
  handle is held. After the handle drops: `close b`, `close a`, both drops once and after every
  close, and `store dropped` last.
- `an_app_dropped_during_a_running_close_s_drain_leaves_it_every_server`: the drain gate closed.
  Both drains start (phase `Draining`), the app drops, and nothing has closed or dropped; the gate
  opens and the same end state is required.
- `a_close_abandoned_after_the_app_s_drop_releases_the_servers`: the hook gate closed for good.
  The app drops inside the hook, the close task is aborted and awaited as cancelled; both servers
  have dropped once, neither `close` ran, the store is alive while the handle is held and drops with
  it. Batch 42's abandoned-shutdown test holds the same for a `serve` runner; this holds it for a
  handle's, and is the one test b02 fails on the handle path.

## The tests

- **F426's residual case:** `ulo-runtime-tokio`'s `tests/app.rs`:
  `an_app_dropped_before_a_running_close_s_close_step_leaves_it_every_server`,
  `an_app_dropped_during_a_running_close_s_drain_leaves_it_every_server` and
  `a_close_abandoned_after_the_app_s_drop_releases_the_servers`. Batch 42's four in the same file,
  unchanged. Decisions 2 and 3.

### Before and after

Specs in `batch45/specs/`, output in `batch45/probes/` and `batch45/broken/`, each with
`summary.txt` recording every `cmp`. The breaks ran `cargo test -p ulo-runtime-tokio --test app
--locked` in the mirror. p00 and p01 ran on `d2e6b26a`'s core, with the tests as they stood before
two of their helpers were folded into one, which changed no assertion; every other run is on the
final tree. A first round of every probe and break, on the same core before that fold and before a
doc rewording, is in `superseded/round1/` and gave the same results.

| Run | Result |
| --- | --- |
| p00. `d2e6b26a` with the new tests | the three new tests failed: servers dropped at the app's drop inside the hook; no `close` run after a drop during the drain |
| p01. the drop leaves the release when the phase is `Stopping` to `Destroying` | the first two passed; this batch's third and batch 42's abandoned-shutdown test failed, the servers never dropping |
| p02. the fix | 25 passed |
| p03. the fix, twenty runs | 25 passed each |
| p04. a `close` racing the drop on two threads, 3 × 500 | every server closed 486/478/486, both released unclosed 14/22/14; never partial, never a leak |
| p05a. a second `close` waiting while the first runs in the hook; the app dropped; the first aborted | nothing released at the drop; the second resumed after the hook, drained and closed `b` then `a`, `Ok`; the servers dropped, then the store with the handle |
| p05b. p05a with the claim inside `sequence` | the first's abandonment dropped both servers unclosed; the second answered `Ok` having drained and closed none |
| b01. the drop ignores the flag, releasing as batch 42's did | the three new tests failed: `a server closed or dropped at the app's drop: ["drop a", "drop b"]` (the first and third); `` not every server's `close` ran after a drop during the drain `` with `left: []` (the second) |
| b02. the flag is never cleared: `Closing`'s drop does nothing | the third and batch 42's abandoned-shutdown test failed, the servers never dropping and the store with them; the first two passed, the close step releasing |
| b03. the drop's release unwraps the list's lock with `expect` | batch 42's fourth failed, `the app's list of servers is poisoned` at the app's drop; all else passed |
| b04. the close step's release removed, `Closing`'s kept | batch 40's `a_closed_app_lists_no_addresses` and `a_close_on_an_unserved_app_drops_its_server` and batch 42's fourth failed; the three new tests passed (S4) |
| b05. b04 and b02 together: the shutdown never releases | eight failed: the three new tests (`no server dropped`, `` `drop a` was not logged once ``), batch 42's abandoned and fourth, batch 40's three |
| the restored tree | every target in the verification passed |

## Left for the DESIGN folds

Line numbers are those of `d2e6b26a`.

- The core DESIGN, §9.5 step 6 (line 911): "A drop while a `close` on a handle runs the sequence in
  another task, before this step, releases the servers unclosed, and that close closes none; during
  this step every `close` still runs." is false now. A drop while a `close` is under way, from its
  first poll, leaves the servers to it, and this step closes and releases them; a `close` dropped
  before this step with the app gone and no other `close` under way releases them as it drops,
  unclosed; a `close` first polled after the drop closes nothing.
- The transports DESIGN, decision 149 (line 1675): its sentences "A drop while a `close` on a
  handle runs the shutdown in another task … releases the servers unclosed, and that close closes
  none" and "Leaving the servers to a live runner would need … a flag in the shared state and an
  ordering between it and the sequence's own drop; that the drop releases whatever runs elsewhere
  is pending sign-off": the flag is built, a count of `close` calls under way and the drop's flag
  under one lock, by the sixty-sixth response. The test list gains this batch's three, and the
  breaks b01 to b03.

## Needs sign-off

Numbered from S1 for this batch.

### S1. The flag is a count of `close` calls under way, not the phase

Decision 1. The response says the drop reads the shared state's phase. The phase is set early
enough but cannot say that the runner is gone, and p01 is the drop that read it: an abandoned
shutdown kept its servers for the life of the last handle, the leak F426 was filed for. The count
is taken by every `close`, waiters included (p05a, p05b), and a `close` dropped before its close
step releases when it is the last. This is a departure from the response's mechanism, not its
behaviour.

### S2. A `close` first polled after the drop is a `close` after the drop

Decision 1. The claim is at the first poll. A `close` future made before the drop and first
polled after it closes nothing, as batch 42 already documents for a `close` after the drop. A claim
at the call would cover it, with `AppHandle::close` as a `fn` returning `impl Future + Send` and
the trigger still at the first poll: a public signature change, left to the designer. A first poll
racing the drop on another thread resolves to one of the two orders under the lock (p04).

### S3. A `close` kept and never resumed holds the servers

What the drop used to release, a `close` polled once and then neither polled again nor dropped now
holds, for as long as it is kept, and so does a `close` stuck in a hook that never returns on an app
with no `Timer`. Under a `Timer` every hook and `close` is bounded by `hook_timeout` or the cap. It
is documented on `App`. The alternative, the drop releasing after a deadline, needs a clock in a
drop and is not proposed.

### S4. The first two tests do not distinguish the close step's release from `Closing`'s

b04. With the close step's release removed, a close's `Closing` releases as its future ends, after
every `close` has run, so the order the two tests require still holds. The close step's release is
held by batch 40's `a_closed_app_lists_no_addresses` and `a_close_on_an_unserved_app_drops_its_server`
and batch 42's fourth, each of which reads the list while the app is still held.

### S5. A server's drop can run where a `close` future drops

Decision 2. A `close` abandoned last, after the app's drop, drops the servers on the thread
dropping it: an aborted task's worker, or a runtime shutting down and dropping its tasks. The
shipped servers' drops abort tasks (`TaskSet`, transports 47) and, on NATS, give up a hold on the
roots' load; none awaits or blocks, read.

### S6. The designer's three questions

- **Loosens a declared capability:** no. No capability changes, and the change only adds the case
  where every server's `close` runs.
- **Changes a public signature:** no. The fix is crate-private; doc text on `App`, `AppHandle` and
  `AppHandle::close` changes. S2 names the one signature change that would close the remaining
  window, not made.
- **Departs from a §3 principle** (`DESIGNER_HANDOFF.md` §3): this is a core lifecycle change, to
  when a shutdown's servers are released. It follows §3.1: a `close` answering `Ok` with no server
  closed is the silent pass it removes. It keeps §3.3: the core adds a `std::sync::Mutex` and no
  dependency, and the runtime-levels check passes. It keeps §3.7: every release still goes through
  the one `release_servers`. No principle is departed from. The mechanism departs from the
  response's wording (S1), and S3 is a hold the drop no longer bounds.

## Verification

Full output in the session scratchpad, `batch45/verify/`, with `summary.txt`; one target at a time,
`--locked`, the OpenSSL flags set, in the mirror unless noted. No docker command was run and no
broker suite. The SHA-256 of each of this batch's five source files matched before and after the
mirror's verification (`verify/our-files-start.txt`, `our-files-end.txt`).

- **The crates that keep an `AppHandle`:** `ulo` 1 doctest passed and 5 ignored; `ulo-runtime-tokio`
  47 passed and 1 ignored, `app` 25 of them (batch 42's 22 and these 3); `ulo-rpc` 54 passed and 4
  ignored; `ulo-http` 20 and 9 ignored; `ulo-grpc` 1 and 6 ignored; `ulo-ws` 117 and 4 ignored.
- **The suites on the real servers:** `ulo-rpc-tcp` `conformance`, `conformance_cbor` and
  `conformance_tls` 52 passed and 3 ignored each; `ulo-rpc-tcp-smol`'s three 156 and 9; `ulo-runtime-smol`
  `conformance` 15; `ulo-http-hyper` `conformance` 36 and 2, `conformance_smol` 36 and 2;
  `ulo-ws-hyper` `conformance` and `conformance_smol` 31 each; `ulo-ws` `conformance_smol` and
  `conformance_pipe` 62; the HTTP adapters' `conformance` and `ws_conformance`: axum 69, actix 35 and
  34 ignored, poem 69, rocket 65 and 4 ignored, salvo 70; actix's `conformance_http2` with
  `conformance-http2` 38.
- **The unsafe check:** "self-test: 6 planted lines refused, 2 valid lines accepted", 461 files
  scanned, 2 listed, 0 findings, exit 0.
- **The runtime-levels check,** `scripts/runtime_levels.py --locked`: 50 members, 54 entries, 33
  checked, 0 failures, 0 errors, exit 0.
- `cargo check --workspace --all-targets --all-features --locked`, `cargo check --workspace
  --all-targets --locked`, and `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo
  --exclude ulo-graphql-async-graphql --locked` in `target/msrv-1.88`: exit 0 each, `ulo`'s 17 known
  warnings and no warning located outside `crates/ulo/src`.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo` and
  `ulo-runtime-tokio`: exit 0 each.
- **Clippy,** `cargo +1.98.1 clippy -p ulo -p ulo-rpc -p ulo-runtime-tokio --all-targets
  --all-features --no-deps --locked`: exit 0, run again on the final test file (`clippy-final.txt`).
  Six of its locations fall in the batch's files, each on a line the batch did not change: batch 42's
  five, moved by the new docs (`crates/ulo/src/app/mod.rs:117`, `:239`, `:259`, `:290`,
  `crates/ulo/src/app/shared.rs:299`), and the `Err`-variant size on `AppHandle::load`
  (`crates/ulo/src/app/handle.rs:69`). None is on an added line, and none is in `tests/app.rs`.
- **The workspace test,** in the real tree, `cargo test --workspace --no-fail-fast --locked`: exit 0,
  1,318 passed, 0 failed, 141 ignored, over 207 lines starting `test result:`. It ran on the tree as
  it stood, batches 43's and 44's uncommitted changes to the link crates included, and those
  sessions changed files in it during the run (`verify/real-tree-status-start.txt`,
  `real-tree-status-end.txt`), so its counts mix the three batches; batch 42's were 1,310 and 141
  over 203. The SHA-256 of each of this batch's five source files matched before and after it
  (`verify/ws-our-files-start.txt`, `ws-our-files-end.txt`).
- **The prose:** batch 42's scan for the banned register and for unfilled template braces, asserted
  first against a planted file (it flagged `we`, `just` and the braces), over this log and every
  added line under `crates`: no finding. The scan checks vocabulary only; the text was also read
  against its tests.

## Ledger text for the coordinator

### Fix note, appended under F426

*Appended 2026-10-18:* The residual case batch 42 left, a drop while a `close` on a handle runs the
shutdown elsewhere, is fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 45,
in the core (`ulo`), as the sixty-sixth transports response set out. The `App` value's drop no
longer releases the servers while a `close` is under way: each `close` claims itself at its first
poll, before any await (`crates/ulo/src/lifecycle/shutdown.rs:144`), in a count kept beside a flag
the drop sets, both under one `std::sync::Mutex` (`:57`, `:63`); the drop (`app_dropped`, `:177`,
called from `crates/ulo/src/app/mod.rs:161`) sets the flag and releases only when the count is
zero, and the last `close` to end or drop after the drop releases (`:162`). A `close` that reaches
its close step closes every server and releases them there (`:224`); one dropped before that step,
the app gone and no other under way, releases them unclosed as it drops. The phase alone was
probed and does not mark it: it reads `Stopping` for a shutdown whose runner was dropped, and a drop
that read it kept the servers for the life of the last handle (batch 45's `probes/p01-*-0.txt`). On
`d2e6b26a` a drop inside the before-shutdown hook dropped both of two servers at once, and a drop
during the drain left the close step an empty list (`probes/p00-*-0.txt`); with the fix every
server's `close` ran, then the servers dropped, then the store with the last handle. Held by
`an_app_dropped_before_a_running_close_s_close_step_leaves_it_every_server`,
`an_app_dropped_during_a_running_close_s_drain_leaves_it_every_server` and
`a_close_abandoned_after_the_app_s_drop_releases_the_servers` (`crates/ulo-runtime-tokio/tests/app.rs:862`,
`:885`, `:906`), beside batch 42's four. With the drop releasing unconditionally the three fail
(`broken/b01-*`); with the count never cleared the third and batch 42's
`an_app_whose_shutdown_is_abandoned_drops_its_server` fail (`b02-*`); with the drop unwrapping a
poisoned list, batch 42's `a_closed_app_s_drop_and_a_later_close_release_nothing_again` fails
(`b03-*`). One limit remains: a `close` future made before the drop and first polled after it is a
`close` after the drop and closes no transport, and a `close` polled once and then kept unpolled
holds the servers while it is kept. Verified against `d2e6b26a` plus batch 45's tree.
