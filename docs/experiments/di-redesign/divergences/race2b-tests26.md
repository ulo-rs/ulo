# Divergences: race 2b, tests batch 26, F385 found to be the WebSocket drain scenario racing `/echo`'s connection phase with the paired hooks intact, a client's runtime handed to `Link::connect` so the smol TCP link needs none of its own (F386), one public `Smol::block_on` in place of five copies (S2), and `ulo-rpc-tcp-core` marked a crate for link authors (S3)

The forty-ninth response, signed off 2026-10-11, answers `runtime-e3.md`. F385 is not the paired-hook
guarantee failing. `/echo` refuses after the upgrade, so its connection phase runs after the 101,
and the scenario starts the app's close as soon as the client reads that 101; when the drain begins
first, the phase cannot open its execution, the connection is refused with 1001 and the drain's own
reason, and neither `OnConnect` nor `on_disconnect` runs. A probe holding the connection 300 ms in
that window fails the scenario on six hosts across both runtimes, every time; the natural failure did
not recur in 2,200 runs. The scenario now exchanges a message before it closes the app. `Link::connect`
takes the client's runtime, which `RpcClient` passes; the smol TCP link spawns a client connection's
tasks there unless `.with_runtime(..)` names another, and its refusal of a client link given none is
gone. `ulo_smol::Smol` gains `block_on`, `with_workers` and `current`, and the runtime, HTTP and RPC
suites' smol harnesses, and two test helpers with the same body, are each a line over it.

Files changed: `Cargo.lock`; for F385, `crates/ulo-ws-conformance/src/cases/close_codes.rs`; for F386,
`crates/ulo-rpc/src/{link.rs, client.rs}`, `src/link.rs` of the seven tokio links,
`crates/ulo-rpc-tcp-smol/{src/lib.rs, src/link.rs, tests/runtime.rs, tests/conformance.rs,
tests/conformance_cbor.rs, tests/conformance_tls.rs, tests/drain_end.rs, tests/client_close.rs}`,
`crates/ulo-rpc-tcp/tests/{interop.rs, handle.rs}`, `tests/handle.rs` of the six other tokio links, `crates/ulo-rpc-kafka/tests/conformance.rs`, `crates/ulo-rpc-mqtt/tests/{conformance.rs,
abandoned_connect.rs}`, `crates/ulo-rpc-redis/tests/conformance.rs`,
`crates/ulo-rpc/tests/{runtime.rs, drain_window.rs, stream_end.rs}` and
`crates/ulo-rpc-conformance/src/cases/{deadlines.rs, order.rs}`; for S2, `crates/ulo-smol/{Cargo.toml,
src/lib.rs, tests/conformance.rs, tests/block_on.rs (new)}`,
`crates/ulo-http-conformance/{Cargo.toml, src/smol_harness.rs}`,
`crates/ulo-rpc-conformance/{Cargo.toml, src/smol_harness.rs}`,
`crates/ulo-listen-smol/tests/listener.rs` and `crates/ulo-hyper-serve/tests/support/mod.rs`; for S3,
`crates/ulo-rpc-tcp-core/{Cargo.toml, src/lib.rs}`; in the workspace's `FRAMEWORK_GAPS.md`, a note
under F385 and one under F386. The batch started from `2a9c5ef0`; `ce31dd2f`, committed by another
agent during the build, touches `transports/RESPONSE.md` alone, so the tree is `ce31dd2f` plus this
batch.

## The signatures

```rust
// ulo_rpc::Link
fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send;
//                ^ new: the client's runtime, its app's inside an app, `RpcClient::new`'s outside one

// ulo_rpc_tcp_smol::Tcp
pub fn with_runtime(self, runtime: impl Runtime) -> Self;   // unchanged signature; now an override
// `Link::usable` is no longer overridden: a client link given no runtime is usable.

// ulo_smol
#[derive(Clone)]
pub struct Smol { /* private: the executor, and the worker count `block_on` uses */ }
impl Smol {
    pub fn with_workers(self, workers: usize) -> Smol;      // new: threads `block_on` adds; 0 unset
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output; // new
    pub fn current() -> Option<Smol>;                       // new: the runtime whose `block_on` runs on this thread
}
```

