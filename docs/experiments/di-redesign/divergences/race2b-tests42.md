# Divergences: race 2b, tests batch 42, an app's drop lets go of its servers (F426)

F426 is fixed in the core, `ulo`, as the sixty-fifth transports response set out. The `App`
value's hold on the shared state is one `Owner`, made only by `wire` and moved from typestate to
typestate, and its drop calls `release_servers`, the function the close step's end calls. Every
path F426 names drops that value: an `App<Bound>` dropped unserved is it, and `serve`'s future owns
it, whether that future is dropped before any trigger or inside a shutdown it was running. After a
normal close the drop's release takes an empty list. No server's interface changes and no weak
handle is added. Five tests hold it, four in the core's app tests on a server keeping its handle
and a TCP listener, one in `ulo-rpc` over the TCP link; with the drop a no-op, the three F426 tests
and the TCP one fail, and two breaks of the drop's own mechanism each fail the fourth. Two findings
are new. A drop while a `close` on a handle runs the shutdown elsewhere, before its close step,
releases the servers, and that close then closes none (S2). The TCP and UDP RPC links keep their
sockets bound after they drop without `close`, their accept and receive loops being detached tasks
holding the socket: the app's drop now reaches the link, and the link's drop does not reach its
socket (S3, a new ledger entry below).

Files changed: `crates/ulo/src/app/mod.rs`, `src/app/shared.rs` and `src/lifecycle/shutdown.rs`;
the tests in `crates/ulo-runtime-tokio/tests/app.rs` and `crates/ulo-rpc/tests/closed_app.rs`.
`Cargo.lock` is unchanged. The batch started from `460de797`, made no commit and did not edit the
workspace's `FRAMEWORK_GAPS.md`; the F426 fix note and the new entry are at the end of this log for
the coordinator to paste. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

No public signature changes.

```rust
// crates/ulo/src/app/mod.rs, crate-private
pub(crate) struct Owner(Arc<AppShared>);     // new: `App`'s `shared` field; Deref to Arc<AppShared>
impl Drop for Owner { /* release_servers */ }
pub(crate) fn from_shared(shared: Owner, ..) // was Arc<AppShared>

// crates/ulo/src/lifecycle/shutdown.rs
pub(crate) fn release_servers(shared: &AppShared);   // was private
```

What changes in meaning without a signature:

- Dropping an `App` value lets go of the bound servers, as the end of the close step does. For an
  app bound and dropped unserved, one whose `serve` future is dropped, and one whose shutdown was
  abandoned mid-way, each server drops with its last other holder and its `close` does not run.
- An `AppHandle` kept past the drop keeps the shared state and no server: a `close` on it runs the
  shutdown's hooks, answers `Ok` and closes no transport, where before the fix it closed the
  servers the app still listed.
- A drop while a `close` on a handle runs the shutdown elsewhere, before its close step, releases
  the servers, and that close closes none of them; during the close step it closes every one.

## Decisions

Line numbers are those of the batch's tree.

### 1. What the brief asked to verify, by probe

The probes ran through `batch42/scripts/brk.py`, which copies each file it edits aside to
`batch42/aside/` before the edit, copies it back after the runs and `cmp`s the two, and removes any
probe file it created; every restore reported `identical` and every created file `gone`.

- **(a) `App` had no `Drop`.** `p02-*-0.txt`, on `460de797` unchanged: the four `Drop` impls under
  `crates/ulo/src`, none on `App` or `AppShared`.
- **(b) The moves out of `self`.** p01 (`p01-*-0.txt`) added an empty `impl<S> Drop for App<S>`: the
  core then fails to compile with three `E0509`s, at `connect`'s, `listen`'s and `serve`'s move of
  `self.shared`. A `Drop` on `App` itself would need each of the three to clone the `Arc` instead,
  and `listen` would then release the servers it has stored when the `App<Connected>` it consumed
  drops at its return, unless the old value were dropped before the store. The drop sits instead on
  the field's type, `Owner` (decision 2), which the three move unchanged, so a typestate change is a
  move and never a drop. `serve` moves the hold into its future: dropped, the future drops it; run to
  its end, the hold drops at the return, after the close step has emptied the list, and the second
  release takes an empty `Vec`. The drop takes a `std::sync::Mutex` and does not wait on anything
  asynchronous; it runs no `close` and no part of the shutdown sequence.
