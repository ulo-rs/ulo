# Divergences: race 2b, tests batch 43, a NATS server that waits for its roots under `roots_timeout`, and NATS links whose tasks stop when they drop (F423, F428)

The sixty-sixth response, signed off 2026-10-18, answers `race2b-tests41.md` and
`race2b-tests42.md`. Its point 1 changes decision 148 for servers, and its point 2 asks whether the
broker links hold F428's shape. Both are built here for the NATS link. A server's `listen` waits
for the roots' load, on `nats://` and `tls://`, and so does a `tls://` client's connect, each under
a new setting, `Nats::roots_timeout(Bound)`, 30 s at `Bound::Default`. On expiry a `nats://` server
connects without the file and logs one `warn`, and a `tls://` link is refused `Misconfigured`. A
zero is refused in `prepare` with `zero_bound`'s hint. A `nats://` client still waits for nothing.
F428's premise holds on NATS, by probe: a server's link bound through an app dropped unserved, a
server's link listened directly and a client's link each kept their connection to the broker for
the three seconds watched after the drop. The server's lanes and each connection's reply router are
now held through an `AbortOnDrop` in the link's state, and a server side's drop clears its held
calls, whose reply paths otherwise hold the table they sit in and the connection with it. Six new
tests hold the two items, and each fails against a break; the boot-budget test is rebuilt for the
server's wait. The NATS conformance suite passed 12 of 12 runs, 6.64 to 15.52 s.

Files changed: `crates/ulo-rpc-nats/src/link.rs`, `src/lib.rs` and `Cargo.toml` (the `test-util`
feature on the tokio dev-dependency). `Cargo.lock` is unchanged. The batch started from
`d2e6b26a`, made no commit and did not edit `FRAMEWORK_GAPS.md`; the ledger's text is in this log's
last section for the coordinator to paste, its commit written `<commit>`. Versions are unchanged at
`0.1.0-alpha.1`.

## The signatures

One public method is added:

```rust
// crates/ulo-rpc-nats/src/link.rs
impl Nats {
    pub fn roots_timeout(self, timeout: Bound) -> Self;   // new; `ulo::Bound`
}
```

Inside `ulo-rpc-nats`:

```rust
// crate-private
pub(crate) roots_timeout: Bound,                        // new field of `Nats`
struct State { .., tasks: Vec<AbortOnDrop> }            // new field
struct AbortOnDrop(AbortHandle);                        // new
const ROOTS_TIMEOUT: Duration = Duration::from_secs(30);
impl Drop for ServerSide { .. }                         // new: clears the held calls
async fn roots(&self, servers: &[ServerAddr], runtime: &Tokio, waits: bool) -> ..;   // `waits` added
async fn Roots::wait(&self, runtime: &Tokio, tls: bool, bound: Bound) -> ..;          // `bound` added
async fn Roots::awaited(&self, runtime: &Tokio, bound: Bound) -> Option<Arc<Load>>;   // new
async fn within<F: Future>(runtime: &Tokio, bound: Bound, fut: F) -> Result<F::Output, Duration>; // new
```

What changes in meaning without a signature:

- A `nats://` server's `listen` waits for the load, at most `roots_timeout`, where it waited for
  nothing. A `tls://` server's `prepare` and `listen` and a `tls://` client's connect wait as
  before, now under the bound.
- A link dropped without `close` aborts its server lanes and reply routers, and its connections
  close; before, each stayed open for the life of the process.

## Decisions

Line numbers are those of the batch's tree.

### 1. F428's premise on NATS, by probe

p01 (`probes/p01-0.txt`), against a plaintext `nats:2.10.14` with its monitoring port, counting the
connections `/connz` reports, at `d2e6b26a`: a server's link bound through an app, the app dropped
unserved; a server's link prepared and listened directly, the link and its inbound stream dropped;
a client's link connected, the link and what `connect` answered dropped. After each drop the count
stayed where it was at 1.9 ms, 54 ms, 205 ms, 505 ms, 1 s, 2 s and 3 s: 1, then 2, then 3 connections.
The F426 fix had the app's drop reach the server's link, so its connection outlived the link.