Unchanged: `RpcClient::new`, `RpcClientModule`, `Link::usable`, `Link::prepare` and every other `Link`
method; the seven tokio links' public API; `ulo-rpc-tcp-core`'s API; every conformance scenario's
name. The smol TCP link's `NO_RUNTIME` text names a server alone, the only side that can still lack
one, in an app with no runtime.

What changes in meaning without a signature: a smol TCP client link built with no runtime connects,
its tasks on its client's runtime, where `usable` refused it; a link carrying its own runtime ignores
the argument. `drain_closes_with_1001` exchanges one message before it starts the close.

Dependencies: `ulo-smol` gains `event-listener`, already in its tree through `async-lock`, for
`block_on`'s stop signal, and drops its `async-io` and `futures` dev-dependencies;
`ulo-http-conformance` and `ulo-rpc-conformance` drop `async-io` from their `smol` features. No
package is added to the lock.

## Decisions

### 1. F385: the scenario raced, and the guarantee held

- **The hunt.** The pipe host's test binary was built once at the batch's start and copied to the
  scratchpad (`bin/conformance_pipe`, its hash beside it), and every natural run used that copy or
  a variant built the same way. 2,200 runs, none failing: 300 unloaded; 300 of the binary and 500 of
  the scenario alone under twelve CPU-bound `yes` processes on a ten-core machine; 300 and 400 in six
  and eight parallel workers, each run on a fresh copy of the binary, since the one failure was the
  first run after a build, under four and ten; and 200 each with the smol harness's executor on one
  thread and on sixteen, under six. Every run's output is kept (`f385/runs/`). A first parallel batch
  of 300 handed each worker's index to the binary as a test filter and ran subsets; it is kept under
  `f385/runs/superseded/` and not counted.
- **The window.** Read, then probed: `/echo` is `refuse = close`, so `Switch::serve` spawns a task
  that runs the connection phase after the 101 is written (`connection.rs`, `run`). The suite's
  client returns from `connect` once it reads the 101 and the scenario starts the close at once. The
  phase opens its execution with `Execution::open`, which the app refuses from Draining on, and
  `connect` answers that as a refusal with close code 1001 and "the server is shutting down", the
  drain's own code and reason. No `OnConnect` ran, so by `Admitted::end`'s rule no `on_disconnect`
  runs either, and the client reads exactly what an admitted connection reads at the drain.
- **The probe.** 300 ms on the runtime's clock between the upgrade and the phase, for `/echo` alone,
  with a line at the refusal and one when the phase dispatches. The scenario failed on the pipe
  host, the HTTP port on tokio and on smol, the standalone server on tokio and on smol, and axum's
  embed host, each with F385's panic and the probe's "connect phase refused: `Execution::open`
  answered Closed; no `OnConnect`" (`probes/p01-f385-widened-window.txt`). The window is in shared
  code, so every host and both runtimes had it; the pipe host is where its timing showed once.
- **The fix is the scenario's.** It exchanges one `echo` message before starting the close, as
  `on_disconnect_fires_once` and the keep-alive scenarios already do; an answered message shows the
  phase admitted the connection, so the drain meets an admitted, idle one. Under the same probe it
  passes on all six hosts, the probe showing the phase dispatched before the drain
  (`probes/p03-*.txt`). A comment above the exchange says why it is there. See S1.
- **The other mechanism F385 named,** the drain's live count reaching zero before the connection's
  `end` ran, is ruled out by reading: the tracker's guard drops after the spawned future, `end`
  included, completes, and the pipe server's drain joins the table's.

### 2. F386: the runtime comes from the client

- `Link::connect` takes `&Arc<dyn Runtime>`. `RpcClient::build` keeps a clone of the runtime it was
  built with for the connect closure, so a reconnect passes the same one. `RpcClientModule` already
  builds its client on the app's `Dep<dyn Runtime>`, so a client module needs nothing new.