- **(c) A handle outliving the app.** On the unchanged tree (`p02-*-1.txt`) an `App<Bound>` dropped
  unserved with a handle kept left the server alive and its address accepting, and the handle's
  later `close` ran the before-shutdown hook, closed the server once and dropped it, `Ok`; a second
  `close` answered the same outcome. With the fix (`p03-*-0.txt`) the server dropped at the app's drop,
  its address refusing, and the handle's later `close` ran the hook, closed nothing and answered `Ok`,
  a second `close` the same outcome, and the singleton dropped with the handle. A later `close` meets
  an empty list in the drain and in the close step, and `close_servers`' loop stops at once on it.
- **A drop during another runner's shutdown**, not asked, found while reading `close_servers`: with
  a handle's `close` in another task waiting in a before-shutdown hook, the app's drop released the
  server at once, and that close closed nothing (`p03-*-0.txt`, before the fix it closed the server,
  `p02-*-1.txt`). With the handle's `close` waiting inside the server's own `close` instead
  (`p04-*-0.txt`), the drop left the server alive, since `close_servers` holds its own copy of the
  list, and the close finished and released it. S2.

### 2. The fix: the `App` value's hold releases the servers as it drops

`App`'s `shared` field is an `Owner` (`crates/ulo/src/app/mod.rs:139`) wrapping the
`Arc<AppShared>`, with `Deref` to it, so every use reads as before. Only `wire` makes one (`:235`);
`connect` (`:249`), `listen` (`:314`) and `serve` (`:380`) move it. Its `Drop` (`:149`) calls
`release_servers` (`crates/ulo/src/lifecycle/shutdown.rs:343`), which takes the list out under the
lock and drops it after the lock is released, as the close step's call at `:170` does. The release
neither awaits nor waits on another task. A server's drop, and every drop it reaches, runs on the
thread dropping the app; the shipped servers' drops reach `TaskSet`'s abort and, for NATS, a file
removal, and Kafka's link moves its flushing drops to a thread of their own, read.

The drop reaches the lock through `unwrap_or_else(PoisonError::into_inner)`, as every other use
does. The lock can be poisoned: `addresses()` calls each server's `bound` under it, and a `bound`
that panics leaves it poisoned. A drop that unwrapped the lock would then panic, and abort the
process if the drop ran during an unwind, a `serve` future dropped by a panic among them.

`App`'s docs (`:49`), `AppShared::servers`' field doc (`crates/ulo/src/app/shared.rs:45`), the
shutdown module's step 6 and `release_servers`' doc say what changed. `App<Bound>::addresses()`'s
doc is unchanged: it answers for a borrowed app, which has not dropped, and its "empty once the
close step has run" still holds.

### 3. The tests

- `crates/ulo-runtime-tokio/tests/app.rs`, beside batch 40's three: `Listening` (`:482`), a server
  keeping the `AppHandle` its `prepare` is handed and a `std::net::TcpListener` it binds, counting
  its `close`s and drops in `Seen` (`:459`), whose `serve` never returns; `Hooked` (`:530`), a module
  whose singleton records its drop and whose before-shutdown hook counts its runs and, when told,
  never returns. `listening` (`:564`) binds one and requires its address to accept a connection
  first. `assert_released` (`:590`) is what the three F426 paths share: the server dropped once, its
  address refusing a connection, its `close` never run, the singleton alive while a handle is held
  and dropped with it. `refuses` (`:584`) requires `ConnectionRefused`: p06 (`p06-*-0.txt`) held it
  false on an open listener and on one whose backlog was full, an attempt answered with a reset on
  this host, and true on a dropped one.
  - `a_bound_app_dropped_unserved_drops_its_server` (`:601`): bound, a handle kept, dropped.
  - `an_app_whose_serve_is_dropped_drops_its_server` (`:611`): `serve` on a signal that never
    resolves, dropped by a 50 ms timeout, the hook never run.
  - `an_app_whose_shutdown_is_abandoned_drops_its_server` (`:623`): `serve` on an immediate signal,
    dropped by a 200 ms timeout inside the before-shutdown hook that never returns, and never
    resumed; the hook ran once.
  - `a_closed_app_s_drop_and_a_later_close_release_nothing_again` (`:636`): `bound` panics once
    inside `addresses()`, caught, which poisons the list's lock; a handle's `close` closes and
    releases the server once; the app's drop closes and releases nothing more and does not panic; a
    later `close` on the handle answers the first outcome's signal, runs the hook and the server's
    `close` no more, and the singleton drops with the handle.