The holders, read: `listen` spawned one `request_lane` per pattern and one `control_lane`, each
holding `Arc<ServerSide>`, and so the side's `Client`, and its `Subscriber`; their `JoinHandle`s
were dropped at the spawn, which detaches. A lane ends when its subscription ends or its phase reads
`Closed`, and only `close` sets that. `connect` spawned `route_replies` holding `Arc<ClientSide>`
and the inbox `Subscriber`; it ends on a disconnect, the subscription's end, or a frame sent to a
dropped receiver, so an idle client never ends it. async-nats keeps a connection open while any
`Client` or `Subscriber` lives.

### 2. The tasks are held through `AbortOnDrop`

`State::tasks` (`crates/ulo-rpc-nats/src/link.rs:123`) holds an `AbortOnDrop` (`:135`) for each lane
`listen` spawns (`:322`-`:325`) and for each connection's router (`:378`-`:381`). `State::hold`
(`:128`) lets go of tasks that have ended before it adds one, so the routers of a client's earlier
connections, which end with their reply lane, do not pile up. The state drops with the link, which
aborts every task still running; their sides and subscribers drop, and async-nats closes the
connection. `close` is unchanged: it ends the tasks as before, and their handles stay in the state
until the link drops, an abort of an ended task doing nothing (S6). MQTT's `AbortOnDrop` (decision
82) is disarmable and guards a loop until the broker answers; this one is never disarmed, since the
NATS tasks are to end with the link, as a `TaskSet`'s are with its holder (decision 47). It is the
link's own type, MQTT's being private to its crate.

### 3. A server side's drop clears its held calls

A held call's `ReplyPath` (`ServerSide::hold`) holds the connection's `Client` and the `Calls` table
it is held in, and the table holds the path: a cycle that lives until the call ends or `close`
clears the table. With the lanes aborted, a server's link dropped holding a call still kept its
connection, by test (decision 7, b11). `impl Drop for ServerSide` (`:467`) clears the table. A path
the core still holds keeps working, since it holds the client and the table itself; what is cleared
is the table's own copy, which only the lanes, now gone, read (S7).

### 4. `roots_timeout`

`Nats::roots_timeout` (`:208`) sets a `Bound`, `Default` in `Nats::url`. `within` (`:1251`) waits
for a future under it: `ROOTS_TIMEOUT` (`:150`), 30 s, at `Default`, the duration at `After`, no
bound at `Unbounded`. It selects the future against `ulo::Timer::sleep` on the link's `Tokio`, which
enters the runtime to make the sleep, so the wait is timed wherever the link's futures are polled
and `tokio`'s `time` feature is not added to the dependency. The future is polled first, so a load
that has answered is taken under any bound. The bound is the link's own and its clock the link's
runtime, not the app's `Timer` (S2).

`prepare` (`:255`) refuses `Bound::After(Duration::ZERO)` before it reads the url, through
`ulo_transport::prepare::zero_bound` in a `Failures` answered as its `PrepareError`, so a server's
`bind` fails `StartupError::Configure` with "`.roots_timeout(Bound::After(Duration::ZERO))` would
refuse every `tls://` link and connect every `nats://` server without its root certificates; write
`Bound::Unbounded` to turn the timeout off". A client runs no `prepare`, so a client's zero is not
refused (S1).

### 5. A server waits for the load; a `nats://` client does not

`Nats::roots` (`:225`) takes `waits`, set by `listen` (`:285`-`:287`) alone. On a url naming a
`tls://` server, `Roots::wait` (`:1081`) resolves the provider, as before, and waits under the bound
in `prepare`, `listen` and a client's connect; an expiry is `Misconfigured`, "the NATS link could
not load the platform's root certificates for `tls://`: the load did not end within
`roots_timeout`'s 200ms; raise it with `.roots_timeout(..)`, or write `Bound::Unbounded` to wait for
as long as the load takes", which a server's `prepare` returns as `Configure` and the client maps
to `Internal` with `reason: "misconfigured_link"`. On a url naming none, `listen` calls
`Roots::awaited` (`:1104`): the load once written; on a failed load the per-load `warn`, as
`Roots::ready` gives it, and no file; on expiry one `warn` naming `roots_timeout` and saying that
an upgraded connection reads the platform's store on the thread polling it, at that connect and at
each reconnect, and no file. The server's side then holds no load. `prepare` on `nats://` and every
client connect on `nats://` call `Roots::ready`, which starts or joins the load and waits for
nothing, as decision 148 has it.

A load outlasting the bound keeps running; the bound ends the wait, not the load, and a later wait,
or a client's later connect, joins it (decision 147's shared answer).

