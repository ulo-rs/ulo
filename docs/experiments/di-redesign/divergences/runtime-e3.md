# Divergences: runtime neutrality, stage e3: the RPC suite's runtime half, the smol TCP link on a shared runtime-free wire, and the smol CI job

The thirty-fourth response's answer 6 makes a smol TCP link part of the proof, and the
thirty-eighth response's stage e has one CI job run the HTTP suite, the WebSocket suite on both
hosts and the pipe host, and the TCP RPC suite on smol, with smol sockets. This stage builds the
RPC half. `ulo-rpc-conformance`'s `Broker` names a `Harness`, its runtime half, as the HTTP suite's
`Host` does: `runtime()` and `block_on`, `OnTokio` behind a `tokio` feature and `OnSmol` behind
`smol`. The suite's own code names no runtime: its waits go through the harness runtime's `Timer`,
its tasks through `ulo::spawn_with` on that runtime, its handlers sleep on a `Dep<dyn Timer>`
field, its slots wait on a `std` condition variable, and its relay accepts and copies on threads of
its own over `std` sockets. The five broker suites and TCP's and UDP's name `OnTokio` and are
otherwise unchanged; their containers stay on the tokio side, in the broker crates' tests. The TCP
link's runtime-free half moved to a new internal crate, `ulo-rpc-tcp-core`: the framing, the
codec's frame limit, the call-id map, the reply path, the writer queue, the accept loop, `goaway`
and the late `goaway`, the post-drain forwarder and the client connection, all over `futures-io`'s
traits and spawned through the runtime a link hands it. `ulo-rpc-tcp` is that core over tokio's
sockets, read through `tokio-util`'s `compat`, and the new `ulo-rpc-tcp-smol` is the same core over
`async-net`'s sockets and `futures-rustls`. The smol link passes the RPC suite on smol, plain, CBOR
and TLS, 30 scenarios each with the tokio link's two declarations and no other, three runs each,
and has no tokio in its normal tree. A client of either link is answered by a server of the other.
CI gains a `smol` job naming each suite that runs on smol.