- The smol link uses `.with_runtime(..)`'s runtime when given one and the handed one otherwise. Its
  server side is unchanged: given none it takes its app's in `prepare`, and refuses there when the
  app has none, the one case left for `NO_RUNTIME`.
- The seven tokio links name the parameter `_runtime`; the `Link` docs say a link carrying its own
  runtime ignores it. Their `handle.rs` tests now hand a tokio runtime to a link built off one and
  given no handle, and the link still refuses, naming `.with_handle(..)`: the argument is ignored
  even when it could serve. See S2.
- The SPI's module docs gain a paragraph: a server's link reads its app's runtime in `prepare`, a
  client's link receives its client's in `connect`.
- Test links (`client.rs`'s two, `runtime.rs`'s two, `drain_window.rs`, `stream_end.rs`) take the
  argument and ignore it; the suite's `MutedCancel` passes it to the link it wraps; `order.rs`'s and
  Kafka's direct `connect` calls hand the app's runtime.
- The three smol suites, `drain_end.rs` and `client_close.rs` build their client links with no
  runtime, and `interop.rs` builds the smol client with none, so its tasks run on the tokio client's
  runtime handed through `connect`.

### 3. S2: one `block_on`, owning its workers and the current runtime

- **What the five copies did.** Each made an executor, started worker threads running it until a
  stop signal, set a thread-local the harness's `runtime()` read, ran the future on the calling
  thread, then stopped and joined the workers. They had already drifted: the RPC copy set the
  thread-local on its workers and the HTTP and runtime copies did not, so `runtime()` called from a
  task on a worker answered in the RPC suite and panicked in the other two, whose scenarios happen
  to call it only on the test's thread. Two test helpers, `ulo-listen-smol`'s
  `on_smol` and `ulo-hyper-serve`'s, are the same body with one worker and no thread-local; they move
  too.
- **The shape.** `block_on(&self, fut)` runs the executor on the calling thread while `fut` runs,
  and on `with_workers(n)` threads, started there and joined before it returns, a panic in `fut`
  included, through a guard's `Drop`. `Smol::current()` answers the runtime on the calling thread and
  on each worker while `block_on` runs and `None` elsewhere; the previous value is restored on the
  way out, so a nested `block_on` leaves its caller's. That is what lets each harness's `runtime()` be
  `Smol::current()` and drops all three thread-locals. A worker panic panics `block_on` once `fut` has
  completed, unless `fut` is already panicking. See S3.
- **The stop signal** is an `event-listener` `Event` beside a flag, registered before the flag is
  read again; `ulo-smol` depends on nothing new in its tree.
- **Each harness** is now `Smol::new(Arc::new(Executor::new())).with_workers(WORKERS).block_on(fut)`
  and `Smol::current().expect(..)`, the RPC suite's expect message on all three.
- **For users,** the crate docs show `smol.block_on(..)` serving an app, in place of
  `async_io::block_on(executor.run(..))`, and say it is how a test drives a smol app;
  `block_on`'s own docs carry a compiled example spawning on `Smol::current()`.

### 4. S3: `ulo-rpc-tcp-core`'s crate docs

The crate doc's first paragraph keeps what the crate is and a second says it is an implementation
crate for link authors, that application code names `ulo_rpc_tcp::Tcp` or `ulo_rpc_tcp_smol::Tcp`
and not this crate, and that its names follow what the links need. The manifest's description reads
"For link authors, not applications: ..." where it read "Internal: ...".

## The tests