### 6. Reconnects keep the options a connection was made with

The response corrects the sixty-fifth's "later connects and reconnects use the file": async-nats's
`Connector` keeps the `ConnectorOptions` built from the `ConnectOptions` the connection was made
with (async-nats 0.46.0 `src/connector.rs:55`-`:57`, `:75`-`:78`), and no public method changes
them, read, as batch 41 read it. The crate's code already said a connection made without the file
keeps none at its reconnects (batch 41); what the batch changes is the `Nats` documentation, which
now gives that as the reason a server waits, the crate documentation and `roots_timeout`'s, and the
comments of `listen` and `connect` (`:285`, `:336`), which say why each side waits or does not. The
wrong claim stands in DESIGN decisions 139 and 144, left for the folds.

### 7. The tests

New:

- `over_tls::a_nats_server_waits_for_its_roots_and_reconnects_with_them` (`:2509`), item 3a,
  behind `integration`: a `nats:2.10.14` requiring TLS from the test certificate, reached through
  the relay; on one thread beside the ticker the server's link, `nats://` through the relay, with a
  load of its own (`Roots::own`) that blocks `PAST_BOOT`, 6 s, past `Budget::default().boot`, 5 s,
  and inside the 30 s default, which the test asserts. `prepare` and `listen` take at least 6 s,
  and the server's side holds the load; a `tls://` client's request reaches the server; the relay's
  cut closes both connections, and requests from the client's later connections are sent until one
  reaches the server, which needs the server's connection to have reconnected and resubscribed.
  Every gap stays under `STALL`, 25 ms, with ticks ending while the server binds and while it
  reconnects; each link loads once. Over three runs of p02 (`probes/p02-*.txt`), `listen` returned
  6.008 to 6.015 s after `prepare` began, the longest gap was 5.8 to 13.2 ms over 1,291 to 1,418
  ticks while the server bound and 5.3 to 5.9 ms while it reconnected. A connection made without the file would have async-nats
  trust the platform's store, which holds no test certificate, and fail `UnknownIssuer`.
