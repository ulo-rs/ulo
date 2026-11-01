# Divergences: race 2b, tests batch 40, a closed app lets go of its servers (F425), and the deadline-stamp test on a stepped clock (F424)

Two defects, both of classes already decided. F425's cause is in the core, `ulo`: the app holds
every bound server in `AppShared::servers` for as long as its shared state lives, and every shipped
transport's server keeps the `AppHandle` its `prepare` is handed, so the two hold each other and
neither drops. Probed without a broker: a handler-less server keeping its handle, served and closed
with every handle dropped, never dropped, nor did the app's singleton; a server keeping none dropped
with the last handle. `ulo_rpc::Server` over the TCP link did the same, so every RPC link leaked with
its app's whole singleton store, and by reading so did every server of the HTTP, gRPC and WebSocket
transports; NATS's roots file is where it showed. The shutdown's close step now lets go of the servers once every `close` has run, so
a server drops when `serve` returns and the app with its last handle. Three tests hold it, in the
core, in `ulo-rpc` over TCP and on NATS through the loader seam, where a closed server app's roots
file is gone once `serve` returns; with the release removed, all three fail. F424 was the test's:
it now runs on a clock that moves only while the client connects, 2 ms of a 2.5 ms deadline, so the
frame carries exactly 500 µs, sent as `1`. The whole unit binary passed 1,000 of 1,000 runs and
1,000 of 1,000 beside twelve CPU-bound processes, and the test fails on `2` with the stamp read
before the connect and on `0` with the rounding broken. Two gaps are new: F426, an app bound and
dropped without being served or closed, which keeps its servers and its sockets; and F427, the NATS
conformance suite failing on macOS in 18 of 25 reruns after it passed in the verification, up to 50
scenarios a run, which passes in 6 s with the link's platform trust-store load replaced, batch 39
having given every `nats://` link one.

Files changed: for F425, `crates/ulo/src/lifecycle/shutdown.rs`, `src/app/shared.rs` and
`src/app/mod.rs`, the tests in `crates/ulo-runtime-tokio/tests/app.rs` and
`crates/ulo-rpc-nats/src/link.rs`, and a new test file `crates/ulo-rpc/tests/closed_app.rs`; for
F424, `crates/ulo-rpc/src/client.rs`'s tests. `Cargo.lock` is unchanged. In the workspace's
`FRAMEWORK_GAPS.md`, fix notes under F425 and F424, their commit written `<commit>`, and F426 and
F427 new. The batch started from `668038d2` and made no commit. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

No public signature changes.

```rust
// crates/ulo/src/lifecycle/shutdown.rs, crate-private
fn release_servers(shared: &AppShared);   // new: called once the close step's last `close` has run
```

What changes in meaning without a signature:

- Once the shutdown's close step has run every `close`, the app holds none of its servers. A
  server drops with its last other holder, `serve` returning, or at once when nothing else holds
  it, a `close` on a handle of an app never served; the app's shared state, its singleton store
  among it, drops with its last `AppHandle`. Every server of every shipped transport and every
  singleton of a closed app now drops, where none did.
- `App<Bound>::addresses()` answers nothing once a `close` on a handle has closed the transports.
- A server's drop, and every drop it reaches, a link's among them, runs on the thread that drops
  the last holder: inside `serve`'s future as it returns, or inside the shutdown sequence's.

## Decisions

Line numbers are those of the batch's tree unless a line says otherwise.

### 1. The cause, by probe

Batch 39 left the cause unread. Read in `ulo`: `App::listen` stores every bound server in
`AppShared::servers` (`crates/ulo/src/app/mod.rs:279`), a `Mutex<Vec<Arc<dyn ErasedServer>>>`
(`crates/ulo/src/app/shared.rs:47`) that nothing emptied, and `ErasedServer::prepare` hands each
server `Mounted { app: app.clone(), .. }` (`crates/ulo/src/transport/server.rs:267`). Five servers
keep that handle: `ulo_rpc::Server` in its `Shared` (`crates/ulo-rpc/src/server.rs:161`),
`ulo-http`'s server (`crates/ulo-http/src/server.rs:339`) and embedding
(`crates/ulo-http/src/embed.rs:643`), `ulo-grpc`'s server (`crates/ulo-grpc/src/server.rs:342`)
and `ulo-ws`'s gateway table (`crates/ulo-ws/src/table.rs:240`). Each holds the app and the app
holds each, so a closed app and its servers stay in memory however many handles drop.