- `crates/ulo-rpc/tests/closed_app.rs`, `a_bound_app_dropped_unserved_drops_its_server_s_link`
  (`:107`): batch 40's wrapped TCP link under `ulo_rpc::Server`, bound and dropped unserved, the link
  dropped at the app's drop and the singleton with the last handle. It reads no socket, and a
  comment says why: the TCP link's listener outlives the link (decision 4).

### 4. The TCP and UDP links' sockets outlive the link

The RPC test first required the link's address to refuse a connection after the drop, and failed
on it. p05 (`p05-*-0.txt`): the link dropped at the app's drop and the singleton at the last
handle's, and a connection to the address succeeded before the drop, between the two drops, and
after 10 ms of yielding. p08 (`p08-*-0.txt`) and p07 (`p07-*-0.txt`) rebound the address: the TCP
link's and the UDP link's each answered `AddrInUse` while bound, right after the app's drop and
200 ms later. Read: `Core::listen` (`crates/ulo-rpc-tcp-core/src/link.rs:212`) spawns the accept loop
on the link's runtime holding the listener and keeps its `TaskHandle` in the link's shared `State`,
which that loop and the forwarding task each hold too; `close` takes the handle and awaits it
(`:297`). A `TaskHandle` detaches on drop (decision 47), so a link dropped without `close` leaves the
loop accepting on its socket until the process ends. The UDP link keeps its receive loop's tokio
`JoinHandle` the same way (`crates/ulo-rpc-udp/src/link.rs:193`), read. `ulo-rpc-tcp-smol` shares
the core, read, not probed. Before the fix the links never dropped on these paths, so this was
hidden behind F426. A new ledger entry, below; not fixed (S3). The core test's own server holds its
listener directly, which is why it reads the socket and the RPC test does not.

The RPC test's first run, outside the scripts and its output not kept, failed on that check, "the
link's address still accepts a connection after the app's drop", which p05, p07 and p08 then
located. p05's two earlier runs, one that did not compile and one without the timed connections,
are in `superseded/`.

## The tests

- **F426:** `ulo-runtime-tokio`'s `tests/app.rs`: `a_bound_app_dropped_unserved_drops_its_server`,
  `an_app_whose_serve_is_dropped_drops_its_server`, `an_app_whose_shutdown_is_abandoned_drops_its_server`
  and `a_closed_app_s_drop_and_a_later_close_release_nothing_again`; `ulo-rpc`'s
  `tests/closed_app.rs`, `a_bound_app_dropped_unserved_drops_its_server_s_link`. Decisions 2 and 3.

### Before and after

Specs in `batch42/specs/`, output in `batch42/probes/` and `batch42/broken/`, each with
`summary.txt` recording every `cmp` and removal. The breaks ran `cargo test -p ulo-runtime-tokio
--test app` and `cargo test -p ulo-rpc --test closed_app`. Another session was building batch 41 in
`crates/ulo-rpc-nats` throughout; a break of the core was in the tree for its run alone, and any
build of theirs in that window compiled it.