Files changed: the workspace `Cargo.toml` (the members `crates/ulo-rpc-tcp-core` and
`crates/ulo-rpc-tcp-smol`) and `Cargo.lock` (the two crates; no third-party package added);
`.github/workflows/ci.yml` (the `smol` job, and the `test` job's comment); the new crates
`crates/ulo-rpc-tcp-core` (`Cargo.toml`, `src/lib.rs`, `src/link.rs`) and `crates/ulo-rpc-tcp-smol`
(`Cargo.toml`, `src/lib.rs`, `src/link.rs`, `tests/{conformance.rs, conformance_cbor.rs,
conformance_tls.rs, runtime.rs, late_goaway.rs, drain_end.rs, client_close.rs}`);
`crates/ulo-rpc-conformance/{Cargo.toml, src/lib.rs, src/relay.rs, src/rt.rs (new),
src/tokio_harness.rs (new), src/smol_harness.rs (new), tests/harness.rs (new)}` and every file
under `src/cases/` but `admission.rs`, `headers.rs`, `mod.rs`, `payloads.rs` and `unary.rs`;
`crates/ulo-rpc-tcp/{Cargo.toml, src/link.rs, tests/interop.rs (new)}`; for TCP's three suites, UDP's and the five brokers',
`tests/conformance*.rs` (one line, `type Harness = OnTokio`) and the crate's `Cargo.toml` (the
suite's `tokio` feature); in the workspace's `FRAMEWORK_GAPS.md`, F385 and F386 filed. The tree is
`cc798ddb` plus this stage. `a185656f`, `8dd0a40d` and `cc798ddb`, committed by another agent
during the stage, change the two DESIGN files and `RESPONSE.md` and nothing else.

## The signatures

```rust
// ulo_rpc_conformance
pub trait Harness: Send + Sync + 'static {                 // new
    type Runtime: Runtime;
    fn runtime() -> Self::Runtime;
    fn block_on<F: Future>(fut: F) -> F::Output;
}
pub trait Broker: Sized + Send + Sync + 'static {
    type Harness: Harness;                                  // new, required
    type Link: Link;
    /* the rest unchanged */
}
#[cfg(feature = "tokio")] pub struct OnTokio;               // new: a multi-thread runtime per scenario
#[cfg(feature = "smol")] pub struct OnSmol;                 // new: an `Executor` per scenario, four threads
// conformance_suite! stamps `#[test]`, each scenario through `Harness::block_on`; it stamped
// `#[tokio::test(flavor = "multi_thread")]`.
pub mod __private {
    impl Slots { pub fn hold(&'static self, declared: Option<NonZeroUsize>) -> Option<Slot>; }  // was `async`, a tokio permit
    pub struct Slot;                                        // new: the held slot, given back on drop
}
pub mod relay {
    impl Relay {
        pub async fn bind() -> std::net::TcpListener;       // was tokio's `TcpListener`
        pub fn listen(listener: std::net::TcpListener, upstream: SocketAddr) -> Relay;  // likewise
        // unchanged: start, without_upstream, forward_to, addr, cut, cut_for, open, last_outage
    }
    // unchanged: Outage, shadowed, unshadowed, reachable
}
// features: `tokio` = ulo-tokio, tokio with `rt-multi-thread`; `smol` = ulo-smol, async-io. tokio
// and ulo-tokio are optional; futures-channel joins the normal dependencies.

// ulo_rpc_tcp_core, new crate, doc-described as internal
pub const DEFAULT_MAX_FRAME: u64;
pub type Reader = Pin<Box<dyn futures_io::AsyncRead + Send>>;
pub type Writer = Pin<Box<dyn futures_io::AsyncWrite + Send>>;
pub trait Accept: Send + 'static {
    fn accept(&mut self) -> impl Future<Output = io::Result<Incoming>> + Send;
}
pub struct Incoming { /* private: the peer and the handshake yielding (Reader, Writer) */ }
impl Incoming {
    pub fn plain(peer: SocketAddr, reader: Reader, writer: Writer) -> Incoming;
    pub fn handshaking<F>(peer: SocketAddr, handshake: F) -> Incoming
    where F: Future<Output = io::Result<(Reader, Writer)>> + Send + 'static;
}
pub struct Core { /* private: endpoint, TLS, codec, max_frame, prepared, running state */ }
impl Core {
    pub fn new(endpoint: EndpointSpec) -> Core;
    pub fn set_tls(&mut self, tls: Tls);
    pub fn set_codec(&mut self, codec: Codec);
    pub fn set_max_frame(&mut self, bytes: u64);
    pub fn limit(&self) -> u64;
    pub fn capabilities(&self) -> Capabilities;
    pub fn prepare(&mut self, usable: Result<(), BoxError>) -> Result<(), BoxError>;
    pub fn listen<A, F>(&self, runtime: Arc<dyn Runtime>, adopt: F) -> Result<Inbound, BoxError>
    where A: Accept, F: FnOnce(BoundListener, Option<Arc<ServerConfig>>) -> io::Result<A>;
    pub fn client_addr(&self) -> Result<SocketAddr, BoxError>;
    pub fn connect(&self, runtime: &Arc<dyn Runtime>, reader: Reader, writer: Writer) -> Outbound;
    pub async fn drain(&self);
    pub async fn close(&self);
    pub fn bound(&self) -> Vec<BoundAddr>;
}

// ulo_rpc_tcp: no public change. `Tcp` holds a `Core` and its `Tokio`.

// ulo_rpc_tcp_smol, new crate
pub struct Tcp { /* private: Core, Option<Arc<dyn Runtime>> */ }
impl Tcp {
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self;
    pub fn tls(self, tls: Tls) -> Self;
    pub fn codec(self, codec: Codec) -> Self;
    pub fn max_frame(self, bytes: u64) -> Self;
    pub fn with_runtime(self, runtime: impl Runtime) -> Self;
}
impl Link for Tcp { const NAME: &'static str = "tcp"; /* the tokio link's capabilities */ }
```

Unchanged: `Link`, `RpcClient`, `RpcClientModule`, `ulo_rpc::Server`, `Budget`, `startup_failed!`,
`report`, `failures_dir`, every scenario's name and what it asserts.

Dependencies: `ulo-rpc-tcp-core` depends on `ulo`, `ulo-net`, `ulo-rpc`, `ulo-transport`, `bytes`,
`futures-channel` with `sink`, `futures-io`, `futures-util` with `io` and `sink`, and `tracing`.
`ulo-rpc-tcp` drops `bytes`, `futures-core`, `futures-util`, `ulo-transport` and `tracing` from its
normal dependencies and tokio's `sync`, `macros` and `time`, and gains `ulo-rpc-tcp-core` and
`tokio-util` with `compat`; its tests gain `ulo-rpc-tcp-smol`, `ulo-transport`, `futures-util` and
`tracing` as dev-dependencies, and tokio's `macros`, `time` and `sync` there. `ulo-rpc-tcp-smol`
depends on `ulo`, `ulo-net`, `ulo-rpc`, `ulo-rpc-tcp-core`, `async-net` 2, `futures-rustls` 0.26 and
`futures-util` with `io`, and on `ulo-rpc-conformance` with `smol`, `ulo-smol`, `ulo-test-certs`,
`ulo-transport` and `futures-executor` for its tests. Every third-party package was already in
`Cargo.lock`.

## Decisions

### 1. The RPC suite's runtime half is a `Harness` the `Broker` names

- **Shape.** `Broker::Harness: Harness`, one line per broker, carries `runtime()` and `block_on`,
  the HTTP suite's shape (`Host::Harness`) without its listener and client, which the RPC suite
  does not need: its client is the link under test and its relay is runtime-free (decision 3). The
  WebSocket suite puts the two methods on `Host` itself; a `Broker` is an environment rather than a
  host, and a separate type lets the smol hosts share one implementation. S1.
- **`OnTokio` and `OnSmol` live in the suite, behind features,** as the HTTP suite's do: `OnTokio`
  starts a multi-thread runtime per scenario, as `#[tokio::test(flavor = "multi_thread")]` did, and
  `OnSmol` an `async-executor` `Executor` per scenario run by the test's thread and three more.
  `OnSmol::runtime()` reads the executor from a thread-local set on the test's thread and on each
  worker, so a task the scenario spawns finds the runtime too; the HTTP suite's sets it on the
  test's thread alone. That is the third copy of the smol `block_on` (`ulo-smol`'s own test, the
  HTTP suite, this one), about thirty lines; a shared helper would be new public surface on
  `ulo-smol` or a fourth test-support crate. S2.
- **What tokio did in the suite, and where each use went.** The runtime per scenario: the
  harness's `block_on`. Sleeps and timeouts: the harness runtime's `Timer`, through `rt::sleep`
  and `rt::timeout` (a `select` against the timer's sleep). The handlers' sleeps, `stall`, `hold`,
  the bounded pair, `ticks` and `until_drain`: a `Dep<dyn Timer>` field on the two controllers,
  since a handler parameter does not compile (F384). Spawned waits (a held call, a close in the
  background, the server's `serve`): `ulo::spawn_with` on the harness runtime, whose
  `ValueHandle` answers `Result<T, TaskEnd>` where tokio's `JoinHandle` answered
  `Result<T, JoinError>`. `tokio::select!`: `futures_util::future::select`, which polls its left
  side first, as the one `biased` select did. `yield_now`: a ten-line future. `JoinHandle::is_finished`
  in the client-close scenario: `now_or_never` on the handle, which panics with the call's answer
  when it has one, as the assertion's message printed it. `spawn_blocking(move || thread.join())`
  in the plain-thread scenario: a wait on `JoinHandle::is_finished`, bounded as before, then the
  join, which no longer blocks. `tokio::time::Instant`: `std::time::Instant`. The slots' tokio
  `Semaphore`: a `std` condition variable, the test thread blocking before `block_on`, as the HTTP
  and WebSocket suites take theirs. The relay: decision 3. The broker containers: unchanged, in the
  broker crates' tests, which start them with testcontainers on tokio inside `OnTokio::block_on`.
- **One assertion removed.** The plain-thread scenario asserted that its `std::thread` had no tokio
  runtime current, which cannot be asked without depending on tokio. A thread `std::thread::spawn`
  starts inherits no thread-local, a runtime's context included, so the assertion could not fail;
  the scenario's doc now says the thread starts with none.
- **The suite's tree.** With neither feature, and with `smol`, `cargo tree -p ulo-rpc-conformance -e
  normal -i tokio` finds no tokio; with `tokio` it prints tokio through `ulo-tokio` and directly
  (verification). Under the forty-eighth response's two levels, the suite with `smol`,
  `ulo-rpc-tcp-core` and `ulo-rpc-tcp-smol` are runtime-free: no tokio crate in their trees.

### 2. The tokio links run the suite unchanged in result

Every tokio suite was run once on the final tree: TCP's three 30 passed and 2 ignored each, what the
plain suite gave on the new harness with the link not yet moved (`runs/tcp-suite-harness-2.txt`);
UDP 27 and 5; the brokers' counts are the ones batch 24 recorded (verification). No
scenario changed what it asserts, and no declaration was added or removed.

### 3. The relay runs on threads of its own, over `std` sockets

- **Why.** A socket belongs to a runtime, and the relay sits between a client of either runtime
  and the server. A relay on the harness's runtime would need a socket type per harness, which the
  thirty-fourth response's answer 2 keeps out of every runtime interface; `std` sockets on threads
  belong to none.
- **What it does.** One thread accepts; each relayed connection has a thread that connects to the
  upstream and copies one way while a second copies the other, as `copy_bidirectional` did: a
  direction's end shuts the other socket's writing half, a failure either way shuts both sockets.
  `cut` shuts both sockets of every connection, which ends their blocking reads, and awaits each
  connection's end through a `futures-channel` oneshot its thread drops when it returns, so the
  relay's methods stay `async` and need no runtime. A connection accepted while its upstream
  connect is in flight is marked, so a cut before the connect finishes stops the thread from
  relaying through it. Dropping the relay stops the accepting thread, woken by a connection to its
  own address, and shuts every connection without waiting.
- **A behaviour the port first lacked.** The relay keeps a handle to each client socket for `cut`,
  so a connection's thread returning, its upstream refusing the connect, left the client's
  connection open where tokio's dropped task had closed it. The tokio TCP suite's `drain` and
  `drain_window` failed on it, a call during the drain timing out where `unavailable` was due
  (`runs/tcp-suite-harness-1.txt`); each connection's thread now shuts the client's socket on every
  way out, and `tests/harness.rs` pins it (`a_connection_the_upstream_refuses_is_closed`).
- **The cost.** Two threads per relayed connection and one per relay. The suites relay a few
  connections per scenario.
- `reachable` tries its connects on a thread of its own and answers through a oneshot.

### 4. The TCP link's runtime-free half is a crate both links share

- **What moved** to `ulo-rpc-tcp-core`, from `ulo-rpc-tcp`'s `link.rs`: the constants (the default
  frame limit, the accept back-off, the three queue bounds); `prepare`'s checks, the frame limit and
  the capabilities; the server's state, its call-id map (`Ids`), `inward`, the reply path,
  `queue_reply` and `with_id`; `encode` with its `FrameTooLarge`; `read_frame` and `write_frames`;
  the accept loop with its drain and close phases; each connection's reader, writer and late
  `goaway`; `forward`, `idle` and `refuse`; the client connection's writer, reply reader and send;
  `drain` and `close`. Only the sockets stay in each link: adopting `ulo-net`'s `BoundListener`,
  accepting, the TLS handshake as an `Incoming`, connecting, and splitting a stream into a
  `Reader` and a `Writer`.
- **What it runs on.** `futures-io`'s traits; channels from `futures-channel` and
  `ulo-transport`'s ordered queue; `ulo-transport`'s `Watch` for the phase, the held-call count and
  the client epoch, where tokio's `watch` was; `ulo-transport`'s `TaskSet` for the server's
  connections and a `FuturesUnordered` of `TaskHandle`s for the client writers, where tokio's
  `JoinSet`s were; `futures-util`'s `select_biased!` where tokio's `select!` was; and every task
  spawned on the runtime the link hands it, the accept back-off slept on that runtime's timer. A
  client's reply lane ends when its stream is dropped through a oneshot's cancellation, where tokio's
  `Sender::closed` was. The server's forwarder takes a slot with `poll_ready` before `try_send`, so a
  delivery refused by a gone receiver comes back to be answered `unavailable`, as tokio's
  `SendError` returned it.
- **The tokio link on it.** `ulo-rpc-tcp` adopts the socket under its handle's `enter()`, connects
  through `Tokio::run`, and hands the core tokio's halves through `tokio-util`'s `compat`, which
  wraps each read's slice in tokio's `ReadBuf` and starts nothing; its tasks are spawned through its
  own `Tokio` as `ulo::Runtime`, so each is a `TaskHandle` and a panic in one ends it `Panicked`
  rather than as tokio's `JoinError`. Its public API, error texts and capabilities are unchanged, and
  its test targets pass (verification).
- **Why a crate rather than a module of `ulo-rpc`.** The core is TCP's: a hub carrying one
  transport's wire would be an odd home, and `ulo_rpc::__private` holds what the generated code and
  the conformance suite reach. The cost is a third published crate for the TCP links, which both
  depend on, internal by its description, as `ulo-handler-codegen` and `ulo-transport-macros` are.
  S3.

### 5. The smol TCP link

- **Name.** `ulo-rpc-tcp-smol`, the type `Tcp` and `Link::NAME` `"tcp"`, as the tokio link's: one
  wire, so a server's `LinkInfo` names the wire, whichever crate carries it, and `tests/interop.rs`
  in `ulo-rpc-tcp` has a client of each answered by a server of the other, a unary call and a
  streamed reply. S4.
- **Its runtime is any `ulo::Runtime`.** The sockets are `async-io`'s, driven from its reactor
  thread whichever executor polls them, so only the link's tasks need a runtime: `with_runtime(..)`
  takes any, `ulo_smol::Smol` on a smol app, and the interop test runs the link's tasks on tokio. A
  server's link given none takes its app's in `prepare`, `AppHandle::runtime()`, the app's
  redacting wrapper; a client's link given none is refused by `usable`, naming `.with_runtime(..)`,
  where `RpcClient::new` or `RpcClientModule`'s init hook takes it, and in `connect`. The tokio links
  capture the runtime current where they are built; smol has none to capture, and the client side
  is never shown the app (F386). S5.
- **TLS** is `futures-rustls`'s acceptor over `ulo-net`'s `Arc<ServerConfig>`, as
  `ulo-listen-smol`'s is. A plain stream is cloned into its two halves, as `async-net` streams
  allow; a TLS stream is split through `futures-util`'s `split`, a lock between the halves, as
  tokio's `split` is for the tokio link's TLS.
- **Tests beside the suites,** each ported from the tokio link's where it has one: `runtime.rs`
  (the refusals, and whose runtime a server's tasks run on), `late_goaway.rs` (TLS through
  `futures-rustls`), `drain_end.rs`, `client_close.rs`.

### 6. The smol suites on smol, with smol sockets

The three suites name `OnSmol`; the server's link is `Tcp::new(127.0.0.1:0)`, taking its app's
runtime, and the client's `Tcp::new(relay).with_runtime(OnSmol::runtime())`. TLS's bridge, which
carries the client's plain connection on over TLS, is `async-net` and `futures-rustls`, its tasks on
the scenario's runtime. The declarations are the tokio link's two, citing U17, and no other.

### 7. One CI job, `smol`

It runs, each as its own step with `--locked`: `ulo-smol`'s runtime suite; the HTTP suite against
the hyper backend on smol (`ulo-http-hyper --test conformance_smol`); the WebSocket suite against
the standalone server on smol (`ulo-ws-hyper --test conformance_smol`), and the HTTP-port and pipe
hosts (`ulo-ws --test conformance_smol --test conformance_pipe`); and the RPC suite against the smol
TCP link, plain, CBOR and TLS. `test` runs them too, inside `cargo test --workspace`; the job names
them so the smol proof is a result of its own. It ends, on a failure, with the print and upload
steps the other suite jobs have, its artifact `conformance-failures-smol`. Run here step by step
with a warm dependency cache and no incremental cache, it took 819 s, 28 s to 423 s a step, nearly
all of it compiling, each `-p` resolving features for its own package; the tests themselves take
about five seconds together (`verify/ci-smol-job-local-2.txt`). A single `cargo test` over the five
packages would share one feature resolution, but cargo applies a `--test` filter to every package
it names, and a step per suite names the suite that failed, as the broker job's steps do.

## The tests

- **`crates/ulo-rpc-conformance/tests/harness.rs`**, run with both features, which a workspace build
  turns on: `a_task_on_a_smol_worker_finds_the_scenario_s_runtime`,
  `the_tokio_harness_runs_the_scenario_on_a_tokio_runtime`,
  `a_bounded_suite_holds_no_more_slots_than_its_bound`,
  `the_relay_carries_both_ways_and_a_cut_ends_every_connection`,
  `a_connection_the_upstream_refuses_is_closed`.
- **`crates/ulo-rpc-tcp-smol/tests/`**: the three suites (30 passed and 2 ignored each);
  `runtime.rs`, `a_client_link_given_no_runtime_refuses_to_connect`,
  `a_client_on_a_link_given_no_runtime_is_refused_where_it_is_built`,
  `a_client_module_on_a_link_given_no_runtime_fails_connect`,
  `a_server_link_given_no_runtime_runs_its_tasks_on_its_app_s` and
  `a_server_link_given_a_runtime_runs_its_tasks_there` (four tasks: the accept loop, the forwarder,
  the connection and its writer); `late_goaway.rs`; `drain_end.rs`; `client_close.rs`.
- **`crates/ulo-rpc-tcp/tests/interop.rs`**: `a_smol_client_is_answered_by_a_tokio_server`,
  `a_tokio_client_is_answered_by_a_smol_server`.
- **Changed:** every RPC suite's broker names `OnTokio`. The tokio TCP link's own tests are
  unchanged and pass on the shared core.

### Before and after

Each break went through `runtime-e3/scripts/brk.py`, which replaces each span (asserting it occurs
once), runs the target, writes every touched file and `Cargo.lock` back byte for byte, and
compares a hash over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock` and `.github` and every
untracked file under `crates`. Every restore reported `ok`, the hash the one recorded before the
break: `853bace5..` for the first batch, `99f54f67..` for the three rerun after the relay tests were
bounded, the one edit between the two. A span occurring fifteen times was refused before anything
was written (`broken/00-selftest.txt`). Three first attempts are kept under `broken/superseded/`
with their specs: the relay break hung its test, which was killed after a minute, then bounded, and the break rerun; the
TLS break and the suite's planted dependency did not compile or load and were rewritten. Specs in
`runtime-e3/specs/`, output in `runtime-e3/broken/`.

| Break | Target | Result |
| --- | --- | --- |
| `OnSmol`'s workers not given the executor | `harness.rs`'s worker test | failed: "`runtime()` is called inside `block_on`, on a thread running the scenario's executor" |
| `Slots` ignoring the bound | `harness.rs`'s slots test | failed: "a third slot was taken while two of two were held" |
| the relay's connection thread not shutting the client's socket | `a_connection_the_upstream_refuses_is_closed` | failed: no end within the read timeout, `WouldBlock` |
| the relay's `cut` shutting no socket | the relay's cut test | failed: "the test did not finish within 30s" |
| the core sending no late `goaway` | `late_goaway.rs` on both links | both failed: "the late connection: no frame within 3s" (tokio), "nothing within 3s" (smol) |
| the core's forwarder never ending the inbound stream | `drain_end.rs` on both links | both failed: "the server's stop took 30.00s with no call held, against a drain window of 30s" |
| the core's `close` leaving the client epoch | `client_close.rs` on both links | both failed: "`OnModuleDestroy` hook of `RpcClient`: timed out after 10s" |
| the smol link's `prepare` taking no app runtime | `runtime.rs` | failed: `a_server_link_given_no_runtime_runs_its_tasks_on_its_app_s`, "the server's link listens: error: configuration failed with 1 error" |
| the smol link's `prepare` replacing a given runtime with the app's | `runtime.rs` | failed: "the link's runtime was handed 0 tasks" |
| the smol link's `usable` always `Ok` | `runtime.rs` | two failed: "a client was built on a link with no runtime", "a client module over a link with no runtime connected" |
| the smol link adopting its listener with no TLS | its `late_goaway.rs` | failed: the client's TLS handshake, `ConnectionReset` |
| the smol link speaking CBOR unasked | `ulo-rpc-tcp`'s `interop.rs` | both failed: `Unavailable`, "the link closed before the reply arrived" |
| tokio with `net` planted in `ulo-rpc-tcp-smol`'s dependencies | the tree check | printed `tokio v1.53.2 └── ulo-rpc-tcp-smol`, and `net` among the features |
| `tokio-util` planted in `ulo-rpc-conformance`'s dependencies | the tree check | printed `tokio v1.53.2 └── tokio-util v0.7.19 └── ulo-rpc-conformance` |

## Left for the transports DESIGN fold

Line numbers are those read on `a185656f`.

- §1's crate table: `fw-rpc-tcp`'s row (line 38) loses TCP, which is tokio's sockets over
  `fw-rpc-tcp-core`; rows for `fw-rpc-tcp-core` (the TCP wire over `futures-io`, tasks on the runtime
  a link hands it, internal) and `fw-rpc-tcp-smol` (`async-net`, `futures-rustls`, any runtime's
  tasks, a server taking its app's); `fw-rpc-conformance`'s row (line 39) gains the `Harness`,
  `OnTokio` and `OnSmol` behind features, and the relay on threads over `std` sockets.
- §5.2's conformance paragraph (line 957): `type Harness: Harness` on `Broker`; the stamped tests are
  `#[test]`s through `Harness::block_on`; `Relay::bind()` and `listen` take a `std` listener; the
  relay's threads, the slots on a condition variable; the TCP suite stamped on both links, the smol
  link's three on `OnSmol`.
- §5.3: line 1044's constructors gain `fw_rpc_tcp_smol::Tcp::new(endpoint)` with `.with_runtime(..)`;
  line 1046's "Every link is tokio-based and holds a tokio runtime of its own" holds of seven links,
  the eighth taking any runtime for its tasks, a server's its app's; the TCP row (line 1052) and the
  TLS paragraph (line 1060), `futures-rustls` on the smol link.
- §8's table: `fw-rpc-conformance` leaves the tokio-bound row (line 1313), its last clause, "runs its
  fixtures on tokio and joins the plain thread it calls a client from through `spawn_blocking`", no
  longer holding, for the no-tokio row (line 1307), tokio behind its `tokio` feature;
  `fw-rpc-tcp-core` and `fw-rpc-tcp-smol` join the no-tokio row; `fw-rpc-tcp` stays tokio-bound.
- §8's "Not built" (line 1315): the smol TCP link, the RPC suite taking a runtime and the CI job are
  built; the forbidden-dependency check is not. The closing claim stays until the job passes in CI.
  Under the forty-eighth response's two levels, `fw-rpc-tcp-core`, `fw-rpc-tcp-smol` and
  `fw-rpc-conformance` without its `tokio` feature sit at the runtime-free level, which the stage f
  check then holds them to.
- The X table (after X35, line 1389): a row for `Harness` and `Broker::Harness`, `OnTokio`, `OnSmol`,
  `fw-rpc-tcp-core`'s `Core`, `Accept`, `Incoming`, `Reader`, `Writer`, and
  `fw_rpc_tcp_smol::Tcp::with_runtime`.
- Decisions after 99 (line 1587): the RPC suite's harness (decision 1), the relay on threads
  (decision 3), the shared TCP core (decision 4), the smol link's runtime rule (decision 5).

## Needs sign-off

### S1. The RPC suite's runtime half is a type the `Broker` names

Decision 1. `type Harness = OnTokio` on each of the nine brokers. The alternative is `runtime()`
and `block_on` on `Broker` itself, as the WebSocket suite has them on `Host`.

### S2. A third copy of the smol `block_on`

Decision 1. `OnSmol` in the suite, beside `ulo-smol`'s test harness and the HTTP suite's, setting
the executor on the workers too. The alternative is a helper on `ulo-smol`, public surface, or a
test-support crate the three share.

### S3. The TCP links share a crate, `ulo-rpc-tcp-core`, and the tokio link reads through `compat`

Decision 4. Everything but the sockets moved, so the drain behaviour (the late `goaway`, F366; the
stream ending once idle) is one implementation. The tokio link now reads and writes through
`tokio-util`'s `compat` and spawns through `ulo::Runtime`. The alternatives are a module under
`ulo_rpc::__private`, which avoids a published crate, or sharing only the framing and copying the
connection logic into the smol link.

### S4. The smol link is `Tcp` with `Link::NAME` `"tcp"`

Decision 5. The same name as the tokio link's, the wire being one. The alternative is a name of
its own, `"tcp-smol"`, which a handler reading `cx.link().name()` would then see differ by
runtime.

### S5. The smol link takes any runtime; a server given none takes its app's; a client needs one

Decision 5 and F386. The alternative is refusing a server's link given none, as the tokio links
refuse one built off a runtime, which would have every smol server name a runtime the app already
has; or an SPI change giving `connect` the client's runtime (F386's candidate).

### S6. The relay belongs to no runtime: threads and `std` sockets

Decision 3. The alternative is a socket type on the harness, a listener and a connect per runtime.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-e3/`: `runs/` for the
runs during the build, `broken/` (with `summary.txt` and `superseded/`), `specs/`, `suites/` (with
`summary.txt` and `brokers-summary.txt`), `tree/` (with `summary.txt`), `verify/` and `scripts/`.
The final tree's hash, by `brk.py`'s measure, is `99f54f67..` (`verify/final-tree-hash.txt`). The
three workspace checks and the docs ran on `853bace5..`, which differs from it only by the relay
tests' bound in `tests/harness.rs`, compiled since by the workspace test run and the last clippy run;
every suite run, the workspace test run and the last clippy run are on the final tree.

- **The smol TCP suites, three consecutive runs each,** one target at a time, `--locked`, libtest's
  time (`suites/summary.txt`): plain 30 passed and 2 ignored (1.32 s, 1.33 s, 1.31 s), CBOR 30 and 2
  (1.32 s, 1.33 s, 1.31 s), TLS 30 and 2 (1.33 s, 1.32 s, 1.32 s). Every scenario that passes on the
  tokio TCP link passed on the smol one; the two ignored are the tokio link's two declarations,
  citing U17, and no scenario is declared not applicable on the smol link that is not on the tokio
  one.
- **Every tokio RPC suite once,** on the final tree, `--locked`: TCP 30 and 2 (1.32 s), TCP CBOR 30
  and 2 (1.32 s), TCP TLS 30 and 2 (1.33 s), UDP 27 and 5 (1.32 s) (`suites/summary.txt`); with
  `--features integration --test conformance` and the OpenSSL flags, NATS 31 and 1 (6.58 s), Redis
  31 and 2 (6.10 s), MQTT 36 and 1 (6.44 s), RabbitMQ with `ULO_CONFORMANCE_PARALLEL=6` 31 and 1
  (30.60 s), Kafka 32 and 1 (27.30 s) (`suites/brokers-summary.txt`), the counts batch 24 recorded
  for each. One CBOR run during the build took 5.31 s (`runs/tcp-all-core-1.txt`), run in a
  workspace still compiling; two reruns took 1.33 s and 1.32 s (`runs/tcp-cbor-time-*.txt`).
- **The links' own tests:** `cargo test -p ulo-rpc-tcp` on the shared core, 10 results, every one
  passing (`runs/tcp-all-core-1.txt`, before `interop.rs`, whose 2 passed in their own run and in the
  workspace run); `cargo test -p ulo-rpc-tcp-smol`, 9 results, every one passing
  (`runs/smol-all-1.txt`);
  `ulo-rpc-conformance` with `tokio,smol`, its unit tests 2 and `harness` 5
  (`runs/harness-bounded-1.txt`).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`verify/check-*.txt`, `scripts/diag.py`, which reads both cargo's human format and its short one,
  a cached warning being replayed in the first; given one planted location in each format it reported
  both, `verify/diag-selftest.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other (`verify/check-1.88.txt`).
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags, on the final tree: 1,047 passed, 0
  failed, 131 ignored across 195 test results (`verify/workspace-test.txt`). Stage e2's 942 plus the
  105 this stage adds: the smol link's 98 (90 suite, 5 `runtime.rs`, one each in `late_goaway.rs`,
  `drain_end.rs` and `client_close.rs`), `harness.rs`'s 5 and `interop.rs`'s 2. Ignored rises by the
  smol suites' 6 and the smol crate's `ignore` doc example. The thirteen targets added would make
  197 results where e2 counted 184; the two-result difference was not traced. The first workspace
  run stopped on a full disk, `No space left on device` while compiling
  (`verify/workspace-test-0-disk-full.txt`); `target/debug/incremental`, 84 GB of cargo's incremental
  cache, was removed, 86 GB then free, and the run repeated.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`tree/summary.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http`, `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`, `ulo-runtime-conformance`, `ulo-smol`,
  `ulo-rpc-tcp-core`, `ulo-rpc-tcp-smol`, and `ulo-rpc-conformance` at its default features and with
  `smol`, each exiting 101 with "did not match any packages" or 0 with "nothing to print". The
  positive controls print tokio: `ulo-rpc-conformance` with `tokio`, and `ulo-rpc-tcp`. The two
  planted dependencies (the breaks table) printed tokio with the planted crate as its dependent.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-rpc-tcp-core`,
  `ulo-rpc-tcp`, `ulo-rpc-tcp-smol` and `ulo-rpc-conformance`, the last at default features and with
  `tokio,smol`: each exits 0 (`verify/doc-*.txt`). Two failed first, a link to `BoundListener` in the
  core's crate doc and links to `OnTokio` and `OnSmol`, which exist only behind their features; the
  first is a qualified link now and the second code text.
- `cargo +1.98.1 clippy` over the ten touched crates, `ulo-rpc-conformance`, `ulo-rpc-tcp-core`,
  `ulo-rpc-tcp`, `ulo-rpc-tcp-smol` and the six other links, with `--all-targets --all-features
  --no-deps --locked` and the OpenSSL flags: exit 0, three warning locations outside
  `crates/ulo/src`, `ulo-rpc-conformance`'s `cases/app.rs:513`, `ulo-rpc-kafka`'s `link.rs:1431` and
  `ulo-rpc-mqtt`'s `link.rs:725`, none on a changed line or in a new file, by
  `scripts/changed_lines.py`, which, given a new file's line and a changed line as planted
  locations, reported both and not an unchanged one (`verify/checker-selftest.txt`,
  `verify/clippy-3*.txt`).
- **The smol job's steps,** run here in order (`verify/ci-smol-job-local-2.txt`): the runtime suite
  15 passed; the HTTP suite on smol 36 and 2; the standalone server on smol 31; the pipe host 31 and
  the HTTP port on smol 31; the smol TCP link's three suites 30 and 2 each. Their first run, before
  the job was final, failed the pipe host's `drain_closes_with_1001` once (`verify/ci-smol-job-local.txt`),
  filed as F385; 140 further runs of the pipe host and 80 of the two other WebSocket smol hosts passed.
- **Containers:** the five broker suites started theirs through testcontainers, which removed each.
  The user's four, seaweedfs, mailpit, postgres:18 and redis:7, were the only ones running after
  each broker suite and at the end (`suites/brokers-summary.txt`, `verify/docker.txt`). No
  permission check refused an action.