- `plain::a_nats_server_whose_roots_outlast_roots_timeout_connects_without_them` (`:1954`), item
  3b's `nats://` half, behind `integration`, against the plaintext container: the server's load
  gated for `HELD`, 10 s, its `roots_timeout` 200 ms. `listen` returns in at least 200 ms and under
  5 s; one `warn` on the test's thread carries the expiry's text, through a process subscriber
  (`capture`, `:2186`, MQTT's shape, S5); the side holds no load; `/connz` reports one connection
  and no `tls_version`; one load.
- `a_tls_link_whose_roots_outlast_roots_timeout_is_refused` (`:2167`), item 3b's `tls://` half, no
  broker: a server's bind refused `Configure` and two client calls `Internal` with
  `misconfigured_link`, each naming `roots_timeout`'s 200 ms, the three under 5 s with each side's
  load gated for 10 s; one load a side, the second call joining the first's.
- `a_zero_roots_timeout_is_refused_at_bind` (`:2151`), item 3c: on `nats://` and on `tls://`, the
  refusal names the setting and `Bound::Unbounded`, and no load starts.
- `roots_timeout_bounds_the_wait_as_its_bound_says` (`:2131`), on a paused clock, the reason for
  `test-util`: `Default` ends the wait at 30 s, `After(5 s)` at 5 s, and `Unbounded` waits an hour
  for a future that takes one.
- `plain::a_link_dropped_without_close_closes_its_connection` (`:1997`), item 4, behind
  `integration`: the connection count `/connz` reports returns to 0 within five seconds after an
  app holding a bound server is dropped unserved; to 1 after a server's link holding a call is
  dropped with its inbound stream and the delivery; to 0 after the client's link and its `Outbound`
  are dropped.

Changed:

- `plain::a_plain_link_connects_within_the_boot_budget_while_its_roots_load` (`:1886`), item 3d:
  the server's link has a load of its own from the test certificate, since a `nats://` server now
  waits for the process's gated load; the client still connects on the process's load gated for
  10 s, and binding, connecting and one request stay under the boot budget, with one process load,
  the client holding no file, and the server's own load once. The assertion that the server held
  the client's file becomes one that it holds another.
- `LOADS`, `GATES` and `TLS_LOADS` widen to 25, 4 and 5 slots; the calls of `Roots::wait` and
  `Nats::roots` in `two_loads_write_files_of_their_own`, `a_plain_link_s_logged_cause_does_not_name_tls`
  and the boot-budget test pass the new argument.

### 8. The breaks

Each went through `batch43/scripts/brk.py` in the mirror (decision 11), the file copied aside before
the edit and compared with `cmp` after the restore, against the mirror's copy and the real tree's,
one spec at a time:

- b01, a `nats://` server not waiting (`listen` passing `waits: false`): the waiting test failed at
  `listen`, "InvalidCertificate(UnknownIssuer)", the connection made before the load ended having
  async-nats read the platform's store (`broken/b01-0.txt`).
- b02, batch 39's planted per-connection load, a `ClientConfig` built from the file handed to
  async-nats, which then reads the platform's store at every connection: the waiting test failed,
  "the test's thread went 392.6 ms without polling the ticker while binding the server"
  (`broken/b02-0.txt`).
- b03, batch 39's planted per-reconnect load, `rustls_native_certs::load_native_certs` in a
  `reconnect_delay_callback` from each connection's second attempt: the waiting test failed, 740.4
  ms "while reconnecting" (`broken/b03-0.txt`).
- b04, the bound ignored (`within` awaiting the future at every bound): the `tls://` expiry test
  failed at 10.02 s, the bind failing "Connection refused" once the gated load ended, not as a
  `Configure` refusal (`broken/b04-0.txt`); the `nats://` expiry test failed, "`listen` returned
  after 10.014805625s, where the bound is 200ms" (`broken/b04-1.txt`).
- b05, the zero accepted (`zero_bound` given `Bound::Default`): the zero test failed, the `nats://`
  bind failing "Connection refused" rather than `Configure` (`broken/b05-0.txt`).
- b06, `Default` at 10 s: the bound test failed on `Err(10s)` (`broken/b06-0.txt`).
- b07, `Unbounded` at 30 s: the bound test failed on `Err(30s)` (`broken/b07-0.txt`).
- b08, the expiry's `warn` written at `debug`: the `nats://` expiry test failed, "the expiry was
  logged other than once: []" (`broken/b08-0.txt`).
- b09, a `nats://` client waiting (`connect` passing `waits: true`): the boot-budget test failed,
  "binding, connecting and one request took 10.024365958s, past the boot budget of 5s"
  (`broken/b09-0.txt`).
- b10, no task held (`State::hold` dropping the handle): the drop test failed, "the server of an
  app dropped unserved kept its connection", 1 (`broken/b10-0.txt`).
- b11, `ServerSide`'s drop clearing nothing: the drop test failed, "the server's link dropped
  holding a call kept its connection", 2 (`broken/b11-0.txt`).
- b12, the client's router alone not held: the drop test failed, "the client's link dropped
  without `close` kept its connection", 1 (`broken/b12-0.txt`).

Every restore reported `ok` with both `cmp`s identical (`broken/summary.txt`).

The planted loads read the host's store, being async-nats's own read; nothing counts them, since
async-nats offers no seam for its read. The test holds them at zero through what they cost: a gap
past 25 ms on the thread polling the connection in either phase, and `UnknownIssuer` for a
connection trusting the platform's store (S4). The seam's own loads are counted, one per link.

### 9. The provider resolution does no I/O (batch 41's S2)

Read in rustls 0.23.45, the version in the lock. `ClientConfig::builder()`
(`src/client/client_conn.rs:315`) calls `builder_with_protocol_versions` (`:332`), which calls
`CryptoProvider::get_default_or_install_from_crate_features` (`src/crypto/mod.rs:243`-`:256`), then
`builder_with_provider` (`client_conn.rs:354`), a struct literal, and `with_protocol_versions`
(`src/builder.rs:207`), which walks the provider's cipher suites and key-exchange groups in memory.
`get_default_or_install_from_crate_features` reads `PROCESS_DEFAULT_PROVIDER`, a `OnceLock`
(`mod.rs:727`-`:732`), through `get_default` (`:234`); without one, `from_crate_features` (`:265`)
returns `ring::default_provider()` (`src/crypto/ring/mod.rs:32`-`:40`) or
`aws_lc_rs::default_provider()` (`src/crypto/aws_lc_rs/mod.rs:43`-`:51`), each a struct of static
slices copied into `Vec`s, and `install_default` sets the `OnceLock` (`mod.rs:712`-`:716`). No file,
socket or random source is touched; `SecureRandom::fill` reads the system's randomness only when a
handshake calls it. The one write is the panic hook's message on stderr when two providers are
linked and none installed, the case the link refuses. The resolution stays on the caller's thread.

### 10. The NATS suite

`cargo test -p ulo-rpc-nats --features integration --test conformance --locked`, in the mirror,
after `cargo clean -p ulo-rpc-nats` and a build whose output names `Compiling ulo-rpc-nats` once
(`suites/fixed-build.txt`, `suites/fixed-binary.txt`): 12 runs (`suites/fixed-*.txt`,
`suites/summary.txt`), 12 passed, 54 passed and 1 ignored each, libtest's 6.64 to 15.52 s, median
9.3 s, load averages 5.9 to 14.1 over the minute, no roots file left by any. The suite's servers are
`nats://` links to a plaintext server, so each now waits for the process's load, 0.2 to 0.3 s warm
in batch 41's p03, before it connects plain. The times were measured while batches 44 and 45 built
and tested on the host, and no control at `d2e6b26a` was run beside them (S8).

### 11. Building in a mirror

As batch 41 did: `batch43/mirror` is `git archive` of `d2e6b26a`, with `crates/ulo-rpc-nats`
copied from the real tree by checksum, no mtime carried, and compared with `diff -r` before every
probe, break, suite and check (`scripts/sync.sh`). The break runner's tree hash covers the mirror's
`crates/ulo-rpc-nats`, `Cargo.toml` and `Cargo.lock`. Batches 44 and 45 edited `crates/ulo`,
`crates/ulo-runtime-tokio` and the other RPC links in the real tree throughout. The workspace test
ran in the real tree at the end (S9).

## The tests

- **The server's wait and its bound (F423, the sixty-sixth response's point 1):**
  `link::tests::over_tls::a_nats_server_waits_for_its_roots_and_reconnects_with_them` and
  `link::tests::plain::a_nats_server_whose_roots_outlast_roots_timeout_connects_without_them`,
  behind `integration`; `link::tests::a_tls_link_whose_roots_outlast_roots_timeout_is_refused`,
  `link::tests::a_zero_roots_timeout_is_refused_at_bind` and
  `link::tests::roots_timeout_bounds_the_wait_as_its_bound_says`. A `nats://` client not waiting:
  `link::tests::plain::a_plain_link_connects_within_the_boot_budget_while_its_roots_load`.
  Decisions 4, 5, 7 and 8.
