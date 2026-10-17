# Divergences: race 2b, tests batch 38, NATS roots through a file (F417), a UDP client's host name refused where it is built (F418), a failed Kafka connect's consumer dropped off the worker (F419), and a WebSocket answer queued before the next message is read (F420)

The sixty-third response, signed off 2026-10-17, answers `race2b-tests37.md`. F417 is fixed by its
option (b): a `tls://` NATS link loads the platform's roots once per link on its runtime's blocking
pool, writes them as PEM to a file of its own in the OS temp directory and hands that file to every
connection through async-nats's `add_root_certificates`, which async-nats reads on tokio's blocking
pool at each connection and each reconnect (probed), so its own read of the store on the polling
thread no longer runs; the file is removed once the link and its connections drop it, and a store
yielding no root or a file that cannot be written is refused as MQTT's `Roots` refuses. Over TLS,
against a NATS server serving the test certificate, the suite's one-thread scenario holds the thread
to 4.5 to 4.6 ms and a reconnect forced by a relay's cut to 4.3 to 5.5 ms, under a bound of 25 ms;
with async-nats reading the store on every connection again both fail at 78.2 and 77.1 ms, and with
it read on every reconnect alone the reconnect test fails at 149.4 ms. F418 is fixed: the UDP link's
`usable` refuses a host name and an inherited endpoint, saying the link takes IP addresses only, as
the TCP links do since F415. F419 is fixed: the Kafka client's reply consumer is `Detached` from its
creation, held by a test that fails three connects after their consumer exists, 6.9 to 7.4 ms
against 50 and 110.0 ms with the drop back on the worker. F420 did not reproduce in 14,000 runs; the
cause was found by reading, a message counted out of `max_inflight` before its answer was queued,
and with that window widened by 2 ms the scenario failed as CI did 43 times in 2,000 before the fix
and none in 2,000 after it. Two gaps are new: F422, a NATS TLS connection panicking in a build that
links both of rustls's providers, and F423, a `nats://` url to a server requiring TLS, which keeps
async-nats's own read of the store.

Files changed: for F417, `crates/ulo-rpc-nats/src/link.rs`, `src/lib.rs` and `Cargo.toml`, the
workspace `Cargo.toml`, and `Misconfigured`'s doc in `crates/ulo-rpc/src/link.rs`; for F418,
`crates/ulo-rpc-udp/src/link.rs` and a new test file, `crates/ulo-rpc-udp/tests/client_address.rs`;
for F419, `crates/ulo-rpc-kafka/src/link.rs`; for F420, `crates/ulo-ws/src/connection.rs`.
`Cargo.lock` gains three dependency lines under `ulo-rpc-nats` and no package. In the workspace's
`FRAMEWORK_GAPS.md`, fix notes under F417, F418, F419 and F420, their commit written `<commit>`, F422
and F423 new, and F420 and F421 moved above F419: F420 had been written between F419's title and its
body, and F421 at the file's end. The batch started from `2f3c9ed0` and made no commit; `68f143c3`,
two files under `docs/experiments/di-redesign/integrations/`, was committed on the branch meanwhile
and touches nothing the batch touched. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

```rust
// crates/ulo-rpc-nats/src/link.rs, crate-private
pub(crate) struct Roots { pub(crate) load: fn() -> CertificateResult, pub(crate) dir: fn() -> PathBuf, written: OnceCell<Result<Arc<RootsFile>, String>> }
impl Roots {
    fn platform() -> Roots;                                              // `load_native_certs`, `std::env::temp_dir`
    async fn load(&self, runtime: &Tokio) -> Result<Arc<RootsFile>, BoxError>;
    #[cfg(test)] fn path(&self) -> Option<PathBuf>;
}
pub(crate) struct RootsFile { path: PathBuf }
impl RootsFile { fn write(loaded: CertificateResult, dir: &Path) -> Result<RootsFile, String>; }
impl Drop for RootsFile;                                                 // removes the file
impl Nats { async fn roots(&self, servers: &[ServerAddr], runtime: &Tokio) -> Result<Option<Arc<RootsFile>>, BoxError>; }
async fn open(servers: Vec<ServerAddr>, options: ConnectOptions, roots: Option<Arc<RootsFile>>) -> Result<Client, BoxError>;
// was `open(url)` and `open_with(url, options)`; `ServerSide` and `ClientSide` gain `_roots: Option<Arc<RootsFile>>`

// crates/ulo-rpc-udp/src/link.rs
impl Link for Udp { fn usable(&self) -> Result<(), BoxError>; }         // reads `address()` beside the runtime

// crates/ulo-rpc-kafka/src/link.rs, crate-private
pub(crate) client_producer: fn(&ClientConfig) -> Result<FutureProducer, BoxError>,   // a field of `Kafka`, `producer` unset
async fn route_replies(side: Arc<ClientSide>, replies: Detached<StreamConsumer>, ..);   // was `replies: StreamConsumer`
```