- **F385:** `drain_closes_with_1001`, changed as decision 1 describes, on all ten WebSocket hosts.
- **F386, `ulo-rpc-tcp-smol`'s `runtime.rs`,** replacing the three refusal tests:
  `a_client_link_given_no_runtime_runs_its_tasks_on_its_client_s` (outside an app, through
  `RpcClient::new(Tcp::new(addr), runtime)`; the link's two tasks and the client's router and close
  on the client's runtime, four exactly); `a_client_module_on_a_link_given_no_runtime_runs_its_tasks_on_its_app_s`
  (inside an app, `RpcClientModule::for_root(Tcp::new(addr))`; at least the link's two and the
  router on the app's runtime); `a_client_link_given_a_runtime_runs_its_tasks_there` (the link's two
  on its own, the client's two on the client's); `a_client_link_connects_on_the_runtime_it_is_handed`
  (`Link::connect` called directly; two tasks on the handed runtime). The two server tests stay.
- **F386 elsewhere:** every smol suite's client link and `interop.rs`'s smol client given no runtime;
  the tokio links' `handle.rs` handing a runtime and still refused.
- **S2, `ulo-smol`'s `block_on.rs`:** `a_task_spawned_inside_runs_while_the_future_waits_for_it`,
  `workers_poll_tasks_on_threads_beside_the_caller`,
  `current_answers_the_runtime_on_the_caller_and_on_every_worker`,
  `a_panicking_future_stops_the_workers_and_leaves_no_runtime_current` and
  `workers_stop_when_the_future_completes`, each on a thread of its own under a five-second bound,
  so workers left running fail it rather than hang the binary. The doc example compiles and runs
  as a doctest. Every smol suite, `ulo-listen-smol`'s and `ulo-hyper-serve`'s tests run on it.

### Before and after

Each break went through `batch26/scripts/brk.py`, batch 25's script with `.github` added to its hash:
it replaces each span (asserting it occurs once), runs the target, writes every touched file and
`Cargo.lock` back byte for byte, and compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`,
`Cargo.lock` and `.github` and every untracked file under `crates`. Specs in `batch26/specs/`, output
in `batch26/broken/` and `batch26/probes/` with `broken/summary.txt`.

Every restore reported `ok`. The probes and the first two breaks ran on the tree hashed
`be256f2f..` or before it; the `block_on` breaks, and the first break against `runtime.rs` alone, ran on `81ef48f8..`, after every test in
`block_on.rs` was bounded, the one edit between the two (decision 3). The first runs of three of
them are kept under `broken/first-layout/`, and the first run of the stop break, whose two unbounded
tests hung their binary for 17 min 50 s until it was killed, under `broken/superseded/`.

| Break | Target | Result |
| --- | --- | --- |
| 300 ms between `/echo`'s upgrade and its connection phase, the scenario as at `2a9c5ef0` | `drain_closes_with_1001` on the pipe host, the HTTP port on tokio and on smol, the standalone server on tokio and on smol, and axum's embed host | failed on all six, `probe.departures` empty where `[("/echo", Drain)]` was due, the probe printing that `Execution::open` refused and no `OnConnect` ran |
| the same window, the scenario exchanging a message first | the same six | passed on all six, the probe printing the phase dispatched before the drain |
| the smol link's `connect` ignoring the handed runtime and refusing without one of its own, as before F386 | the smol TCP suite | all 32 failed, "the tcp link could not connect: the smol TCP link has no runtime for its tasks" |
| the same | the smol link's `runtime.rs` | five of six failed, "no runtime for its tasks": the three client tests whose link has no runtime, and the two server tests through their client, whose link has none; the client given its own runtime passed |
| the smol link's `connect` taking the handed runtime over its own | `runtime.rs` | `a_client_link_given_a_runtime_runs_its_tasks_there` failed, "the link's runtime was handed 0 tasks" |
| `block_on` starting none of its workers | `block_on.rs` | `workers_poll_tasks_on_threads_beside_the_caller` failed, "every task ran on the caller's thread" |
| `block_on`'s workers not entering `Smol::current` | `block_on.rs`; `ulo-rpc-conformance`'s `harness.rs` | the current-runtime test failed, "a thread running the executor has the runtime current"; `a_task_on_a_smol_worker_finds_the_scenario_s_runtime` failed, the drift the RPC copy had fixed and the other two had not |
| `block_on`'s workers never told to stop | `block_on.rs` | four of five failed, "the test did not finish within 5s" |
| `block_on` leaving its runtime current | `block_on.rs` | the panic test and the current-runtime test failed, "the runtime stayed current after `block_on` unwound" and "returned" |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those read on `ce31dd2f`.

- §5.3's `Link` listing (line 986): `connect(&self, runtime: &Arc<dyn Runtime>)`, the client's
  runtime, its app's inside an app and `RpcClient::new`'s outside one; a link carrying its own
  ignores it.
- §5.3's constructors (line 1046) and the smol link's paragraph (line 1052): `.with_runtime(..)` is
  an override; a client link given none runs its tasks on its client's runtime, a server's on its
  app's; no client refusal. Decision 103 (line 1599), "a client needs one", no longer holds, and the
  refusal table's row "`fw_rpc_tcp_smol::Tcp` as a client's link, given no runtime" (line 1456)
  goes.
- §8, "A runtime is given, never found" (line 1296): the client hands its runtime to its link.
- §8's `fw-smol` paragraph (line 1305): `Smol::block_on`, `with_workers` and `current`; the runtime,
  HTTP and RPC suites' smol harnesses run on them.
- §1's crate table, `fw-rpc-tcp-core`'s row: an implementation crate for link authors, not a
  dependency of application code; principle 5 (line 13) and §1's `fw-rpc-tcp-smol` row (line 40),
  wherever they say a client of the smol link is given its runtime.
- §4.2's scenario list (line 813), the drain scenario: it exchanges a message first, since a
  `refuse = close` gateway's connection phase runs after the 101, and a drain reaching it first
  refuses the connection with 1001 and runs no hook.
- The X table after X36 (line 1396): `Smol::block_on`, `Smol::with_workers`, `Smol::current`, and
  `Link::connect`'s parameter.

## Needs sign-off

Numbered from S1 for this batch.

### S1. F385 is fixed in the scenario, not the framework

Decision 1. A `refuse = close` connection upgraded just before the drain is refused at its connection
phase with 1001 and runs no hook, which `Switch::serve`'s documented rule already states, and which a
client cannot tell from an admitted connection closed by the drain. The alternative is a framework
change: let a connection whose 101 was written before the drain run its connection phase through
`Execution::open_terminal`, so it is admitted, then closed by the drain with `on_disconnect`. That
would make every upgraded connection reach `OnConnect`, at the cost of running a connect handler
during the drain. A suite scenario pinning the current rule would need a way to hold the connection
in that window on every host, which no host offers; the probe pins it for now.

### S2. The tokio links ignore the runtime even when they have none of their own

Decision 2. A tokio link built off a runtime and given no handle still refuses to connect when its
client hands it a tokio runtime, as the response said: they carry their own handle. The alternative
is a tokio link falling back to the handed runtime when it is a `ulo_tokio::Tokio`, which would need
a downcast `Runtime` does not offer.

### S3. `block_on`'s workers are a setting on `Smol`, and `Smol::current` is public

Decision 3. The harnesses need extra threads and a thread-to-runtime lookup; putting both in
`ulo-smol` is what removes the three thread-locals and their drift. The cost is two public items
beyond `block_on`. The alternative is a `block_on` that runs only the calling thread, with each
harness keeping its threads and thread-local, which leaves the copies the response asked to remove.

### S4. The two test helpers with the same body moved too

Decision 3. `ulo-listen-smol`'s and `ulo-hyper-serve`'s `on_smol` were the helper with one worker;
they are now `with_workers(1).block_on(..)`. The alternative is leaving them, outside the three the
response named.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch26/`: `f385/` for the hunt
(`runs/` one file per run with a `summary.txt` per batch, `build-workers*.txt`), `bin/` for the pinned
binaries and their hashes, `probes/`, `broken/` (with `summary.txt`, `summary-2.txt`,
`first-layout/` and `superseded/`) and `specs/` for the breaks, `runs/` for the runs during the
build, `suites/` for the RPC suites (with `summary.txt` and the container lists after each broker),
`tree/`, `verify/` and `scripts/`. The final tree's hash, by `brk.py`'s measure, is `81ef48f8..`
(`verify/final-tree-hash.txt`); every suite run, check, the workspace test, the docs and clippy ran
on it.

- **The RPC suites, once each,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 32 passed and 3 ignored each (2.53 s, 2.53 s,
  2.52 s); the smol TCP link's three 32 and 3 each (2.51 s, 2.53 s, 2.52 s), their client links given
  no runtime; UDP 29 and 6 (1.31 s). With `--features integration --test conformance` and the
  OpenSSL flags: NATS 34 and 1 (9.28 s); Redis 33 and 3 (10.24 s); MQTT 39 and 1 (9.49 s); Kafka 35
  and 1 (30.72 s); RabbitMQ with `ULO_CONFORMANCE_PARALLEL=6` 34 and 1 (72.86 s), against batch 25's
  85.86 s and stage e3's 30.60 s, run once, the cause of the spread not traced. Every count is batch
  25's.
- **The smol hosts and `ulo-smol`** (`verify/smol-ws-summary.txt`): `ulo-smol` 15 in its runtime
  suite, 5 in `block_on.rs` and the `block_on` doctest; the HTTP suite on the hyper backend on smol 36
  and 2; the WebSocket suite on the standalone server on smol 31, on the HTTP port on smol 31 and on
  the pipe host 31; `ulo-listen-smol`'s 2 and `ulo-hyper-serve`'s 7 on the shared `block_on`;
  `ulo-rpc-conformance` with `tokio,smol`, its unit tests 2 and `harness.rs` 5.
- **`ulo-ws`'s tests, three runs** (`verify/ulo-ws-{1,2,3}.txt`): each run every target passing, the
  three suite hosts 31 each, `handoff.rs` 11, `table.rs` 9, `broadcast.rs` 2, `runtime.rs` 1.
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`verify/check-*.txt`, `verify/diag.txt`, by batch 25's `diag.py`, which reported a planted
  location, `verify/diag-selftest.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other (`verify/check-1.88.txt`).
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags: 1,070 passed, 0 failed, 138
  ignored across 196 test results (`verify/workspace-test.txt`, `workspace-test-count.txt`). Batch
  25's 1,063 plus 7: `block_on.rs`'s 5, a new target, which is the one more result; the `block_on`
  doctest; and one in the smol link's `runtime.rs`, whose three refusal tests became four client
  tests.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`tree/summary.txt`):
  nothing to print, or "did not match any packages", for `ulo`, `ulo-transport`, `ulo-net`,
  `ulo-http`, `ulo-rpc`, `ulo-ws`, `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`,
  `ulo-runtime-conformance`, `ulo-smol`, `ulo-rpc-tcp-core`, `ulo-rpc-tcp-smol`, and
  `ulo-rpc-conformance` at its defaults and with `smol`. The controls print tokio:
  `ulo-rpc-conformance` with `tokio`, and `ulo-rpc-tcp`. `ulo-http-conformance` with `smol` alone
  prints tokio through hyper and h2, as F383 records of every hyper-based crate.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-rpc`, the seven tokio
  links, `ulo-rpc-tcp-smol`, `ulo-rpc-tcp-core`, `ulo-rpc-conformance` (at its defaults and with
  `tokio,smol`), `ulo-smol`, `ulo-http-conformance` (at its defaults and with `smol`) and
  `ulo-ws-conformance`: each exits 0 (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy` over the sixteen touched crates with `--all-targets --all-features
  --no-deps --locked` and the OpenSSL flags: exit 0, 12 warning locations outside `crates/ulo/src`,
  none on a changed line or in a new file, by batch 25's `changed_lines.py`, which reported two
  planted locations, one on a changed line and one in the new test file, and not an unchanged one
  (`verify/clippy-touched.txt`, `clippy-changed.txt`, `checker-selftest.txt`).
- **Containers:** the user's four, seaweedfs, mailpit, postgres:18 and redis:7, running before the
  broker suites and after each, beside the same eight exited containers, and no other
  (`runs/docker-before.txt`, `suites/docker-after-*.txt`, `verify/docker-after.txt`). Every
  container a suite started was removed by it. No permission check refused an action.
- **Disk:** 56 GB free at the start; 48 GB at the end. A second target directory,
  `target-b26doc` in the scratchpad, held the docs and clippy builds so they did not wait on the
  workspace test.