- **F428 on NATS:** `link::tests::plain::a_link_dropped_without_close_closes_its_connection`, behind
  `integration`. Decisions 1 to 3, 7 and 8.

### Before and after

Specs in `batch43/specs/`, output in `batch43/probes/` and `batch43/broken/` with `summary.txt`;
the suite runs by `batch43/scripts/nats_reruns.sh` in `batch43/suites/`.

| Run | Target | Result |
| --- | --- | --- |
| p01. connections after a drop without `close`, at `d2e6b26a` | the link's unit tests | each link's connection kept for 3 s |
| p02. the new tests' times, three runs | the link's unit tests | the waiting test's `listen` 6.008 to 6.015 s, longest gaps 5.8 to 13.2 ms binding and 5.3 to 5.9 ms reconnecting; the `nats://` expiry 206.7 to 207.7 ms; three `tls://` refusals 606.3 to 608.1 ms; the boot-budget test 7.7 to 11.9 ms |
| b01. a `nats://` server not waiting | the waiting test | failed, `UnknownIssuer` at `listen` |
| b02. the store read at every connection | the waiting test | failed, 392.6 ms while binding |
| b03. the store read at every reconnect | the waiting test | failed, 740.4 ms while reconnecting |
| b04. the bound ignored | the two expiry tests | failed, 10.02 s each |
| b05. a zero accepted | the zero test | failed, the bind not `Configure` |
| b06. `Default` at 10 s | the bound test | failed, `Err(10s)` |
| b07. `Unbounded` at 30 s | the bound test | failed, `Err(30s)` |
| b08. the expiry not logged at `warn` | the `nats://` expiry test | failed, no `warn` |
| b09. a `nats://` client waiting | the boot-budget test | failed, 10.02 s |
| b10. no task held | the drop test | failed at the app's server |
| b11. held calls not cleared | the drop test | failed at the server holding a call |
| b12. the client's router not held | the drop test | failed at the client |
| the NATS suite | twelve runs | 12 passed, 6.64 to 15.52 s |
| the restored tree | every target above | every test passed |

