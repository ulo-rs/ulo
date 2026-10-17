# Divergences: race 2b, tests batch 37, link-local fixes for blocking connects (F413, F414), a scenario that shows a connect does not stall its thread, and an example that must run (F415)

The sixty-second response, signed off 2026-10-17, answers `race2b-tests36.md`. F413 is fixed: the
Kafka link's `create_topics` holds its admin client in the link's existing `Detached`, so the drop
that joins librdkafka's polling thread runs on a thread of its own. F414 is fixed on MQTT: an
`mqtts://` link builds its TLS configuration from the platform's roots once, on its runtime's
blocking pool, and keeps it; a load yielding no root is refused at a server's `prepare` and as each
client call's `Internal` with `reason: "misconfigured_link"`, through a new link error,
`ulo_rpc::Misconfigured`. Before the fix an unreadable store panicked every call and the server's
`listen`, and an empty one failed every call `Unavailable` (probed). NATS is not fixed: async-nats
0.46 loads the native roots on every connection even when handed a `ClientConfig`, so the link has
no seam, filed as F417 and put for sign-off (S1). The new conformance scenario,
`connecting_does_not_stall_the_thread`, runs every link on one thread through a new
`Harness::block_on_one_thread` and holds the longest gap between a ticker's ticks, while a server
binds and three clients connect, under a new `Budget::stall` of 50 ms: 5.4 to 21.0 ms on all twelve
suites, 108.5 ms on Kafka with the admin client dropped on the worker again. MQTT runs it at the link
level against a Mosquitto serving TLS, where the planted per-connect load fails it at 60.7 ms; that
run found the first handshake running aws-lc-rs's one-time initialisation on its thread, about 20 ms
in a debug build, which the load now does instead. F415 is fixed: the TCP links refused a host name at each
call as `Unavailable`, and their `usable` now refuses it where the client takes the link, saying the
link takes IP addresses only; `RpcClient::new`'s example connects to `10.0.0.5:7000` and runs as a
doctest. S2's 200 ms is `budget.settle * 2 / 5`, not a literal, and was left as it is.

Files changed: for F413, `crates/ulo-rpc-kafka/src/link.rs`; for F414, `crates/ulo-rpc-mqtt/src/link.rs`,
`src/lib.rs` and `Cargo.toml`, the workspace `Cargo.toml`, and in `ulo-rpc` `src/link.rs`,
`src/lib.rs` and `src/client.rs`; for the scenario, `crates/ulo-rpc-conformance/src/lib.rs`,
`src/tokio_harness.rs`, `src/smol_harness.rs`, `src/cases/threads.rs` and `tests/harness.rs`; for
F415, `crates/ulo-rpc-tcp-core/src/link.rs`, `crates/ulo-rpc-tcp/src/link.rs`,
`crates/ulo-rpc-tcp-smol/src/link.rs`, `crates/ulo-rpc/src/client.rs` and `Cargo.toml`, and two new
test files, `crates/ulo-rpc-tcp/tests/client_address.rs` and
`crates/ulo-rpc-tcp-smol/tests/client_address.rs`; `Cargo.lock` gains three dependency lines and no
package. In the workspace's `FRAMEWORK_GAPS.md`, fix notes under F413, F414 and F415, their commit
written `<commit>`, and F417, F418 and F419 new. The batch started from `16688117`, and no commit
was made during it. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

```rust
// crates/ulo-rpc/src/link.rs, public, re-exported at the crate root
pub struct Misconfigured { pub source: BoxError }
impl fmt::Display for Misconfigured;                  // the source's text
impl Error for Misconfigured;                         // `source()` is the cause

// crates/ulo-rpc/src/client.rs, private
fn connect_error(link: &str, err: BoxError) -> RpcError;

// crates/ulo-rpc-conformance, public
pub trait Harness {
    /* `Runtime`, `runtime`, `block_on` as before */
    fn block_on_one_thread<F: Future>(fut: F) -> F::Output;          // new, required
}
pub struct Budget { pub boot: Duration, pub settle: Duration, pub recovery: Duration, pub stall: Duration }   // `stall` new
impl Budget {
    pub const fn new(boot: Duration, settle: Duration, recovery: Duration) -> Self;   // sets `stall` to 50 ms
    pub const fn with_stall(self, stall: Duration) -> Self;                          // new
}
pub async fn connecting_does_not_stall_the_thread<B: Broker>();     // in `cases::threads`
// crate-private: `const STALL: Duration` (50 ms), `const CONNECTS: usize` (3), `struct Ticks`;
// `conformance_suite!` takes an entry `name => module::case [runner]`, run through its own `@run` arms

// crates/ulo-rpc-mqtt/src/link.rs, crate-private
pub(crate) struct Roots { load: fn() -> CertificateResult, loaded: OnceCell<Result<TlsConfiguration, String>> }
impl Roots {
    fn platform() -> Roots;
    async fn load(&self, runtime: &Tokio) -> Result<TlsConfiguration, BoxError>;
}
fn configure(loaded: CertificateResult) -> Result<TlsConfiguration, String>;
impl Mqtt { async fn tls(&self, target: &Target, runtime: &Tokio) -> Result<Option<TlsConfiguration>, BoxError>; }
impl Target { fn options(&self, client_id: String, tls: Option<TlsConfiguration>) -> MqttOptions; }   // was (client_id)

// crates/ulo-rpc-tcp-smol/src/link.rs
impl Link for Tcp { fn usable(&self) -> Result<(), BoxError>; }    // was the trait's default
```