Two probes, each a test file copied into the tree for its run and removed after it:

- p01 (`probes/p01-*-0.txt`, source `probes/src/probe_f425_core.rs`): a transport with no
  handlers, a server for it that keeps or does not keep its handle, and a root module whose one
  singleton records its drop, served on an immediate signal and every handle dropped. The server
  keeping its handle never dropped and neither did the singleton; the server keeping none dropped
  once the last handle did, the singleton with it.
- p02 (`probes/p02-*-0.txt`, `probes/src/probe_f425_rpc.rs`): `ulo_rpc::Server` over the TCP link
  on `127.0.0.1:0`, wrapped in a link that delegates every method and records its drop, through the
  same module: neither the link nor the singleton dropped.

The leak is not NATS's: it is the core's hold on its servers, and every RPC link leaked through
`ulo_rpc::Server`, which keeps its handle. A probe shows what, not why: the cycle named here is read,
and the fix's tests are what show that breaking it is enough.

### 2. The fix: the close step lets go of the servers

`sequence` calls `release_servers` (`crates/ulo/src/lifecycle/shutdown.rs:341`) right after
`close_servers` returns (`:169`), which takes the vector out of `AppShared::servers` and drops it
after the lock is released. It runs after the step's last `close`, not inside it: a runner dropped
during the step leaves `Progress::cursor` behind and the next one resumes from it, which needs the
servers still listed, and between `close_servers` returning and the release there is no await. The
module documentation's step 6, `AppShared::servers`' field doc and `App<Bound>::addresses()` say
what changed (`crates/ulo/src/app/mod.rs:330`). With the fix (`probes/p03-*-0.txt`,
`probes/p04-*-0.txt`, the p01 and p02 sources unchanged): both servers, the wrapped TCP link
included, drop once `serve` returns with a handle still held, and the singleton once the last handle
drops. Through a handle's `close` on an app not yet served (`probes/p09-*-0.txt`): the server drops
in the close step, `addresses()` answers an empty list where it answered the server's address before,
and a later `serve` returns `Ok` at once.

Breaking the cycle on the servers' side instead, each holding the app weakly, needs a weak handle
from the core and a change to all five holders, an SPI change across every transport; the release
breaks it once, in the core, for every server (S1). It does not cover an app that is never closed,
decision 3.

### 3. An app bound and dropped without being closed (F426)

The release runs in the close step, so an `App<Bound>` that is dropped without `serve` or `close`
keeps its servers: p05 (`probes/p05-*-0.txt`) dropped the app and its one handle unserved, and the
server keeping its handle and the singleton never dropped; p06 (`probes/p06-*-0.txt`) did the same
with `ulo_rpc::Server` over TCP, and a TCP connect to the address the server bound succeeded after
the drop. Releasing the servers when the `App<Bound>` value drops would leave a handle that closes
the app later with no server to close, and the core cannot count the handles outside the servers
while each server holds one. Filed as F426, not fixed (S2).

### 4. The tests

- `a_closed_app_drops_a_server_holding_its_handle` (`crates/ulo-runtime-tokio/tests/app.rs:395`),
  in the core's app tests, where the existing `Bare` transport is: `Holding`, a server keeping the
  handle its `prepare` is handed (`:350`), and `Flagged`, a module whose singleton records its drop.
  Served on an immediate signal, the server has dropped once `serve` returns, the singleton has not
  while a handle is held, and it has once the handle drops.
- `a_closed_app_drops_its_server_s_link` (`crates/ulo-rpc/tests/closed_app.rs`, new): p02 made a
  test, the TCP link wrapped to record its drop, dropped once `serve` returns. `ulo-rpc` dev-depends on
  `ulo-rpc-tcp` already.
- `plain::a_closed_app_s_server_removes_its_roots` (`crates/ulo-rpc-nats/src/link.rs:1457`), behind
  `integration`, beside the plaintext test in `plain`: a server's link on `nats://` against the
  plaintext NATS container, its roots from the test-certificate loader in slot 15 (`LOADS` grows to 16)
  and written in a directory of the test's own, bound through an app, served from a task and closed
  through a handle. It requires one load and one file after `listen`, and the directory empty once
  `serve` has returned, through `emptied`'s five-second wait, as the other removal tests read it. It
  reads none of the host's store.