What changes in meaning without a signature:

- A `tls://` NATS link reads the platform's trust store once per link, off the worker, a server's in
  `prepare` and a client's at its first connect, and writes the roots a root store accepts to
  `ulo-rpc-nats-roots-<pid>-<n>.pem` in the OS temp directory, where async-nats read the store on the
  thread polling every connection and reconnect. A store yielding no root, or a file that cannot be
  written, fails a server's `bind` as `StartupError::Configure` and each client call `Internal` with
  `reason: "misconfigured_link"`, where it failed each call `Unavailable`. A `nats://` link writes
  nothing.
- A UDP client on a host name or an inherited endpoint is refused by `RpcClient::new` as
  `UnusableLink`, and a client module's init hook fails the app's `connect`, where each call failed
  `Unavailable`; the refusal says the UDP link takes IP addresses only and resolves no host name.
- A Kafka client's `connect` that fails after its reply consumer exists drops the consumer on a
  thread of its own.
- On a WebSocket connection, a message whose answer is one frame, nothing or an error stays counted
  under `max_inflight` until that answer is queued, so under a bound of one the next message's answer
  cannot be queued first. A streamed answer leaves the count when its stream arrives, as before.

## Decisions

Line numbers are those of `2f3c9ed0` unless a line says otherwise.

### 1. The premise: what async-nats reads, when, and on which thread

Read in async-nats 0.46.0: `config_tls` (`src/tls.rs:58-99`) builds a connection's TLS
configuration, and `try_connect_to` calls it for every connection (`src/connector.rs:446-452`), the
first one in `connect_with_options` and every reconnect in the connection task
(`Connector::connect`, `src/lib.rs:966`). Handed root files and no `ClientConfig`, it skips the
platform's store (`tls.rs:61`) and reads each file through `load_certs`, a `spawn_blocking` task
(`tls.rs:26-39`). Probed against a NATS 2.10.14 container serving TLS from the test certificate,
through the suite's relay (`probes/p01-p04-nats-tls-premises-run2.txt`,
`probes/p02-nats-tls-read-thread.txt`):

- **Each connection and reconnect reads the file again.** With the file removed after a first
  connection, a fresh connection failed "TLS error: No such file or directory (os error 2)", and a
  reconnect forced by cutting the relay reported "client error: nats: TLS error" until the file was
  put back, after which it reconnected.
- **Off the polling thread.** With the file a FIFO whose writer opened it only after 300 ms, a
  connection on a current-thread runtime took 310.5 ms while a ticker beside it went at most 5.1 ms
  between ticks, and a reconnect took 314.3 ms with the ticker at most 9.1 ms. A read on the polling
  thread would have held the ticker for the 300 ms.
- **What stays on the polling thread** is parsing each certificate into a trust anchor
  (`tls.rs:72-80`), building the configuration, and the handshake. With the platform's 157 roots and
  the test certificate, 237,620 bytes of PEM, ten fresh connections' longest gaps were 4.0 to 5.4 ms
  against 3.5 to 4.1 ms with the test certificate alone, and ten clients reconnecting together 6.1 to
  11.7 ms a round against 4.2 to 6.1 ms. The response's estimate holds: the read is well under the
  25 ms bound, and the parse adds about 1 ms a connection.

The probe's first version matched `connected` as a substring of `disconnected` and joined the FIFO's
writer on the runtime's own thread, which deadlocked; it was killed, its container removed by id,
and its output kept as `probes/superseded-p01-p02-hung-substring-match.txt`.

### 2. The roots file