## Left for the DESIGN folds

Line numbers are those of the transports DESIGN at `d2e6b26a`.

- Decision 148 (line 1674): its title and body hold for a `nats://` client alone. A server's
  `listen` waits for the load under `roots_timeout`, on either scheme, since async-nats reconnects
  its one connection with the options it was made with; on expiry a `nats://` server connects
  without the file and logs one `warn`, F423's path; `Roots::awaited`, `Nats::roots`'s `waits`, the
  expiry and zero tests, the waiting test and its three breaks, and the boot-budget test with the
  server on a load of its own and b09.
- Decision 139 (line 1665): "each link hands the file to every connection it makes once the load
  has ended" and "the first points each side can await anything (134); a `tls://` link waits for it
  there and a `nats://` link waits for nothing (148), and `listen` and every later connect read the
  file once it is written" become a server's `listen` waiting on both schemes; the record of the
  sixty-fifth response's "later connects and reconnects use the file" as wrong about a server's
  reconnects, by the sixty-sixth.
- Decision 144 (line 1670): "hands the file to every connection it makes once the load has ended,
  `nats://` included" stays for clients; a server's connection is made once the load answers or the
  bound passes.
- Decision 145 (line 1671): "`ClientConfig::builder()` does no I/O … that it runs on the caller's
  thread rather than the blocking pool is pending sign-off" becomes accepted, with decision 9's
  reading of rustls 0.23.45.
- Decision 147 (line 1673): a wait for the load is bounded by `roots_timeout`, the load is not, and
  a wait that gives up leaves it running, as a call cut short already did.
- A new decision for `roots_timeout`: the setting, `within`, the link's own clock and default, the
  `zero_bound` refusal in `prepare`, the `tls://` refusal's text, and the bound test on a paused
  clock with `test-util`.
- A new decision, or decision 82 widened, for F428 on NATS: `State::tasks` and `AbortOnDrop`,
  `ServerSide`'s drop clearing the held calls, p01, the drop test and b10 to b12.
- §5.3 (line 1077, the NATS TLS paragraph): a server waits for the load under `roots_timeout`.
- §12 (line 1481): the `tls://` NATS row gains a load outlasting `roots_timeout`; a new row for
  `.roots_timeout(Bound::After(Duration::ZERO))`, refused at a server's `prepare` as `Configure`.

## Needs sign-off

Numbered from S1 for this batch.

### S1. A client's zero `roots_timeout` is not refused

Decision 4. A client runs no `prepare`. On a `tls://` client a zero bound refuses each call whose
connect finds the load still running, the select polling the load first; on a `nats://` client it
changes nothing. Refusing it in `usable`, which `RpcClient::new` calls, would refuse it where the
client is built.

### S2. The bound is the link's own, timed on the link's runtime

Decision 4. `Bound::Default` is 30 s, the link's default, not an app default as core DESIGN §3.9
describes `Default` for hooks and constructions, and the wait is timed on the link's tokio runtime,
not the app's `Timer`; an app with no `Timer` still has the bound. `ulo-rpc`'s `timeout_grace`
writes a fixed default too, timed by the app's `Timer`. Whether this departs from §3.9's "every
wait it times runs through the app's `Timer`", which the core states of itself, is the designer's
call.

### S3. A `nats://` server's wait starts at `listen`, and its load at `prepare`

Decision 5. The bound covers `listen`'s wait alone. On a cold macOS store, up to 3 s, the server's
bind now carries the load where it carried none; on a plaintext server, as in the suite, the wait
buys nothing. A server whose load fails later than the bound gets two `warn`s, the expiry's and,
from the task `prepare`'s `Roots::ready` armed, the failure's.

### S4. The planted native loads are held through their cost, not counted

Decision 8. async-nats's own read of the store has no seam, so the waiting test holds it at zero
through the stall it causes on the polling thread and the `UnknownIssuer` a connection trusting the
platform's store meets, as batch 39's breaks did.

### S5. A process subscriber in the link's unit tests

Decision 7. `capture` installs a global `tracing` subscriber, behind `integration`, the first test
reading it installing it for the binary, as MQTT's conformance binary does; it records `warn` and
`error` events with their thread.

### S6. `close` leaves the task handles in the state