| Run | Target | Result |
| --- | --- | --- |
| p01. an empty `impl<S> Drop for App<S>` | `cargo check -p ulo` | three `E0509`, at `connect`, `listen` and `serve` |
| p02. the unchanged tree: `Drop` impls; the three probes | `ulo` source; `ulo-runtime-tokio` | no `Drop` on `App`; an app dropped unserved kept its server and its address accepting, a handle's later `close` closed it |
| p03. the three probes with the fix | `ulo-runtime-tokio` | the server dropped at the app's drop; a later `close` closed nothing, `Ok`; a drop during another runner's before-shutdown hook released the server unclosed |
| p04. a drop while the server's own `close` runs | `ulo-runtime-tokio` | the server kept until that close ended, then dropped |
| p05. the TCP link through an app dropped unserved, with the fix | `ulo-rpc` | the link and the singleton dropped; the address still accepted |
| p06. `refuses` on open, full-backlog and dropped listeners | a test binary | false, false (a reset), true |
| p07, p08. the UDP and TCP links' addresses rebound after the drop | `ulo-rpc-udp`, `ulo-rpc` | `AddrInUse` bound, right after the drop and 200 ms later |
| b01. the drop releases nothing | both targets | the three F426 core tests and the TCP test failed, "the server outlived …", "the server's link outlived the app dropped unserved"; the fourth and batch 40's passed |
| b02. `listen` makes a second hold, dropping the consumed value's after storing the servers | both targets | six core tests failed, the fourth among them (the server dropped at `listen`, its address refused before the test's first step), and batch 40's `a_closed_app_lists_no_addresses` and `a_close_on_an_unserved_app_drops_its_server`; the RPC tests passed |
| b03. the drop's lock unwrapped with `expect` | both targets | the fourth failed, "the app's list of servers is poisoned", at the app's drop; all else passed |
| b04. the close step's release removed, the drop kept | both targets | batch 40's `a_closed_app_lists_no_addresses` and `a_close_on_an_unserved_app_drops_its_server` and the fourth failed; batch 40's `a_closed_app_drops_a_server_holding_its_handle` and `a_closed_app_drops_its_server_s_link` passed (S4) |
| the restored tree | every target in the verification | every test passed |

## Left for the DESIGN folds

Line numbers are those of `460de797`.

- The core DESIGN, §9.5 step 6 (line 911): the last sentence, "An app that never reaches this step
  … keeps its servers, their sockets and its singletons (F426, open)", is false now. The `App`
  value's drop lets go of the servers as the step's end does; for an app bound and dropped unserved,
  one whose `serve` future is dropped and one whose shutdown is abandoned, that drop is each
  server's shutdown, its `close` not run, and an `AppHandle` kept past it keeps the shared state and
  no server, a `close` on it running the hooks and closing no transport. A drop while a handle's
  `close` runs the shutdown elsewhere, before the close step, releases the servers unclosed (S2).
- The transports DESIGN, decision 146 (line 1672): its two F426 clauses, "An app that never reaches
  the close step keeps its servers … (F426, open)" and "Releasing the servers when the `App<Bound>`
  value drops would leave a handle that closes the app later with no server to close …", and "that
  F426 is filed rather than fixed is pending sign-off": the drop releases, the later `close` closes
  nothing, by the sixty-fifth response. The test list gains this batch's five.
- The transports DESIGN, decision 47's consequence for links: a link whose socket lives in a task it
  spawned and detached keeps that socket after the link drops without `close` (decision 4, S3).

## Needs sign-off

Numbered from S1 for this batch.

### S1. The drop is on the field's type, not an `impl Drop for App<S>`

Decision 1(b) and 2. `impl<S> Drop for App<S>` refuses the three moves of `self.shared`, and with
clones in their place `listen` would release the servers it stored as the consumed
`App<Connected>` drops, unless the order inside `listen` kept that value's drop ahead of the store.
`Owner` gives one hold per app, moved by each typestate change, with no ordering to keep. b02 is what
a second hold would do. The behaviour is the same as the response's `App`'s `Drop`: dropping any
`App` value runs it.

### S2. A drop during another runner's shutdown releases the servers unclosed

Decision 1. A `close` on a handle running in another task, before its close step, finds an empty
list once the app's value drops, so the servers drop without their `close`; during the close step it
closes all of them. The response's design is followed: the drop releases whatever runs elsewhere.
Leaving the servers to a live runner would need the drop to tell a runner that is live from one
dropped and never resumed: the drop could leave the servers when the sequence's progress lock is
held, and the sequence release them on its own drop once the app's value is gone, which is a flag in
the shared state and an ordering between the two. It is documented on `App`.

### S3. The TCP and UDP RPC links' sockets outlive the link (new ledger entry)

Decision 4. Filed, not fixed: the fix is in the links, outside the core, and touches decision 47.
Candidate fixes: each link's shared state aborting its loops on the link's drop, a `Drop` on the
link that takes the handles out of the state the loops also hold; or the loops holding the listener
or socket through a weak reference the link owns.

### S4. Two of batch 40's tests no longer distinguish the close step's release