With `release_servers` not called (`broken/b01-*.txt`), all three fail: "the server outlived the
closed app's `serve`", "the server's link outlived the closed app's `serve`", and "the roots outlived
the closed app's server" after the five seconds. That run's NATS test left its roots directory,
`ulo-rpc-nats-roots-test-89175-15`, in the temp directory, the test failing before its own cleanup;
removing it was refused by the session's permission check, and it stays with the earlier runs' files
(S3).

### 5. The NATS suite after the fix

In the verification the NATS suite passed, 54 and 1 ignored, and left no roots file in the temp
directory, where batch 39's run left 51. Run again to time it, it failed: the reruns, 25 of them,
failed in 18, 1 to 50 scenarios each, most on the boot check's "the server did not answer within
the boot budget" with its last call failed `Timeout`, "the call timed out". Two runs locate it:

- p14 (`probes/p14-*-0.txt`), the platform's store through `rustls_native_certs::load_native_certs`
  in a test binary: 2.95 s for a process's first load, 163 to 251 ms for the next four, and thirty
  at once ending 353 ms to 2.55 s after they began. Batch 38 measured 77 ms warm.
- p15 (`probes/p15-*-0.txt`), the suite with the link's default loader answering no root, so no
  link reads the store and every `nats://` link connects without a file, three runs: 54 passed each,
  6.0 to 6.5 s, batch 38's time. p16, the next three runs with the store, failed 30, 40 and 49.

Both p15 and p16 ran with the release in place, so the release does not make the suite fail; the
store's load does. Whether the release changes how often it fails was run as six alternating pairs,
the release removed and in place (`broken/b04-*` to `b09-*`, `probes/p17-*` to `p22-*`): removed,
two of six failed, 1 and 40 scenarios; in place, five of six failed, 39, 30, 1, 40 and 40. The
mass failures came in both arms, and six pairs do not settle a rate (S5). The same runs hold F425's
fix at the suite's scale: with the release removed each run left 51 roots files, passing or not,
and with it in place a passing run left none and a failing one about one file for each scenario
that failed, a failed scenario dropping its app unclosed (`probes/roots-left-by-batch40.txt`).
Filed as F427 with the read mechanism, a client's load restarted by each call that gives up first.

### 6. F424: a clock that moves while the client connects

Read: `limit` reads the runtime's clock when the call is built and keeps where the bound ends
(`crates/ulo-rpc/src/client.rs:501`), and `Spec::open` reads it again once `register` has the
connection in hand and stamps what is left (`:556`, `written` at `:593`), decision 129's rule. The
old test gave a fresh `SilentLink` client a 500 µs deadline on the tokio clock, so the connect, the
registration and the write had to take under 500 µs of wall time, and a stream opened on an
instantly made connection measured nothing of the connect. Connecting first with a prior call, the
brief's first route, narrows the window to the registration without closing it, and a preempted
thread can still spend 500 µs there.

`Stepped` (`:1509`) spawns on tokio and keeps its own clock: `now` is its start plus what the test
has moved it, and `sleep` never ends, so no timer the client arms, the stream's bound included, can
end the call before its frame is written. `SlowConnect` (`:1542`) is a `SilentLink` whose `connect`
moves the clock 2 ms before connecting. The test (`:1577`) opens a stream with a 2,500 µs deadline,
so what is left when the frame is handed to the connection is 500 µs exactly and the frame carries
`1`, then one with `1 << 62` seconds on the same connection, which carries `u64::MAX`, as before.
The test now also holds where the stamp is read: read before the connect, the frame would carry `2`.