Decision 2. The tasks end on their own at `close` as before; their handles stay until the link
drops. A client's routers are kept in a list pruned at each connect, not replaced, so a connect
made while an earlier connection's router runs does not stop it.

### S7. A server side's drop clears its held calls

Decision 3. A call the core still holds keeps its reply path, which publishes as before; its table
entry, which the lanes alone read, goes with the side.

### S8. The suite's time beside two other batches

Decision 10. Median 9.3 s against batch 41's 7.7 s, both beside other builds, load averages to 14
here. No control was run; a run on an idle host would settle what the server's wait costs.

### S9. The batch built in a mirror of `d2e6b26a`

Decision 11. Every probe, break, suite run and check ran on `d2e6b26a` with this batch's crate; the
workspace test ran on the real tree, with batches 44's and 45's edits.

### S10. What the batch left in the temp directory

One empty directory, `ulo-rpc-nats-roots-test-12238-17`, b09's, its test failing before its own
cleanup (`verify/roots-newer-than-start.txt`); no roots file is newer than the batch's start. The
batch removed nothing.

### S11. The other broker links were not probed

F428's premise for the MQTT, Redis, RabbitMQ and Kafka links is batch 44's. Read only, not probed:
MQTT keeps each loop's `JoinHandle` in its state after `listen` and `connect` disarm their guard,
and a `JoinHandle` detaches on drop.

## Ledger text

For `FRAMEWORK_GAPS.md`, each appended under its entry.

Under F423:

*Appended 2026-10-18:* Narrowed on `experiment/di-redesign` in `<commit>` by race 2b's test batch
43, by the sixty-sixth response's point 1, which corrects the sixty-fifth's "later connects and
reconnects use the file": async-nats reconnects a connection with the options it was made with
(0.46.0 `src/connector.rs:55`-`:78`, read), so a server connection made before the load ended kept
no file for the app's life. A server's `listen` now waits for the load on `nats://` as on `tls://`
(`Nats::roots`, `crates/ulo-rpc-nats/src/link.rs:225`, `Roots::awaited`, `:1104`), under a new
`Nats::roots_timeout(Bound)` (`:208`), 30 s at `Bound::Default`, as long as the load takes at
`Bound::Unbounded`, `Bound::After(Duration::ZERO)` refused in `prepare` with `zero_bound`'s hint; a
`tls://` client's connect waits under it too, and a `tls://` link whose load outlasts it is refused
`Misconfigured`. A `nats://` server whose load outlasts it connects without the file and logs one
`warn`, F423's path, open for that case; a `nats://` client still waits for nothing, its next
connection after a lost one made afresh. Held by
`over_tls::a_nats_server_waits_for_its_roots_and_reconnects_with_them`: a `nats://` server whose
load blocks 6 s, past the 5 s boot budget, waits for it, connects with the file to a server
requiring TLS, and reconnects with it after the relay's cut, on one thread whose gaps stay under
25 ms; it fails with the server not waiting on `UnknownIssuer` at `listen` (batch 43's
`broken/b01-0.txt`), with the store read at every connection at 392.6 ms while binding
(`broken/b02-0.txt`), and with it read at every reconnect at 740.4 ms while reconnecting
(`broken/b03-0.txt`). The bound by `plain::a_nats_server_whose_roots_outlast_roots_timeout_connects_without_them`,
`a_tls_link_whose_roots_outlast_roots_timeout_is_refused`, `a_zero_roots_timeout_is_refused_at_bind`
and `roots_timeout_bounds_the_wait_as_its_bound_says`, failing with the bound ignored, a zero
accepted, the defaults moved and the `warn` lowered (`broken/b04-*.txt` to `b08-0.txt`); the
client's not waiting by `plain::a_plain_link_connects_within_the_boot_budget_while_its_roots_load`,
failing at 10.02 s with the client waiting (`broken/b09-0.txt`). The NATS suite passed 12 of 12
runs, 6.64 to 15.52 s, beside other batches' builds.

Under F428:

*Appended 2026-10-18:* The NATS link held the same shape, fixed on `experiment/di-redesign` in
`<commit>` by race 2b's test batch 43, by the sixty-sixth response's point 2. Batch 43's
`probes/p01-0.txt`, against a plaintext NATS server's `/connz` at `d2e6b26a`: a server's link bound
through an app dropped unserved, a server's link listened directly and dropped, and a client's link
dropped with what its `connect` answered each kept its connection for the 3 s watched. `listen`'s
request and control lanes and `connect`'s reply router were detached tasks holding the connection's
`Client` and subscriptions, ended only by `close`, a disconnect or the subscription's end. Each is
now held in the link's state through an `AbortOnDrop` (`State::tasks`,
`crates/ulo-rpc-nats/src/link.rs:123`, `:135`), aborted when the link drops, and a server side's
drop clears its held calls (`:467`), whose reply paths hold the table they sit in and the
connection with it. Held by `plain::a_link_dropped_without_close_closes_its_connection`, the
`/connz` count back to 0 after the app's drop, to 1 after a server's link holding a call drops and
to 0 after the client's; it fails with no task held (`broken/b10-0.txt`), with the held calls not
cleared (`broken/b11-0.txt`) and with the client's router alone not held (`broken/b12-0.txt`).

## Verification

Full output of every run is in the session scratchpad, `batch43/`: `probes/`, `broken/` and
`specs/`, `suites/`, `verify/` for the checks, `superseded/` for the first runs, and `scripts/`.
`batch43/scripts/verify.sh` ran the checks below in the mirror on the final crate, its tree hash
read before and after (`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`).

- **The NATS suite** with `integration`: 12 of 12 runs passed, 54 and 1 ignored each, batch 41's
  count (decision 10). The other RPC suites were not run: the batch changes no file outside
  `crates/ulo-rpc-nats`.
- **The NATS link's unit tests with `integration`,** one thread at a time: 26 passed, the six new
  beside the 20 `d2e6b26a` defines, in 13.78 s. **Without it,** `cargo test -p ulo-rpc-nats`: 17
  unit, where `d2e6b26a` runs 14; `conformance` 0, behind `integration`; `handle` 2;
  `two_providers` 3; 1 doctest ignored.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", 461 files scanned, 2 listed, 0 findings, exit 0.
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: 50 members,
  54 entries, 33 checked, 0 failures, 0 errors, exit 0. `ulo-rpc-nats` is `tokio-bound`, not
  checked; the `test-util` feature is on a dev-dependency.
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, with the OpenSSL flags, and `cargo +1.88 check --workspace --all-targets
  --exclude ulo-http-salvo --exclude ulo-graphql-async-graphql --locked` in `target/msrv-1.88`: exit 0
  each, `ulo`'s 17 known warnings, none located outside `crates/ulo/src`.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p ulo-rpc-nats --locked`: exit 0.
- **Clippy:** `cargo +1.98.1 clippy -p ulo-rpc-nats --all-targets --all-features --no-deps --locked`:
  exit 0, no warning located in `crates/ulo-rpc-nats`.
- **The workspace test,** in the real tree with batches 44's and 45's edits, `cargo test --workspace
  --no-fail-fast --locked` with the OpenSSL flags: exit 0, 1,319 passed, 0 failed, 141 ignored, over
  208 lines starting `test result:`, where batch 41 had 1,310 and 141 over 203. The real tree then
  held edits to `crates/ulo`, `crates/ulo-runtime-tokio/tests/app.rs` and the TCP, UDP, MQTT, Redis,
  RabbitMQ and Kafka links beside this batch's (`verify/real-tree-status-before-workspace-test.txt`);
  which tests the nine more are was not read.
- **The break runner's restore check:** every probe and break reported `restore ok`, the tree hash
  equal before and after and both `cmp`s identical (`broken/summary.txt`, `probes/summary.txt`); the
  runner is batch 41's, whose `cmp` self-test it keeps.
- **The prose:** a scan for the banned register and for unfilled template braces, asserted first
  against a planted violation (`verify/planted.txt`), over this log and every added line under
  `crates/ulo-rpc-nats` (`verify/added-lines.txt`), found none. The scan checks vocabulary only.
- **Containers and images:** at the batch's start 13 containers, the user's, every one exited, and 25
  images (`batch43/docker-start-containers.txt`, `docker-start-images.txt`); at the end the same 13
  by id, each still exited, and the same 25 images (`docker-end-*.txt`). The other batches' test
  containers appeared in some before-and-after lists of this batch's runs, a `redis:7-alpine`
  among them; none was this batch's. This batch's containers were its tests' and the suite's
  testcontainers, each removed by its scenario. The batch pulled no image, ran no `docker ... prune`
  and started, stopped or removed nothing of the user's.