What changes in meaning without a signature:

- The Kafka link's `connect` and `listen` drop each admin client on a thread of their own, so a
  topic creation no longer stalls a worker of the link's runtime for about 100 ms.
- An `mqtts://` MQTT link reads the platform's trust store once per link, off the worker, where it
  read it on every connection on the thread polling the connect. A store yielding no root fails a
  server's `bind` as `StartupError::Configure` and each client call `Internal` with
  `reason: "misconfigured_link"`, where it panicked or failed every handshake. Its crypto provider
  is the process's installed one or `aws-lc-rs`, where rumqttc's default panicked in a build linking
  two.
- A TCP link refuses in `usable` what its client's `connect` would refuse on every call: an endpoint
  that is not an IP address and port, an inherited socket, `tls` set. `RpcClient::new` answers it as
  `UnusableLink`, a client module's init hook fails the app's `connect` with it, and a server's
  `prepare` reports `usable`'s runtime check alone, as the smol link's always did.
- A link whose connect answers `Misconfigured` fails each call `Internal` with
  `reason: "misconfigured_link"`; every other failed connect stays `Unavailable`.
- Every RPC suite has one scenario more, the first that a stamped test runs through a runner other
  than `block_on`. A `Harness` implemented outside the workspace fails to compile until it adds
  `block_on_one_thread`.

## Decisions

Line numbers are those of `16688117` unless a line says otherwise.

### 1. What each link reported on a failed root load before the batch

Probed against a local TLS listener serving the test certificate for `localhost` (the NATS listener
writes an `INFO` requiring TLS first), with `SSL_CERT_FILE` naming a missing file for an unreadable
store and `SSL_CERT_DIR` an empty directory for a store holding no root, which rustls-native-certs
reads in place of the platform's store (`probes/p01-mqtt-roots-today-*.txt`,
`probes/p02-nats-roots-today-*.txt`). Neither run reads the host's own store except the third,
which uses it.

| Link, store | Client call, three in a row | Server `bind` |
| --- | --- | --- |
| MQTT, unreadable | panics in the caller's await, "could not load platform certs: [..]" (rumqttc 0.25.1 `src/lib.rs:369`, `TlsConfiguration::default`'s `expect`, reached from `Target::options`, `crates/ulo-rpc-mqtt/src/link.rs:465`, called in `connect`, `:244`) | panics in `listen` (`link.rs:205`), uncaught by the app |
| MQTT, no root | `Unavailable`, "the MQTT link could not connect: TLS: I/O: invalid peer certificate: UnknownIssuer" | fails, "transport `Rpc` failed to bind: [..] UnknownIssuer" |
| MQTT, the platform's | `Unavailable` as above, 544 ms, then 78.2 to 79.7 ms each, the store read on every connect | fails as above after 80.1 ms |
| NATS, unreadable | `Unavailable`, "the nats link could not connect: TLS error: could not load platform certs: could not load certs from file /nonexistent/batch37-roots.pem: [..]" (async-nats 0.46.0 `src/tls.rs:61-63`, through `ClientInner::connection`, `crates/ulo-rpc/src/client.rs:620`) | fails, "transport `Rpc` failed to bind: TLS error: could not load platform certs: [..]" |
| NATS, no root | `Unavailable`, "IO error: invalid peer certificate: UnknownIssuer", 2.6 to 4.2 ms | fails, "[..] UnknownIssuer" |
| NATS, the platform's | `Unavailable` as above, 1.57 s, then 76.7 to 77.2 ms each | fails after 77.0 ms |