- **When.** For a url naming a `tls://` server (`Nats::roots`, `crates/ulo-rpc-nats/src/link.rs:139`
  on the batch's tree), a server in `prepare` (`:171`) and a client at its first connect, the first
  points each side can await anything, as MQTT's. `listen` and every later connect read the kept
  file. A `nats://` link loads and writes nothing.
- **How.** `Roots` (`:892`) loads them in `Roots::load`, which runs `RootsFile::write(load(), &dir())` on the link's runtime's
  `spawn_blocking` inside a `tokio::sync::OnceCell::get_or_try_init`, MQTT's shape: the outcome is
  kept, a failure included; a task that ends because the runtime stopped is not kept; a panic is
  kept as a refusal naming it.
- **What is written.** `write` (`:951`) adds each loaded certificate to a `RootCertStore` and writes
  as PEM only those it accepts, since async-nats fails the whole connection on any certificate in
  the file that is not a trust anchor (`tls.rs:72-80`, a `collect` into one `Result`). A load
  yielding none is refused with MQTT's three messages; roots beside errors are kept and the errors
  logged at `warn`. The encoding is `base64` 0.22, already in the lock.
- **Where, and under what name.** `std::env::temp_dir()`, as
  `ulo-rpc-nats-roots-<pid>-<n>.pem`, `n` a process-wide counter, opened with `create_new`, so a
  file already at the name, a symbolic link included, is never written through; the next number is
  tried instead. A write that fails after the file exists removes it.
- **When it goes.** `RootsFile`'s drop removes it (`:1002`). The link holds it in its `OnceCell`,
  and each `ServerSide` and `ClientSide` holds a clone, since async-nats reads the file again at each
  reconnect for as long as a connection lives (S2). A client built by `RpcClient::new` closes its
  link from a task when dropped, so its file goes once that close ends.

### 3. The refusal

As MQTT's (batch 37's decision 4): `Roots::load` answers a kept failure as `ulo_rpc::Misconfigured`,
"the NATS link could not load the platform's root certificates for `tls://`: ..", which a server's
`prepare` returns, failing `bind` as `StartupError::Configure`, and which the client maps to
`Internal` with `reason: "misconfigured_link"` on each call. A failed write is the same refusal:
"could not create a file for them in `<dir>`: .." or "could not write them to `<path>`: ..".
`Misconfigured`'s doc names the NATS cases beside MQTT's.

### 4. NATS over TLS at the link level, and a reconnect

`crates/ulo-rpc-nats/src/link.rs`'s unit tests, behind `integration`, implement a `Broker`,
`TlsNats<SLOT>`, over a `nats:2.10.14` container started with `--tls --tlscert --tlskey` on the test
certificate, copied in, and a relay in front of it. Each link's loader is the platform's store plus
the test certificate, so every connection reads a file of the size the host's store writes, 158
certificates here, and each test counts its loads in a slot of its own (S3). The bound is 25 ms,
MQTT's over TLS, set through `Budget::with_stall`.

- `connecting_over_tls_does_not_stall_the_thread` runs the suite's scenario: 4.5 to 4.6 ms over
  three runs, 236 to 938 ticks in the window, the first load of the store taking most of it, and four
  loads, the server's and three clients'.
- `reconnecting_over_tls_does_not_stall_the_thread` binds a server's link and connects a client's,
  both through the relay, on one thread beside a ticker that counts each gap in the phase the test
  names when the gap ends. A request reaches the server; the relay's cut then closes both
  connections, and requests from client connections made after the cut are sent until one reaches
  the server, which needs the server's own connection to have reconnected inside async-nats,
  reading the file again, and resubscribed. Every phase stays under the bound, at least one gap
  ends while reconnecting, and each link loads once: 4.3 to 5.5 ms in every phase over three runs,
  4 to 138 ticks while reconnecting (`probes/p08-*.txt`). The reconnect is the relay's, not a
  broker restart (S4).
- **The planted loads.** With a `ClientConfig` handed to async-nats, which then reads the platform's
  store on every connection and reconnect as before the fix, the scenario fails at 78.2 ms while
  connecting a client and the reconnect test at 77.1 ms while binding the server (`broken/b04-*.txt`).
  With the store read only on every reconnect, through a `reconnect_delay_callback` that skips each
  connection's first attempt, the scenario passes and the reconnect test fails at 149.4 ms while
  reconnecting (`broken/b05-*.txt`), so the reconnect phase is held on its own.

Eight unit tests need no broker, through the loader and directory seams: the three MQTT has, an
unreadable store and an empty one on both sides; a directory that cannot be written, on both sides; a
client writing one file holding the loaded root, read back as PEM, and removing it once dropped; a
server's link removing the file `prepare` wrote though it never listened; two links writing files of
their own, each removed with its link; and a `nats://` link writing nothing.

### 5. What the fix leaves

- **A `nats://` url to a server requiring TLS.** async-nats upgrades such a connection whatever the
  scheme (`src/connector.rs:503-505`), and the link, writing roots for `tls://` alone, hands it no
  file, so the store is read on the polling thread as before: 74.3 to 115.7 ms over three connects,
  against 76.7 to 77.3 ms for a `tls://` url handed no file (p04). Filed as F423, put for sign-off as
  S1.