- The whole unit binary, its 23 tests in parallel as `cargo test` runs them, built once and run
  1,000 times from a copy (`probes/p08-*.txt`, the binary's hash beside it): 1,000 of 1,000 passed.
- The same copy beside twelve `yes` processes, the script's own, started before the first run and
  killed after the last, `ps` then finding none of the twelve (`probes/p10-*.txt`,
  `probes/p10-load-pids.txt`): 1,000 of 1,000 passed.
- With the stamp read before `register` (`broken/b02-*.txt`), the test failed, left `"2"` against
  right `"1"`.
- With `deadline_ms`'s `.max(1)` removed (`broken/b03-*.txt`), the test failed on `"0"`, and so did
  `every_remaining_time_is_written_as_milliseconds_the_server_reads_and_never_as_zero`.

The binary was built before the last edit to `addresses()`'s documentation, a doc comment in `ulo`
that changes no code.

## The tests

- **F425:** `ulo-runtime-tokio`'s `tests/app.rs`, `a_closed_app_drops_a_server_holding_its_handle`;
  `ulo-rpc`'s `tests/closed_app.rs`, `a_closed_app_drops_its_server_s_link`; behind `integration`,
  `ulo-rpc-nats`'s `link::tests::plain::a_closed_app_s_server_removes_its_roots`. Decisions 2 and 4.
- **F424:** `ulo-rpc`'s `client::tests::a_deadline_is_sent_as_milliseconds_the_server_reads` on
  `Stepped` and `SlowConnect`, replacing the test of that name. Decision 6.

### Before and after

The probes and breaks went through `batch40/scripts/brk.py`, batch 39's script with a `create` map
that copies a probe's test file into the tree and removes it after the run, run one spec at a time by
`batch40/scripts/breaks.sh`, which records the container list before and after each. `brk.py`
replaces each span, asserting it occurs once, runs the targets under bash, writes every touched file
and `Cargo.lock` back byte for byte, removes what it created, and compares a SHA-256 over `git diff
HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every
untracked file under `crates` and `scripts`. Specs in `batch40/specs/`, output in `batch40/probes/`
and `batch40/broken/` with `summary.txt` and `runner.txt`. Every restore reported `ok`, and every
container list after a spec matches the one before it. p01 and p02 ran on `668038d2` unchanged, the
tree hash `e3b0c442..`, that of an empty diff; the rest on the batch's tree as it stood at each run.
p08 and p10 ran `batch40/scripts/f424_loop.sh` and `f424_loaded.sh`. p11 to p22 and b04 to b09 ran
after the verification, on its tree. A first run of b02 and b03, whose output filter dropped the
assertion's `left` line, and a first p09, which did not compile, are kept in `batch40/superseded/`.

| Run | Target | Result |
| --- | --- | --- |
| p01. a server keeping its handle and one keeping none, through an app, closed | the core, a handler-less transport | the first and the singleton never dropped; the second dropped with the last handle |
| p02. `ulo_rpc::Server` over the TCP link, closed | `ulo-rpc` | neither the link nor the singleton dropped |
| p03, p04. p01 and p02 with the release | the same | each server and the TCP link dropped when `serve` returned, the singleton with the last handle |
| p05, p06. a bound app dropped unserved and unclosed | the same | the servers and the singleton never dropped; the TCP address still accepted (F426) |
| p07. the three F425 tests with the release | their targets | each passed |
| p08. the F424 test with the stepped clock | the unit binary, 1,000 runs | 1,000 passed |
| p09. `addresses()` after a handle's `close` | the core | the address before, an empty list after, the server dropped |
| p10. the same binary beside twelve CPU-bound processes | 1,000 runs | 1,000 passed |
| p11 to p13. the NATS suite after the verification, ten runs | the NATS suite | eight failed, 1 to 50 scenarios each (F427) |
| p14. the platform's trust store loaded | a test binary | 2.95 s first, 163 to 251 ms after, thirty at once up to 2.55 s |
| p15. the NATS suite, the store's load replaced by no root | three runs | 54 passed each, 6.0 to 6.5 s |
| p16. the NATS suite with the store again | three runs | 30, 40 and 49 failed |
| b04 to b09 and p17 to p22. the release removed and in place, alternating | the NATS suite, six pairs | removed: 2 of 6 failed; in place: 5 of 6 failed (decision 5) |
| b01. the release removed | the three F425 tests | each failed |
| b02. the stamp read before the connect | `ulo-rpc`'s unit tests | the deadline test failed, `"2"` |
| b03. a sub-millisecond remainder not raised to one | `ulo-rpc`'s unit tests | two failed, the deadline test on `"0"` |
| the restored tree | every target above but the NATS suite | every test passed |

## Left for the DESIGN folds

Line numbers are those of `668038d2`; `1a5115e3`, the batch 39 fold, has since changed the
transports DESIGN.

- The core DESIGN, §9.5 step 6 (line 911): once every `close` has run, the app lets go of its
  servers, so a server drops with `serve`'s return or at once, and the app with its last handle; a
  server keeping the `AppHandle` its `prepare` is handed, as every shipped transport's does, held
  the app otherwise (F425). An app bound and dropped without `serve` or `close` keeps its servers,
  F426.
- The transports DESIGN, §2.7 (line 353): `App<Bound>::addresses()` answers nothing once a `close`
  on a handle has closed the transports.
- Decision 139: the roots file's lifetime, the link and every connection that holds it, now ends at
  a server app's clean shutdown, held by `plain::a_closed_app_s_server_removes_its_roots`; before it,
  a server's link bound through an app never dropped (F425).
- Decision 129's test list: `a_deadline_is_sent_as_milliseconds_the_server_reads` holds the stamp's
  point and its rounding on a stepped clock (decision 6).
- Decision 139, and batch 39's S6: on macOS every `nats://` link's store load takes 0.2 to 3 s and
  the NATS suite fails intermittently through it (F427).

## Needs sign-off

Numbered from S1 for this batch.

### S1. The core releases its servers rather than each server holding the app weakly

Decision 2. Five servers keep the handle, and the release breaks the cycle for all of them in one
place, with no change to the `Server` SPI or to `Mounted::app`. Each server holding a weak handle
would not need the close step and would cover F426, at the cost of a weak handle in the core's
public API and an upgrade at every use in five places, a call that finds the app gone included.

### S2. F426 is filed, not fixed

Decision 3. An app bound and dropped without `serve` or `close` keeps its servers, its sockets and
its singletons. Neither releasing at the `App<Bound>` value's drop nor starting the shutdown there
is sound while each server holds a handle; S1's alternative covers it.

### S3. Roots files the batch left in the temp directory

Decisions 4 and 5. b01's NATS test failed before removing its directory, and the session's
permission check refused removing `ulo-rpc-nats-roots-test-89175-15` and its one file, so the batch
removed nothing. The temp directory held 226 roots files before the batch's suite reruns and holds
988 `ulo-rpc-nats-roots-*.pem` files and 25 `ulo-rpc-nats-roots-test-*` directories after them; 762
of the files are this batch's, 306 from the six runs with the release removed, 51 each, and the rest
from the reruns' failed scenarios (`probes/roots-left-by-batch40.txt`, by process and minute).

### S4. The F424 test's clock never runs its timers

Decision 6. `Stepped::sleep` never ends, so the test does not reach the path where the bound ends
during the connect; that refusal is held on every link by the conformance suite's
`a_call_whose_connect_outlasts_its_deadline_is_refused_before_the_server_sees_it`, and the stamp
measured on a connection already made by `a_call_s_deadline_is_measured_once_its_connection_is_made`.
A clock whose sleeps end when it is moved past them would let one unit test hold the refusal too, at
the cost of a timer wheel in a test helper.

### S5. The NATS suite on macOS (F427)

Decision 5. The suite fails on this host whether or not the release is in place, and passes with the
trust-store load replaced. Six alternating pairs failed five times with the release and twice without
it, which is no evidence either way at that count; a longer alternating run would cost 51 roots files
per run with the release removed. Fixing F427, the suite's links on the loader seam or a load that
survives a call giving up, would remove the variance the comparison needs to be read through. CI runs
the suite on Linux, where the store is a file read, and was not checked.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch40/`: `probes/` for the
probes, with `probes/src/` for the probe test files, `broken/` and `specs/` for the breaks,
`suites/` for the RPC suites with `summary.txt` and the container lists before and after each,
`verify/` for the checks, `superseded/` for runs a later one replaced, and `scripts/`. One pass,
`batch40/scripts/verify.sh`, ran every check below on the final tree, its hash `36fd15ab..` read
before and after it (`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`). The ledger and
this log are outside the hash and were written during and after it.

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 52 passed and 3 ignored each (3.25 to 3.26 s);
  the smol TCP link's three 52 and 3 each (3.23 to 3.24 s); UDP 49 and 6 (3.25 s). With
  `--features integration --test conformance` and the OpenSSL flags: NATS 54 and 1 (21.16 s); Redis
  53 and 3 (8.41 s); MQTT 59 and 1 (8.85 s); Kafka 55 and 1 (72.18 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 54 and 1 (29.70 s). Each count is batch 39's. The NATS suite's reruns
  after this pass failed as decision 5 sets out (F427).
- **The link unit tests with `integration`:** NATS 16 passed, one thread at a time, the new
  `plain::a_closed_app_s_server_removes_its_roots` among them; MQTT 4 and Kafka 4, batch 39's.
  **NATS's tests without it:** 11 unit, `conformance` 0 (behind `integration`), `handle` 2,
  `two_providers` 3, 1 doctest ignored.
- **UDP's tests:** `client_address` 3, `client_close` 1, `close_read` 1, the suite 49 and 6 ignored,
  `drain_end` 1, `handle` 2, batch 39's.
- **gRPC's tests:** `ulo-codegen-tests`, `builtins` 6, `clients` 8, `compile_fail` 1, `deadlines` 20,
  `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6; `ulo-grpc` 1 unit test and 6 doctests
  ignored; batch 39's counts.
- **`ulo-rpc`'s own tests:** 23 unit, `bad_deadline` 7, `closed_app` 1 (new), `drain_window` 3,
  `runtime` 11, `stream_end` 5, doctests 2 passed and 4 ignored, the `compile_fail` one;
  **`ulo-rpc-conformance`'s:** 2 unit, `harness` 7, 2 doctests ignored. **The core's:** `ulo` 1
  doctest passed and 5 ignored; `ulo-runtime-tokio` `app` 16, one more than before, `conformance` 15,
  `handle` 6, `own_subscriber` 1, 1 doctest ignored.
- **The WebSocket tests,** run because the release changes when every transport's server drops:
  `ulo-ws` `broadcast` 2, `conformance`, `conformance_pipe` and `conformance_smol` 31 each,
  `handoff` 11, `runtime` 1, `table` 10, 4 doctests ignored; `ulo-ws-hyper` `abandoned_handshake` 1,
  `attributes` 12, `conformance` and `conformance_smol` 31 each, `handshake` 5, `hooks` 8, `limits`
  7, `messages` 9, `prepare` 2, `runtime` 1, `stream_end` 1; `ulo-ws-conformance` 1 unit. All
  passed, batch 39's counts.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 461 files scanned, one more than batch 39, `closed_app.rs`,
  2 listed, 0 findings, exit 0.
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0.
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, both with the OpenSSL flags, and `cargo +1.88 check --workspace
  --all-targets --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql --locked` in
  `target/msrv-1.88`: exit 0 each, `ulo`'s 17 known warnings and no warning located outside
  `crates/ulo/src`.
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags:
  exit 0, 1,300 passed, 0 failed, 141 ignored, over 203 lines starting `test result:`. Batch 39's
  first pass had 1,298 and 141 over 202; the two more are the core's test and `closed_app`'s, whose
  binary is the one more line. The HTTP, GraphQL and gRPC suites, run on servers that keep the
  app's handle too, are in it.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo`, `ulo-rpc`,
  `ulo-rpc-nats` and `ulo-runtime-tokio`: exit 0 each.
- **Clippy against `HEAD`:** `cargo +1.98.1 clippy -p ulo -p ulo-rpc -p ulo-rpc-nats -p
  ulo-runtime-tokio --all-targets --all-features --no-deps --locked` with the OpenSSL flags, on the
  final tree and on `HEAD` exported with `git archive` into `batch40/head-tree` and built in a
  target directory of its own, `HEAD` being by then `1a5115e3`, the batch 39 fold, which changes no
  file under `crates`: exit 0 each, 87 warning or error lines in each and the same 36
  locations by file, counted per file; the batch introduces none.
- **The prose:** a scan for the banned register and for unfilled template braces, asserted first
  against a planted violation (`verify/planted.txt`), over this log, the ledger's new text and every
  added line under `crates`, found none. The scan checks vocabulary only; the text was also read
  against its tests.
- **Containers and images:** at the batch's start twelve containers, the user's, every one exited,
  and 25 images (`batch40/docker_start_containers.txt`, `docker_start_images.txt`). Every suite's,
  probe's and break's container list after it matches its list before, the TCP suite's differing
  only in four exits reported 15 hours ago where they were 14, and the end's lists match the
  start's, the same twelve containers by id and the same 25 images (`verify/docker-end.txt`,
  `verify/docker-images-end.txt`, and again after the reruns, `verify/docker-end-after-reruns.txt`).
  The batch's own containers were the suites' and tests' testcontainers, each removed by its scenario.
  The batch pulled no image, ran no `docker ... prune` and started, stopped or removed nothing of the
  user's. The twelve `yes` processes were the batch's own, killed after p10, `ps` finding none.
- **Disk:** 153 GB free when the pass started, 148 GB at its end (`verify/disk-start.txt`,
  `verify/disk-end.txt`), the `HEAD` export's clippy build among the difference.