b04. With the close step's release removed and the drop kept, `a_closed_app_drops_a_server_holding_its_handle`
and `a_closed_app_drops_its_server_s_link` pass: `serve` drops the app's value as it returns, and
the drop releases the servers. The close step's release stays held by `a_closed_app_lists_no_addresses`,
`a_close_on_an_unserved_app_drops_its_server` and this batch's fourth test, each of which keeps the
app's value alive past the close. The two are kept unchanged; their claim, the server gone once
`serve` returns, still holds.

### S5. The fourth test's breaks are of the drop's mechanism, not of a second release

A second release is a `mem::take` of an empty list and cannot drop a server twice: no break of the
release itself fails the fourth test while the other three pass. The two guards it holds are the one
hold per app (b02) and the drop's tolerance of a poisoned lock (b03), each failing it.

## Verification

Full output in the session scratchpad, `batch42/verify/`, with `summary.txt`; one target at a time,
`--locked`, the OpenSSL flags set. No docker command was run and no broker suite.

- **The crates that keep an `AppHandle`:** `ulo` 1 doctest passed and 5 ignored; `ulo-runtime-tokio`
  `app` 22 (batch 40's 18 and these 4), `conformance` 15, `handle` 6, `own_subscriber` 1, 1 doctest
  ignored; `ulo-rpc` 23 unit, `bad_deadline` 7, `closed_app` 2, `drain_window` 3, `runtime` 11,
  `stream_end` 5, doctests 2 passed and 4 ignored and the `compile_fail` one; `ulo-http`
  `max_inflight` 2, `post_deadline_body` 7, `stream_end` 10, `upgraded` 1, 9 doctests ignored;
  `ulo-grpc` 1 unit, 6 doctests ignored; `ulo-ws` `broadcast` 2, `conformance`, `conformance_pipe`
  and `conformance_smol` 31 each, `handoff` 11, `runtime` 1, `table` 10, 4 doctests ignored.
- **The suites on the real servers:** `ulo-rpc-tcp` `conformance`, `conformance_cbor` and
  `conformance_tls` 52 passed and 3 ignored each; `ulo-rpc-tcp-smol`'s three 52 and 3 each;
  `ulo-runtime-smol` `conformance` 15; `ulo-http-hyper` `conformance` and `conformance_smol` 36 and 2
  each; `ulo-ws-hyper` `conformance` and `conformance_smol` 31 each; the HTTP adapters' `conformance`
  and `ws_conformance`: axum 38 and 31, actix 34 and 4 ignored and 1 and 30 ignored, poem 38 and 31,
  rocket 36 and 2 ignored and 29 and 2 ignored, salvo 39 and 31; actix's `conformance_http2` with
  `conformance-http2` 38.
- **The unsafe check:** "self-test: 6 planted lines refused, 2 valid lines accepted", 461 files
  scanned, 2 listed, 0 findings, exit 0.
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: 50 members,
  54 entries, 33 checked, 0 failures, 0 errors, exit 0.
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, and `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo
  --exclude ulo-graphql-async-graphql --locked` in `target/msrv-1.88`: exit 0 each, `ulo`'s 17 known
  warnings and no warning located outside `crates/ulo/src`.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo`, `ulo-rpc` and
  `ulo-runtime-tokio`: exit 0 each.
- **Clippy,** `cargo +1.98.1 clippy -p ulo -p ulo-rpc -p ulo-runtime-tokio --all-targets
  --all-features --no-deps --locked`: exit 0. Five of its locations fall in the batch's files, each on
  a line the batch did not change: `AppConfig::defaults` (`crates/ulo/src/app/mod.rs:105`), the
  `Err`-variant size on `wire`, `connect` and `listen` (`:228`, `:248`, `:279`), and a collapsible `if`
  at `crates/ulo/src/app/shared.rs:297`. None is on an added line.
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked`: exit 0, 1,310 passed,
  0 failed, 141 ignored, over 203 lines starting `test result:` (re-measured on the same tree: the 205 first written here
  was a miscount, and batch 41's log has 203). It ran on the tree as it stood,
  batch 41's uncommitted changes to `crates/ulo-rpc-nats` included, so its counts mix the two
  batches; batch 40's were 1,300 and 141 over 203, and this batch adds five tests in binaries that
  already existed. The SHA-256 of each of this batch's five source files matched before and after
  the verification (`verify/our-files-start.txt`, `our-files-end.txt`).
- **The prose:** batch 40's scan for the banned register and for unfilled template braces, asserted
  first against a planted violation, over this log and every added line under `crates`. The scan
  checks vocabulary only; the text was also read against its tests.

## Ledger text for the coordinator

### Fix note, appended under F426

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 42, in
the core (`ulo`), as the sixty-fifth transports response set out. The `App` value's hold on the
shared state is one `Owner` (`crates/ulo/src/app/mod.rs:139`), made only by `wire` and moved by
`connect`, `listen` and `serve`, whose drop calls `release_servers`
(`crates/ulo/src/lifecycle/shutdown.rs:343`), the function the close step's end calls (`:170`). Each
path this entry names drops that value: an `App<Bound>` dropped unserved is it, and `serve`'s future
owns it, dropped before any trigger or inside the shutdown it runs. After a normal close the drop's
release takes an empty list. Each server then drops with its last other holder, its `close` not run.
Batch 42's `probes/p02-*-1.txt`, on `460de797`: an app dropped unserved with a handle kept, a server
keeping its handle and a TCP listener did not drop and its address accepted; `probes/p03-*-0.txt`,
with the fix: the server dropped at the app's drop and its address refused, and the handle's later
`close` ran the hooks, closed nothing and answered `Ok`. Held by `a_bound_app_dropped_unserved_drops_its_server`,
`an_app_whose_serve_is_dropped_drops_its_server`, `an_app_whose_shutdown_is_abandoned_drops_its_server`
and `a_closed_app_s_drop_and_a_later_close_release_nothing_again` (`crates/ulo-runtime-tokio/tests/app.rs:601`,
`:611`, `:623`, `:636`), and `a_bound_app_dropped_unserved_drops_its_server_s_link`
(`crates/ulo-rpc/tests/closed_app.rs:107`), `ulo_rpc::Server` over the TCP link. With the drop a
no-op the first three and the TCP test fail (`broken/b01-*`); with `listen` making a second hold
(`b02-*`), or the drop unwrapping a lock a panicking `bound` poisoned (`b03-*`), the fourth fails.
Two limits remain: a drop while a `close` on a handle runs the shutdown elsewhere, before its close
step, releases the servers and that close closes none (`probes/p03-*-0.txt`); and a server whose
socket lives in a task it detached keeps the socket after its drop, the TCP and UDP RPC links among
them (the entry below).

### New entry

**F4xx — the TCP and UDP RPC links keep their sockets bound after the link drops without `close`, so an app dropped unserved, or a scenario that fails before its close, leaves its RPC address accepting connections or receiving datagrams for the rest of the process.** Observed 2026-10-18 by race 2b's test batch 42, by a test, once F426's fix let the link drop.
On `experiment/di-redesign` at `460de797` plus batch 42's tree. `Core::listen` (`crates/ulo-rpc-tcp-core/src/link.rs:212`) spawns the accept loop on the link's runtime holding the listener and keeps its `TaskHandle` in the link's shared `State`, which the accept loop and the forwarding task each hold too; only `close` (`:297`) takes and awaits it, and a `TaskHandle` detaches on drop (transports decision 47), so nothing ends the loop when the link drops. The UDP link keeps its receive loop's tokio `JoinHandle` in its state the same way (`crates/ulo-rpc-udp/src/link.rs:193`), read; `ulo-rpc-tcp-smol` shares `ulo-rpc-tcp-core`, read, not probed. Batch 42's `probes/p05-*-0.txt`: `ulo_rpc::Server` over the TCP link wrapped to record its drop, bound through an app dropped unserved: the link and the app's singleton dropped, and a connection to the bound address succeeded before the drop, between the app's drop and the last handle's, and after 10 ms of yielding. `probes/p08-*-0.txt` and `p07-*-0.txt`: binding the TCP link's and the UDP link's address answered `AddrInUse` while bound, right after the app's drop and 200 ms later. What a caller sees: the port stays taken, connections to it are accepted, probed, and a call sent on one reaches no server, read. Before F426's fix the link never dropped on these paths, which hid this. Candidate fixes: a `Drop` on each link taking its loops' handles out of the state they share and aborting them; or the loops holding the socket through a reference the link alone owns. The core's own probe server, holding its listener directly, released its socket at the app's drop. Predates batch 42. Verified against `460de797` plus batch 42's tree.