- **Two crypto providers.** async-nats builds its configuration with `ClientConfig::builder()`
  (`tls.rs:89`), which panics in a build enabling both of rustls's providers with neither installed.
  Built with `cargo test -p ulo-rpc-nats -p ulo-rpc-mqtt`, which unifies rumqttc's `aws-lc-rs` with
  async-nats's `ring`, a TLS connection panicked at rustls 0.23.45 `src/crypto/mod.rs:249`
  (`probes/p05-nats-tls-two-providers.txt`); built for the NATS crate alone it connected. Both paths
  through async-nats call the builder, so this predates the fix. Filed as F422, S7.

### 6. F418: the UDP link

Probed with the link as `2f3c9ed0` has it (`probes/p06-udp-host-name-at-head-0.txt`): `RpcClient::new(Udp::new("billing:7000"), ..)`
answered `Ok`, and three calls each `Unavailable`, "the udp link could not connect: `billing:7000` is
not a socket address such as `0.0.0.0:8080`", from `address()` (`crates/ulo-rpc-udp/src/link.rs:127`)
read in `connect` (`:194`), while `usable` (`:156`) checked the runtime alone. `usable` now reads
`address()` too (`:164` on the batch's tree), and `address()` (`:131`) prefixes its parse error with
"the UDP link takes IP addresses only and resolves no host name", so `RpcClient::new` answers
`UnusableLink`, "`RpcClient::new` was given a udp link it cannot use: the UDP link takes IP addresses
only and resolves no host name: `billing:7000` is not a socket address such as `0.0.0.0:8080`"
(`probes/p07-udp-host-name-fixed-0.txt`). Every refusal in `address()` moved with it, the inherited
endpoint too, as S5 of batch 37 accepted for TCP. A server's `prepare` checks the runtime and reads
the same address, so a server's refusal carries the same prefix. The UDP link has no smol counterpart:
`ulo-rpc-tcp-smol` is the workspace's one smol link. The test is F415's three, in
`crates/ulo-rpc-udp/tests/client_address.rs`.

### 7. F419: forcing the failure

`connect` makes the reply consumer, subscribes it and makes the producer in one task
(`crates/ulo-rpc-kafka/src/link.rs:300-302`). Read for a way to fail the two later steps from the
link's configuration: the base configuration carries `bootstrap.servers` and, for `ssl://`,
`security.protocol` (`Brokers::config`), the same for the consumer and the producer, and the reply
topic is a name `create_topics` accepted a step earlier, so neither `subscribe` nor the producer's
creation fails on anything a caller sets; what fails them in production is the process, out of
threads or descriptors. The test therefore plants the failure through a crate-private field,
`Kafka::client_producer` (`:98` on the batch's tree), `producer` unless a test replaces it, as MQTT's
`roots.load` is replaced (S6). The fix holds the consumer in `Detached` from its creation (`:306`)
and hands `route_replies` (`:1263`) the `Detached`, which wrapped it on entry before.

`a_connect_failing_after_its_reply_consumer_does_not_stall_the_thread`, in the link's unit tests
behind `integration`, starts a single-node KRaft broker of its own, `apache/kafka-native:3.8.0`
behind a relay as the suite's are, and on one thread beside a ticker runs three connects whose
producer is refused, each creating the reply topic and a subscribed consumer before it fails. The
longest gap was 6.9 to 7.4 ms of `Budget::stall`'s 50 over three runs; with the consumer dropped on
the worker again it was 110.0 ms (`broken/b03-*.txt`).

### 8. F420: the rate, the cause, and the fix

- **The rate.** The scenario was run as `cargo test` builds it, the test binaries of `2f3c9ed0`
  copied out so the runs could not be rebuilt under them (`f420/bin-before/sha256.txt`), on this
  host, macOS on 10 cores, in a debug build (`f420/before/summary.txt`): alone, 3,000 times each on
  smol and on the pipe with nothing else running, and 2,000 times each beside twelve `yes`
  processes; the whole suite binary, its 31 tests in parallel threads, 1,000 times each with nothing
  else running and 1,000 times each beside the twelve processes in two concurrent loops per binary.
  14,000 runs, no failure of any test. It is recorded unreproduced as it stands; the CI failure was
  on Linux, whose scheduler and core count this host does not reproduce.
- **The cause, read.** `message` (`crates/ulo-ws/src/connection.rs:1404`) called `handling.done()`
  as soon as the handler answered, before pushing the answer to the outbound queue. `done` is
  `Flight::handled` (`:992`): it lowers `handling`, gives the server's place back and notifies
  `progress`, and the read loop reads while `handling` is under `max_inflight` (`:1144`). On a
  multi-threaded executor the loop, woken on another worker, can read `open`, which waited unread
  in the socket, spawn its task, and that task, whose handler is synchronous, can queue `opened`
  before `hold`'s task, between `done` and its push, queues `held`. The executor in both smol
  suites runs `WORKERS` threads (`ulo-http-conformance`'s `OnSmol::block_on`); a current-thread
  executor polls the push before the loop, with no await between them.
- **The probe.** A 2 ms `std::thread::sleep` between `done` and the push widens that window. With it
  the scenario failed as CI did, at `limits.rs:42`, left `{"id":2,"data":"opened"}` against right
  `{"id":1,"data":"held"}`, 17 times in 1,000 on smol and 26 in 1,000 on the pipe
  (`f420/widened/summary.txt`), and through the break runner 35 in 500 and 11 in 500
  (`broken/b11-*.txt`). Every failure was that assertion.
- **Host or scenario.** The designer's §4.2 bounds "the messages one connection has queued or
  running" and says the handlers in flight never exceed it. An answer computed and not yet queued is
  still the message's work: its task has not finished and its frame has not reached the queue the
  connection writes from. The scenario's doc states its premise, that `open` runs once `hold` has
  answered. The release point was the host's defect; the scenario asserts what the bound promises
  with a bound of one (S8).
- **The fix.** `message` counts a unary, empty or failed answer out once it has queued it, and a
  streamed answer once its stream arrives, before the pump writes its items as the client takes them
  (`connection.rs:1431`, `:1439` on the batch's tree); `Handling`'s doc says so. With the same 2 ms
  sleep the scenario passed 1,000 of 1,000 on each of smol and the pipe (`f420/widened/summary.txt`)
  and 500 of 500 on each through the break runner (`broken/b12-*.txt`); without it, 2,000 of 2,000 on
  each and the whole suite binary 300 of 300 on each (`f420/after/summary.txt`). No committed test
  fails without the fix every time (S5).

## The tests

- **F417, in `crates/ulo-rpc-nats/src/link.rs`'s unit tests:**
  `a_store_that_cannot_be_read_refuses_the_server_s_bind`,
  `a_store_that_cannot_be_read_fails_every_call_misconfigured`,
  `a_store_holding_no_root_is_refused_as_misconfigured`,
  `a_file_that_cannot_be_written_is_refused_as_misconfigured`,
  `a_client_writes_its_roots_once_and_removes_them_when_it_drops`,
  `a_server_s_link_dropped_before_it_listens_removes_its_roots`,
  `two_links_write_files_of_their_own`, `a_plain_link_writes_no_roots`, and, behind `integration`,
  `over_tls::connecting_over_tls_does_not_stall_the_thread` and
  `over_tls::reconnecting_over_tls_does_not_stall_the_thread`, decisions 2 to 4.
- **F418:** `crates/ulo-rpc-udp/tests/client_address.rs`, `a_client_given_a_host_name_is_refused_where_it_is_built`,
  `a_client_given_an_ip_address_and_port_is_built` and `a_client_module_given_a_host_name_fails_connect`,
  decision 6.
- **F419, in `crates/ulo-rpc-kafka/src/link.rs`'s unit tests behind `integration`:**
  `failed_connect::a_connect_failing_after_its_reply_consumer_does_not_stall_the_thread`, decision 7.
- **F420:** the suite's `max_inflight`, stamped `max_inflight_stops_reading`, with the window
  widened by a probe edit, decision 8.

### Before and after

The probes and breaks went through `batch38/scripts/brk.py`, batch 37's script, run one spec at a
time by `batch38/scripts/breaks.sh`, which records the container list before and after each; p01 to
p05 were a test file copied into `crates/ulo-rpc-nats/tests/` for the run and removed after it, from
`batch38/probes/src/`, and p06 and p07 the same in `crates/ulo-rpc-udp/tests/`. `brk.py` replaces
each span, asserting it occurs once, runs the targets under bash, writes every touched file and
`Cargo.lock` back byte for byte, and compares a SHA-256 over `git diff HEAD` of `crates`,
`Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every untracked file
under `crates` and `scripts`. Specs in `batch38/specs/`, output in `batch38/probes/` and
`batch38/broken/` with `summary.txt` and `runner.txt`, and the F420 loops in `batch38/f420/`. Every
restore reported `ok`, and every container list after a spec matches the one before it. p01 to p05
ran on trees between, p06 and p07 on `bb221617..`, before the last doc edits; p08 and every break on
the final tree, `0fd9ca02..`.

| Run | Target | Result |
| --- | --- | --- |
| p01. a connection and a reconnect with the file removed | async-nats, TLS through a relay | the fresh connection failed "No such file or directory", the reconnect reported a TLS error until the file was put back |
| p02. the file a FIFO written after 300 ms | a connection and a reconnect beside a ticker | 310.5 and 314.3 ms; the ticker's longest gap 5.1 and 9.1 ms |
| p03. a platform-sized file | ten connections, ten rounds of ten reconnects | 4.0 to 5.4 and 6.1 to 11.7 ms; 3.5 to 4.1 and 4.2 to 6.1 ms with one certificate |
| p04. `nats://` to a server requiring TLS | three connects beside a ticker | 74.3 to 115.7 ms, as `tls://` handed no file, 76.7 to 77.3 ms (F423) |
| p05. two crypto providers | a TLS connection, built with the MQTT link | panicked at rustls `src/crypto/mod.rs:249` (F422) |
| p06. a UDP client on a host name, before the fix | `RpcClient::new` and three calls | `Ok`; each call `Unavailable`, "`billing:7000` is not a socket address" |
| p07. the same, after it | `RpcClient::new` | `UnusableLink`, "takes IP addresses only and resolves no host name" |
| p08. the stall tests, their numbers printed | NATS over TLS and Kafka's failed connects, three runs each | 4.5 to 4.6 ms and 4.3 to 5.5 ms of 25; 6.9 to 7.4 ms of 50 |
| b01. UDP's `usable` the runtime alone | `client_address` | the two host-name tests failed |
| b02. UDP's refusal without "takes IP addresses only" | `client_address` | the two host-name tests failed on the message |
| b03. Kafka's reply consumer dropped on the worker | the failed-connect test | failed at 110.0 ms |
| b04. a `ClientConfig` handed to async-nats, the store read on every connection | both TLS tests | failed at 78.2 and 77.1 ms |
| b05. the store read on every reconnect alone | both TLS tests | the scenario passed; the reconnect test failed at 149.4 ms while reconnecting |
| b06. the roots not kept | the unit and TLS tests | six unit tests failed, loads `(1, 2)` and `2`; the TLS tests counted 5 and 4 loads |
| b07. the refusal a plain error, not `Misconfigured` | the unit tests | the three client refusals failed, `(Unavailable, None)` |
| b08. the server's roots not written in `prepare` | the unit tests | four failed: the refusals came at `listen`, "transport `Rpc` failed to bind", and `prepare` wrote no file |
| b09. the file never removed | the unit tests | the three removal tests failed |
| b10. one file name per process, opened for writing | the unit tests | `two_links_write_files_of_their_own` failed, both links on one path |
| b11. the count released before the push, widened 2 ms | the scenario, 500 runs each | 35 failures on smol and 11 on the pipe, each CI's |
| b12. the fix, widened 2 ms | the scenario, 500 runs each | none |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `2f3c9ed0`.

- §5.3's TLS paragraph (line 1075): the sentence on NATS becomes the fix's: a `tls://` link loads the
  platform's roots once per link off the worker, as `mqtts://` does, writes those a root store
  accepts as PEM to a file of its own in the OS temp directory and hands it to every connection,
  which async-nats reads on tokio's blocking pool at each connection and reconnect; the file is
  removed once the link and its connections drop it, and a store yielding no root or a file that
  cannot be written is refused as `Misconfigured`. A `nats://` url to a server requiring TLS keeps
  async-nats's own read (F423), and a build linking both providers panics on a NATS TLS connection
  (F422).
- Decision 134's last clause, "NATS has no seam for the same", is superseded by a decision for the
  file, with its premise probed (decision 1 here).
- §5.3's links paragraph (line 1057): a Kafka client's reply consumer is held off the worker from its
  creation, so a `connect` failing after it drops it on a thread of its own (F419).
- §5.3's `Link` sketch and §5.4's client outside an app (line 1102): the UDP link's `usable` refuses a
  host name and an inherited endpoint, as the TCP links' do (decision 138). §12's table (line 1476):
  the UDP client's host name and inherited endpoint beside the TCP row, and a `tls://` NATS store
  yielding no root, or a file not written, beside the `mqtts://` row (line 1477).
- §4.2's load-shedding paragraph (line 866): a message stays in its connection's count until its
  answer is queued, a streamed answer until its stream arrives, so under `max_inflight = 1` the next
  message's answer is not queued before this one's (F420).
- §5.2's conformance paragraph (line 962): NATS runs the one-thread scenario over TLS at the link
  level, beside a reconnect test, as MQTT runs it (decision 137).

## Needs sign-off

Numbered from S1 for this batch.

### S1. Roots are written for `tls://` urls alone (F423)

Decision 5. async-nats also upgrades a `nats://` connection whose server's `INFO` requires TLS, and
the link, handing it no file, leaves its per-connection read of the store in place, 74.3 to 115.7 ms
on the polling thread. Writing the roots for every link would cover it, at the cost of one
off-worker read of the store and one file for every NATS link, most of which never upgrade; refusing
such a connection, naming `tls://`, is the other route. Wanted: which, or neither.

### S2. The file lives as long as the link and every connection it opened

Decision 2. The response says the file goes when the link drops. Each server and client side holds
it too, since async-nats reads it again at every reconnect for as long as a connection lives, and a
side can outlive the link while its tasks finish; a client from `RpcClient::new` closes its link from
a task, so its file goes when that close ends. The removal is a synchronous unlink in a drop, on
whichever thread drops the last holder. A failed write is kept as a failed load is, so a full temp
directory refuses the link until the process restarts.

### S3. The TLS tests read the host's own trust store

Decision 4. Each link's loader is `load_native_certs` plus the test certificate, so the file is the
size the response asked the test to show, 158 certificates and 237,620 bytes here, and the tests read
the host's store: on a host whose store is unreadable the load still yields the test certificate and
logs the rest at `warn`. The alternative is a fixed set of generated certificates of a similar size,
independent of the host.

### S4. The reconnect is forced by the relay, not by restarting the broker

Decision 4. The relay's cut closes both connections and keeps accepting, so the server's connection
reconnects inside async-nats to the same broker, and a restarted container could map a new port the
relay would not follow. The reconnect is what reads the file again; the broker's process is not.

### S5. F420 has no deterministic committed test

Decision 8. The window is a few instructions wide, so the scenario fails without the fix only when a
scheduler splits it, 0 in 14,000 here; the proof is the before-and-after rate with the window widened
by a probe edit. A crate-private hook between the answer and its push, under `conformance-probe`,
would let a test hold the window open and fail every time.

### S6. Kafka's failure is planted through a production field

Decision 7. No configuration the link takes fails the subscribe or the producer after the consumer
exists, so `Kafka::client_producer`, read only by `connect` and replaced only by a test, is the seam,
as `Roots::load` is on MQTT and NATS. A `#[cfg(test)]` field is the alternative, at the cost of a
`cfg` in `connect`.

### S7. F422: two crypto providers

Decision 5. Not fixed. The routes: the link installs a process default provider where none is, a
process-wide effect from a library; the crate's docs tell an application linking both providers to
install one; or an issue against async-nats asking it to build with the provider its own feature
names. Wanted: which, and whether an upstream issue is the user's to file.

### S8. A bound of one orders a connection's unary answers; a stream's items are not ordered

Decision 8. The fix keeps a message counted until its answer is queued, so under `max_inflight = 1`
a connection's unary, empty and failed answers leave in the order their messages arrived. A streamed
answer still leaves the count when its stream arrives, so the next message's answer can precede the
stream's items; holding the count until the stream ends would let one long stream stop the
connection's reading. §4.2 states neither order; the fold above adds the first.

### S9. Two direct dependencies for the NATS link

Decision 2. `ulo-rpc-nats` takes `rustls-native-certs` 0.8, as `ulo-rpc-mqtt` does, for the load,
and `base64` 0.22, a new workspace dependency, for the PEM encoding. Both were in the lock, async-nats
itself bringing `rustls-native-certs` 0.7, so no package was added. The root store comes from
async-nats's re-export of rustls, so the certificates are checked by the rustls async-nats reads
them with.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch38/`: `probes/` for the
probes, with `probes/src/` for the probe test files and `probes/superseded-*` for the first F417
probe, `broken/` and `specs/` for the breaks, `f420/` for the WebSocket loops and the binaries they
ran, `suites/` for the RPC suites, with `summary.txt` and the container lists before and after each,
`verify/` for the checks, the workspace test, docs and clippy, and `scripts/`. Every run below ran on
the tree hashed `0fd9ca02..`, read before the verification and after it
(`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`).

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 52 passed and 3 ignored each (3.27 s each); the
  smol TCP link's three 52 and 3 each (3.24 s each); UDP 49 and 6 (8.26 s). With `--features
  integration --test conformance` and the OpenSSL flags: NATS 54 and 1 (5.67 s); Redis 53 and 3
  (8.92 s); MQTT 59 and 1 (8.27 s); Kafka 55 and 1 (73.96 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 54 and 1 (28.94 s). Each count is batch 37's: the batch adds no
  scenario. UDP's 8.26 s is five seconds over batch 37's 3.26 s and was not seen again: the suite
  took 3.27 s in `cargo test -p ulo-rpc-udp`, 3.26 s in the workspace test, and 3.25 to 3.26 s in
  three runs on the batch's tree and three with `usable` back to the runtime alone
  (`probes/p09-*.txt`, `probes/p10-*.txt`). What held it once is not established.
- **The link unit tests with `integration`** (`verify/nats-lib-integration.txt`,
  `verify/mqtt-lib-integration.txt`, `verify/kafka-lib-integration.txt`): NATS 10 passed, the eight
  unit tests and the two over TLS, one thread at a time; MQTT 4, batch 37's; Kafka 4, the failed
  connect among them.
- **UDP's tests** (`verify/udp-tests.txt`): `client_address` 3, `client_close` 1, `close_read` 1,
  the suite 49 and 6 ignored, `drain_end` 1, `handle` 2.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `builtins` 6, `clients` 8,
  `compile_fail` 1, `deadlines` 20, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6, all
  passed, batch 37's counts; `ulo-grpc` (`verify/ulo-grpc-tests.txt`) 1 unit test and 6 doctests
  ignored.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 23 unit, `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 2 passed and 4 ignored, then the
  `compile_fail` one, batch 37's counts. **`ulo-rpc-conformance`'s**
  (`verify/rpc-conformance-tests.txt`): 2 unit, `harness` 7, 2 doctests ignored.
- **The WebSocket tests**: `ulo-ws` (`verify/ws-tests.txt`), `broadcast` 2, `conformance`,
  `conformance_pipe` and `conformance_smol` 31 each, `handoff` 11, `runtime` 1, `table` 9, 4 doctests
  ignored; `ulo-ws-hyper` (`verify/ws-hyper-tests.txt`), `abandoned_handshake` 1, `attributes` 12,
  `conformance` and `conformance_smol` 31 each, `handshake` 5, `hooks` 8, `limits` 7, `messages` 9,
  `prepare` 2, `runtime` 1, `stream_end` 1; `ulo-ws-conformance` 1 unit. All passed.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 459 files scanned, one more than batch 37, the new test
  file, 2 listed, 0 findings, exit 0 (`verify/unsafe-check.txt`).
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
  exit 0, 1,291 passed, 0 failed, 141 ignored, over 201 lines starting `test result:`
  (`verify/workspace-test.txt`). Batch 37's run had 1,280 and 141 over 200. The 11 more: NATS's
  eight unit tests and UDP's three `client_address` tests, whose new binary is the one more line;
  the TLS and failed-connect tests are behind `integration`.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`,
  `ulo-rpc-nats`, `ulo-rpc-udp`, `ulo-rpc-kafka` and `ulo-ws`: exit 0 each (`verify/doc-*.txt`).
- **Clippy against `HEAD`:** `cargo +1.98.1 clippy -p ulo-rpc -p ulo-rpc-nats -p ulo-rpc-udp -p
  ulo-rpc-kafka -p ulo-ws --all-targets --all-features --no-deps --locked` with the OpenSSL flags, on
  the batch's tree and on `2f3c9ed0` exported with `git archive` into `batch38/head-tree` and built in
  a target directory of its own: exit 0 each, 30 warning locations each, 13 outside `crates/ulo/src`,
  the same lints at the same lines, Kafka's `let_underscore_future` on `Detached`'s drop moved from
  `link.rs:1456` to `:1462` by the batch's lines above it (`verify/clippy.txt`,
  `verify/clippy-head.txt`). The batch introduced none.
- **The prose:** a scan for the banned register and for unfilled template braces, asserted first against a planted
  violation, over this log, the ledger's new text and every added line under `crates`, found none.
  The scan checks vocabulary only; the text was also read against its tests.
- **Containers and images:** at the batch's start twelve containers, the user's, every one exited
  (`verify/docker-start.txt`), and 25 images (`verify/docker-images-start.txt`). Every suite's,
  probe's and break's container list after it matches its list before, and the end's lists match
  the start's, the same twelve containers by id and the same 25 images (`verify/docker-end.txt`,
  `verify/docker-images-end.txt`). The batch's own containers were the suites' and tests'
  testcontainers, each removed by its scenario, the TLS NATS server and the KRaft broker among them,
  and one the first F417 probe left when it was killed, `23f87f7b0768` (`nats:2.10.14`), removed by
  that id. The batch pulled no image, ran no `docker ... prune` and started, stopped or removed
  nothing of the user's. Docker answered throughout. No permission check refused an action.
- **Disk:** 166 GB free when the verification started, 162 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`), the `HEAD` export's clippy build among the difference.