Every client answer is `Unavailable` or a panic. An empty store reads as a server whose certificate
is not trusted, the store unnamed. The servers fail at `bind`, which the designer's §3.2 lists among
the startup phases, MQTT's by a panic.

### 2. F413: the admin client in `Detached`

`create_topics` (`crates/ulo-rpc-kafka/src/link.rs:449-450` on the batch's tree) wraps the admin
client in `Detached` as it is created, so its drop, on every return path, runs on `drop_detached`'s
thread; `Detached`'s doc names the admin client beside the consumer. Nothing waits for the drop:
the topics are created once `create_topics` returns, and the polling thread's stop concerns no one.
`blocking` was not used, since it runs a call and answers its result, and the drop has no result.
F419 is a consumer drop of the same kind on `connect`'s error path, filed, not fixed.

### 3. F414 on MQTT: where the roots load, and what is kept

- **Where.** A server loads them in `prepare` (`link.rs:207` on the batch's tree), the first startup
  phase that can, so a store yielding none fails `bind` as `Configure` before `listen` connects; a
  client at its first connect, the first point a client's link can await anything. `listen` and
  every later connect read the kept configuration (`Mqtt::tls`, `link.rs:154`).
- **How.** `Roots::load` (`link.rs:506`) runs `configure(load())` with the link's runtime's
  `spawn_blocking` inside a `tokio::sync::OnceCell::get_or_try_init`. The load's outcome is kept,
  a failure included, since no retry mends a store; a `spawn_blocking` that ends because the runtime
  stopped is not kept, and a panic in the load is kept as a refusal naming it. A connect dropped
  while it waits on the load, a call's timeout cutting it short, leaves the blocking task running
  and the cell empty, so the next connect starts a second load: the cell keeps the first outcome to
  arrive, not the first load started.
- **What is refused.** `configure` (`link.rs:535`) adds what parses and refuses a load that yielded
  no root, naming the errors, "the platform's store holds none", or how many failed to parse. A load
  yielding roots beside errors keeps the roots and logs the errors at `warn`, where rumqttc's
  default panicked on any error; rustls-native-certs reports a failed file in a directory as such an
  error, which leaves the rest usable.
- **The provider.** `ClientConfig::builder_with_provider` with the process's installed provider, or
  `aws-lc-rs` where none is, the provider rumqttc's default features bring. rustls's `builder()`,
  which rumqttc's default called, panics when the build links two providers and none is installed;
  the link's test build links `ring` through bollard beside rumqttc's `aws-lc-rs`, the confound
  batch 36's probe met.
- **The provider's first use.** Decision 6 found that the first TLS handshake held its thread 35 to
  43 ms in a debug build where later ones held it 9 to 13 ms, and that aws-lc-rs's first draw on its
  random source took 19.1 to 20.3 ms on its own, after which the first handshake held it 9.4 to
  12.6 ms (`probes/p06-*.txt`, `probes/p07-*.txt`). `configure` draws one byte from the provider's
  random source, so that initialisation runs on the blocking pool with the load.

### 4. The client's refusal: `ulo_rpc::Misconfigured`, answered `Internal`

A client's link has two startup points, `usable`, synchronous, and its first connect. Loading in
`usable` would block the thread building the client for the length of a trust-store read, the
stall the fix removes, and an async `usable` changes the SPI for every link. The first connect can
await the blocking pool, and every failed connect was `Unavailable` (`client.rs:619-621`), which
§3.2 forbids for a permanent misconfiguration. `Misconfigured { source }` (`crates/ulo-rpc/src/link.rs:435`
on the batch's tree) follows `NoDestination` and `FrameTooLarge`, link errors the client maps by
type; `connect_error` (`client.rs:797`) answers it `Internal` with `reason: "misconfigured_link"`,
the cause in the message, and each call asks the link again, which answers from what it kept. The
kind is put for sign-off as S2.

### 5. NATS: async-nats offers no seam

The link cannot pass roots loaded once. async-nats 0.46.0 builds its TLS configuration in
`config_tls` (`src/tls.rs:58-62`) on every connection and reconnect, from `try_connect_to`
(`src/connector.rs:446-452`, called at `:473` and `:507`), and loads the native roots inline whenever
`options.tls_client_config.is_some() || options.certificates.is_empty()` (`tls.rs:61`): handing it a
`ClientConfig` through `ConnectOptions::tls_client_config` (`src/options.rs:904`) still loads them,
discards them, and fails the connect if the load fails. The only path that skips the load is root
certificates named as files with `add_root_certificates` (`options.rs:533`), read on the blocking
pool. Nothing was changed on NATS; F417 holds the evidence and the candidate fixes, and S1 asks for
one.

### 6. The scenario, and how it runs on one thread

- **What it does.** `connecting_does_not_stall_the_thread` (`crates/ulo-rpc-conformance/src/cases/threads.rs:126`
  on the batch's tree) starts the broker, spawns a ticker sleeping a tenth of `Budget::stall` between
  ticks, binds a server and connects three fresh clients, each answered by `ready`, then stops the
  ticker. It asserts every tick ran on the scenario's own thread, at least one tick, and the longest
  gap under the bound, naming what the scenario was doing when that gap ended.
- **One thread.** `Harness` gains `block_on_one_thread`, required: `OnTokio` builds a current-thread
  runtime and `OnSmol` runs its executor on the test's thread with no workers. The stamping macro
  takes an entry `name => module::case [runner]`, and this scenario's names `block_on_one_thread`.
  The tokio links capture the runtime current where `Broker::link` builds them, which inside the
  scenario is the current-thread one, so their tasks run on the scenario's thread too; the smol link
  spawns on the runtime the scenario hands it. A harness that polls a task elsewhere fails the thread
  assertion: with `OnTokio`'s runner built multi-threaded, the ticker ran on two worker threads, not
  the scenario's (`broken/b10-*.txt`). `tests/harness.rs` holds both runners to the same property
  with sixteen tasks each.
- **One tick, not two.** The first version required two ticks and failed on TCP in seven runs of
  seven, on UDP and on smol, the server's bind and three connects finishing inside one 5 ms tick; it
  passed on TCP TLS, whose handshakes gave the window three (`probes/superseded/p03-*.txt`). A stall inside a window shorter than a tick still shows, since the
  ticker's next poll follows the stall and the gap that ends there includes it.
- **What it measures.** Every link answered 5.4 to 21.0 ms against 50 (`probes/p04-*.txt`, three runs
  each, on the tree hashed `87ee91ba..`): the TCP, UDP and smol suites 5.4 to 7.2 ms, NATS, Redis,
  MQTT and RabbitMQ 6.1 to 7.3 ms, Kafka 8.2 to 21.0 ms. The window reaches 26 ticks on RabbitMQ and
  267 on Kafka, one to three elsewhere.

### 7. `Budget::stall`, a fourth field, not a fraction of `settle`

Every other wait in the suite scales from `settle`, and the response asks the same of this bound.
Kafka's broker sets `settle` to 3 s (`crates/ulo-rpc-kafka/tests/conformance.rs:140`), six times the
default, and the planted admin-client drop is about 100 ms: a bound that fails it on Kafka is at most
a thirty-first of `settle`, 16 ms at the default budget, where TCP's window alone measures 6 to 7 ms
and a host running the suites in parallel deschedules a thread for longer. `stall` is its own field,
50 ms in `Default` and in `new` whatever the other three are, set by `with_stall`. A broker slow to
answer is waited on and gives the link no reason to block, so it does not raise the bound. Put for
sign-off as S3.

### 8. MQTT over TLS at the link level

The suite's Mosquitto listens on plain `mqtt://`, and the MQTT link loads roots only for `mqtts://`.
`crates/ulo-rpc-mqtt/src/link.rs`'s unit tests, behind the `integration` feature, implement a
`Broker`, `TlsMosquitto`, over a Mosquitto 2.0.18 container whose configuration and certificate are
copied in, serving the test certificate on 8883, and run the suite's own scenario through
`OnTokio::block_on_one_thread`. Each link's loader is the seam: it returns the test certificate as
the store's one root after blocking twice the bound, as a slow trust store does. Its bound is 25 ms,
set through `Budget::with_stall`: at the suite's 50 ms, the provider's initialisation left to the
first handshake held the thread 35 to 43 ms and passed, so decision 3's last change had no test that
failed without it. At 25 ms it fails at 37.0 ms without the change (`broken/b12-*.txt`) and passes at
5.9 to 10.4 ms with it, six runs (`probes/p08-*.txt`). The test also counts the loads, four, the
server's and each client's once. Put for sign-off as S4. The failure paths are three unit tests that
need no broker, through the same seam: rustls-native-certs reading a missing file, and an empty
load.

### 9. F415: where the refusal was, and where it is

- **Before.** `Tcp::usable` (`crates/ulo-rpc-tcp/src/link.rs:91-93`) checked the runtime alone, so
  `RpcClient::new(Tcp::new("billing:7000"), ..)` succeeded, and each call failed in `connect`
  (`link.rs:110`), at `Core::client_addr` (`crates/ulo-rpc-tcp-core/src/link.rs:240`), mapped to
  `Unavailable` (`crates/ulo-rpc/src/client.rs:620`): "the tcp link could not connect: `billing:7000`
  is not a socket address such as `0.0.0.0:8080`", probed (`probes/p05-*.txt`). The smol link used
  the trait's default `usable` and refused at its connect (`crates/ulo-rpc-tcp-smol/src/link.rs:103`).
  Neither said the link takes IP addresses only.
- **After.** Both links' `usable` reads `client_addr` (`crates/ulo-rpc-tcp/src/link.rs:94`,
  `crates/ulo-rpc-tcp-smol/src/link.rs:91` on the batch's tree), and `client_addr` prefixes its parse
  error with "the TCP link takes IP addresses only and resolves no host name", so `RpcClient::new`
  answers `UnusableLink` saying so, and a client module's init hook fails `connect` with it. Every
  refusal in `client_addr` moved with it, the inherited socket and `tls` set too, each as permanent
  as the host name (S5). A server's `prepare` passes `usable`'s runtime check alone to
  `Core::prepare`, which reports the server's own endpoint, so a server keeps its message and an
  inherited socket stays a server's to take. The trait's `prepare` doc says it reports `usable`'s
  refusal where it applies to a server.
- **The example.** `RpcClient::new`'s example (`crates/ulo-rpc/src/client.rs:266`) connects to
  `10.0.0.5:7000` and runs, inside a current-thread tokio runtime its hidden lines enter, with
  `ulo-rpc-tcp` as a dev-dependency of `ulo-rpc`, a cycle cargo allows for dev-dependencies. A
  compile-only doctest would pass on a host name, which only `usable` refuses: put back on
  `billing:7000`, the doctest fails with the `UnusableLink` (`broken/b08-*.txt`).
- **UDP.** The UDP client refuses a host name at each call the same way (`crates/ulo-rpc-udp/src/link.rs:156-158`,
  `:194`), filed as F418 and not changed, the response naming the TCP link.

### 10. S2: the timer scenario's 200 ms

`a_call_s_timer_ends_with_its_bound_after_a_blocked_connect` sets its timeout as `budget.settle * 2 / 5`
(`crates/ulo-rpc-conformance/src/cases/deadlines.rs:660`), its block as half of it and its margin as a
tenth of `settle`: 200 ms is the value at the default budget, not a literal. Nothing was changed.

## The tests

- **The scenario, in `crates/ulo-rpc-conformance/src/cases/threads.rs`:**
  `connecting_does_not_stall_the_thread`, stamped on every link, decisions 6 and 7.
- **The harness, in `crates/ulo-rpc-conformance/tests/harness.rs`:**
  `the_tokio_harness_s_one_thread_runs_every_task_on_the_scenario_s_thread` and
  `the_smol_harness_s_one_thread_runs_every_task_on_the_scenario_s_thread`.
- **F414, in `crates/ulo-rpc-mqtt/src/link.rs`'s unit tests:**
  `a_store_that_cannot_be_read_refuses_the_server_s_bind`,
  `a_store_that_cannot_be_read_fails_every_call_misconfigured`,
  `a_store_holding_no_root_is_refused_as_misconfigured`, and, behind `integration`,
  `over_tls::connecting_over_tls_does_not_stall_the_thread`, decisions 3 and 8.
- **The mapping, in `crates/ulo-rpc/src/client.rs`'s unit tests:**
  `a_misconfigured_link_fails_every_call_internal`, decision 4.
- **F415:** `crates/ulo-rpc-tcp/tests/client_address.rs`, `a_client_given_a_host_name_is_refused_where_it_is_built`,
  `a_client_given_an_ip_address_and_port_is_built` and `a_client_module_given_a_host_name_fails_connect`;
  `crates/ulo-rpc-tcp-smol/tests/client_address.rs`, `a_client_given_a_host_name_is_refused_where_it_is_built`;
  and `RpcClient::new`'s doctest, decision 9.
- **F413:** the scenario on Kafka, decision 2.

### Before and after

The probes and breaks went through `batch37/scripts/brk.py`, batch 36's script with its paths moved,
run one spec at a time by `batch37/scripts/breaks.sh`, which records the container list before and
after each. `brk.py` replaces each span, asserting it occurs once, runs the targets under bash, writes
every touched file and `Cargo.lock` back byte for byte, and compares a SHA-256 over `git diff HEAD` of
`crates`, `Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every
untracked file under `crates` and `scripts`. Specs in `batch37/specs/`, output in `batch37/probes/`
and `batch37/broken/` with `summary.txt` and `runner.txt`. Every restore reported `ok`, and every
container list after a spec matches the one before it. p01 and p02 ran on `16688117` with the probes
added (tree hash `e3b0c442..`, the empty diff); p03 on `3369d568..` and `b145ffdf..`, trees between;
p04 and the first run of every break on `87ee91ba..`, before decision 3's last change
(`broken/superseded-tree-87ee91ba/`, the same outcomes, b01 at 114.6 ms and b02 at 111.4 ms against
50); p05 to p08 and the breaks the table reports on the final tree, `0be9ceb7..`.

| Run | Target | Result |
| --- | --- | --- |
| p01. MQTT on a failed root load, before the fix | client calls and a server's `bind` | decision 1's table |
| p02. NATS on a failed root load | the same | decision 1's table |
| p03. the scenario's first version | TCP, TCP TLS, UDP, smol | failed on TCP, UDP and smol, one tick in the window; the second version passed on each, 5.4 to 6.8 ms |
| p04. the scenario, its numbers printed | twelve suites three times, and MQTT over TLS | passed on every suite, decision 6's numbers; MQTT over TLS passed at 38.4 to 42.4 ms of 50 |
| p05. a TCP client on a host name, before the fix | `client_address` with `usable` and the message as `16688117` | `RpcClient::new` answered `Ok`; each call `Unavailable`, "`billing:7000` is not a socket address such as `0.0.0.0:8080`" |
| p06. MQTT over TLS twice in one process | the TLS test | 34.8 to 40.0 ms the first time, 8.9 to 11.1 ms the second |
| p07. the provider warmed before the scenario | the TLS test | first random draw 19.1 to 20.3 ms, first key share 34 to 35 µs; the scenario after it 9.4 to 12.6 ms, without it 39.5 to 42.8 ms |
| p08. MQTT over TLS on the final tree | the TLS test six times | passed, 5.9 to 10.4 ms of 25 |
| b01. the admin client dropped on the worker | the scenario on Kafka | failed at 108.5 ms, while connecting a client |
| b02. the roots loaded on every connect, on the polling thread | the TLS test | failed at 60.7 ms of 25 |
| b03. the refusal a plain error, not `Misconfigured` | MQTT's unit tests | the two client tests failed, `(Unavailable, None)` |
| b04. the load not kept | MQTT's unit tests | the store read twice by a client, `(1, 2)` and `2`; the TLS test counted five loads |
| b05. the server's load moved out of `prepare` | MQTT's unit tests | both server tests failed: the refusal came at `listen`, "transport `Rpc` failed to bind", not as `Configure` |
| b06. `Misconfigured` mapped to `Unavailable` | `ulo-rpc`'s and MQTT's unit tests | `a_misconfigured_link_fails_every_call_internal` and the two MQTT client tests failed |
| b07. TCP's `usable` the runtime alone | `client_address` | the two host-name tests failed |
| b08. the example back on `billing:7000` | `ulo-rpc`'s doctests | `RpcClient::new`'s failed with the `UnusableLink` |
| b09. the smol link's `usable` the default | smol's `client_address` | failed |
| b10. `OnTokio`'s one-thread runner multi-threaded | the scenario on TCP, the harness tests | the scenario failed, ticks on two worker threads; the tokio one-thread harness test failed |
| b11. TCP's `connect` blocking 100 ms | the scenario on TCP and TCP TLS | failed at 106.7 and 108.4 ms |
| b12. the provider left to the first handshake | the TLS test | failed at 37.0 ms of 25, while binding the server |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `16688117`.

- §5.3's `Link` sketch (lines 976-979): `prepare` reports `usable`'s refusal where it applies to a
  server; `usable` on the TCP links refuses a client endpoint that is not an IP address and port, an
  inherited socket and `tls` set. The link errors the client maps by type gain `Misconfigured`, a
  connect no retry mends, answered `Internal` with `reason: "misconfigured_link"` (F414).
- §5.3's links paragraph (line 1053): Kafka's admin client is dropped on a thread of its own, so the
  rule that its blocking librdkafka calls keep threads of their own holds for `create_topics` too,
  except F419's error path. The TLS paragraph (line 1071): an `mqtts://` link reads the platform's
  roots once per link off the worker, a server's in `prepare`, a client's at its first connect, and
  refuses a store yielding none; NATS over TLS reads them on every connection, async-nats offering no
  seam (F417).
- §5.2's conformance paragraph (line 962): the scenario `connecting_does_not_stall_the_thread`, run
  through `Harness::block_on_one_thread`, `Budget::stall`, and every link's count one higher; MQTT
  runs it over TLS at the link level.
- §5.2's paragraph on what a caller sees (line 956): the `misconfigured_link` reason. §5.4's on a
  client outside an app (line 1098): `UnusableLink` for a TCP client on a host name, the link taking
  IP addresses only (F415, F416 deferred).
- §12's table (line 1471): a TCP client's host name, inherited socket and `tls`, refused at
  `RpcClient::new` and `RpcClientModule`'s init hook; an `mqtts://` store yielding no root, refused at
  a server's `prepare` and as each client call's `Internal`.
- §8: `Runtime` unchanged, the response's condition for a blocking-work method recorded with F413,
  F414 and F416.

## Needs sign-off

Numbered from S1 for this batch.

### S1. NATS over TLS: which fix, async-nats having no seam

Decision 5 and F417. Three routes, none built: an issue or pull request against async-nats making
`tls.rs:61` read `is_none() && is_empty()`, after which the link passes a configuration built once
as MQTT's does; the roots loaded once and written to a PEM file handed to `add_root_certificates`,
which async-nats reads on its blocking pool, at the cost of a file the link creates and removes; or,
for §3.2 alone, the roots loaded once off the worker in `prepare` and at the first connect as a
check answered `Misconfigured`, leaving the per-connection read and its stall in place. Wanted:
which, if any, and whether filing upstream is the user's to do.

### S2. `Misconfigured` is answered `Internal`

Decision 4. `Unavailable` reads as retryable and §3.2 forbids it here. `BadRequest`, which a zero
timeout uses, describes the request, and a trust store is not one. `Internal`, which an unencodable
request frame uses, is the client's own permanent failure. The reason, `misconfigured_link`, carries
the distinction either way. The alternative to the type is a client-side async startup hook on
`Link`, which `RpcClientModule`'s init hook would await, moving the refusal to the app's `connect`
for a module client and leaving `RpcClient::new`'s client to its first call.

### S3. `Budget::stall` is its own field, not scaled from `settle`

Decision 7. A fraction of `settle` small enough to fail Kafka's 100 ms block is at most 16 ms at the
default budget. `stall` is 50 ms whatever a broker's other three are, and a host raises it with
`with_stall`. Wanted: whether the field stands, or the bound should scale from `settle` with Kafka's
broker setting its own.

### S4. MQTT over TLS runs at the link level, with a bound of 25 ms

Decision 8. The suite cannot reach a TLS broker, its Mosquitto serving plain `mqtt://`, so the
scenario runs from the link's unit tests against `TlsMosquitto`. Its bound is half the suite's, so
the provider's initialisation, 35 to 43 ms left to the first handshake in a debug build, fails it;
at 50 ms the change that moved it into the load had no failing test. The margin is 14.6 ms over six
runs. The alternative is a second MQTT suite over TLS, every scenario run against a TLS broker.

### S5. Every client-side refusal in `client_addr` moved to `usable`

Decision 9. The response names the host name; the inherited socket and `tls` set on a client link
were refused at each call as `Unavailable` by the same function, and moved with it. A client built
on a server's link with `tls` set is now refused at `RpcClient::new` rather than at its first call.

### S6. `Harness::block_on_one_thread` is required

Decision 6. A default answering `block_on` would let a harness without one thread pass the scenario
on a multi-threaded runtime unseen, as b10 shows it would; a required method makes every harness
state it. A `Harness` outside the workspace no longer compiles until it does.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch37/`: `probes/` for the
probes, with `probes/superseded/` for the scenario's first version, `broken/` and `specs/` for the
breaks, with `broken/superseded-tree-87ee91ba/` for their first run, `suites/` for the RPC suites,
with `summary.txt` and the container lists before and after each, `verify/` for the checks, the
workspace test, docs and clippy, with the drafts run while the code settled in `verify/drafts/`, and
`scripts/`. Every run below ran on the tree hashed `0be9ceb7..`, read before the verification and
after it (`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`).

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 52 passed and 3 ignored each (3.26 s, 3.26 s,
  3.27 s); the smol TCP link's three 52 and 3 each (3.24 s, 3.24 s, 3.25 s); UDP 49 and 6 (3.26 s).
  With `--features integration --test conformance` and the OpenSSL flags: NATS 54 and 1 (5.57 s);
  Redis 53 and 3 (7.96 s); MQTT 59 and 1 (8.09 s); Kafka 55 and 1 (63.15 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 54 and 1 (29.56 s). Each count is batch 36's and one, the new
  scenario, which passed in every suite and is declared not applicable on no link.
- **MQTT's unit tests with `integration`** (`verify/mqtt-lib-integration.txt`): 4 passed, the TLS
  scenario among them.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `builtins` 6, `clients` 8,
  `compile_fail` 1, `deadlines` 20, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6, all
  passed, batch 36's counts; `ulo-grpc` (`verify/ulo-grpc-tests.txt`) 1 unit test and 6 doctests
  ignored.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 23 unit, one more than batch 36's;
  `bad_deadline` 7, `drain_window` 3, `runtime` 11, `stream_end` 5; the doctests 2 passed and 4
  ignored, where batch 36 had 1 and 5, `RpcClient::new`'s example now running; then the
  `compile_fail` one. **`ulo-rpc-conformance`'s** (`verify/rpc-conformance-tests.txt`): 2 unit,
  `harness` 7, two more, and 2 doctests ignored.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 458 files scanned, 2 listed, 0 findings, exit 0
  (`verify/unsafe-check.txt`).
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: exit 0, 50
  members, 54 entries, 33 checked, 0 failures, 0 errors (`verify/runtime-levels.txt`).
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, both with `CFLAGS=-I/opt/homebrew/opt/openssl/include
  LDFLAGS=-L/opt/homebrew/opt/openssl/lib`: exit 0, `ulo`'s 17 known warnings and no location
  outside `crates/ulo/src` (`verify/check-all-features.txt`, `verify/check-defaults.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql --locked`, in `target/msrv-1.88`: exit 0, the same 17 and no other
  (`verify/check-1.88.txt`).
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags:
  exit 0, 1,280 passed, 0 failed, 141 ignored, over 200 lines starting `test result:`
  (`verify/workspace-test.txt`). Batch 36's run had 1,262 and 142 over 198. The 18 more: the new
  scenario in each of the seven suites the workspace test runs, the broker suites being behind
  `integration`; the two harness tests; the four `client_address` tests in two new binaries, which
  are the two more lines; `ulo-rpc`'s new unit test; `RpcClient::new`'s doctest, the one fewer
  ignored; and MQTT's three unit tests that need no broker.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`,
  `ulo-rpc-kafka`, `ulo-rpc-mqtt`, `ulo-rpc-tcp`, `ulo-rpc-tcp-core`, `ulo-rpc-tcp-smol` and
  `ulo-rpc-conformance` (`--all-features`): exit 0 each (`verify/doc-*.txt`). A draft run refused
  the scenario's doc for linking the private `CONNECTS`, batch 36's trap, and the doc now says
  three in words (`verify/drafts/doc-draft-ulo-rpc-conformance.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-conformance -p ulo-rpc-kafka -p ulo-rpc-mqtt -p
  ulo-rpc-tcp -p ulo-rpc-tcp-core -p ulo-rpc-tcp-smol -p ulo-grpc -p ulo-codegen-tests
  --all-targets --all-features --no-deps --locked`, with the OpenSSL flags: exit 0, 13 warning
  locations outside `crates/ulo/src` (`verify/clippy.txt`): batch 36's eleven at the same lines,
  and two in crates this batch adds to the run, both older than it, `let_underscore_future` on
  `Detached`'s drop at `crates/ulo-rpc-kafka/src/link.rs:1456` and `clone_on_copy` at
  `crates/ulo-rpc-mqtt/src/link.rs:838`, each present at `16688117`. The batch introduced none.
- **Containers and images:** at the batch's start twelve containers, the user's, every one exited
  (`verify/docker-start.txt`), and 25 images (`verify/docker-images-start.txt`). Every suite's,
  probe's and break's container list after it matches its list before, and the end's lists match
  the start's, the same twelve containers by id and the same 25 images (`verify/docker-end.txt`,
  `verify/docker-images-end.txt`). The batch's own containers were the suites' and tests'
  testcontainers, each removed by its scenario, the TLS Mosquitto among them. The batch pulled no
  image, ran no `docker ... prune` and started, stopped or removed nothing of the user's. Docker
  answered throughout. No permission check refused an action.
- **Disk:** 177 GB free at the start, 170 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`).
