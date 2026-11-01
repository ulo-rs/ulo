# Divergences: race 2b, tests batch 36, a call's timer anchored to its bound's end (F411), the links that block a thread while they connect, and gRPC's standing limit pinned

The sixty-first response, signed off 2026-10-17, answers `race2b-tests35.md`. F411's first part is
fixed: a call's own timer was armed for its whole bound at the first poll of its arm, which an
unbiased `select!` makes after the work's first poll half the time, so a connect that blocked its
thread pushed the caller's `Timeout` a full bound past the block. It is now armed for what is left of
the bound ending at `Spec::ends`. A new conformance scenario, sixteen requests with a 200 ms timeout
over a connect blocking 100 ms, failed on all eight links run before the fix, six to nine calls
answered 302.4 to 310.2 ms after they were awaited (1,805.0 to 1,807.6 ms on Kafka's 1.2 s), and
passes after it, every call answered within 200.6 to 203.0 ms (1,201.4 to 1,203.0 ms). F411's second
part, premise first: no shipped link resolves a name on the runtime's worker. Two links block a
thread while they connect for other reasons, Kafka dropping its admin client (102 to 110 ms on a
tokio worker, probed, F413) and MQTT over `mqtts://` loading the platform's roots (71 ms warm and
2.3 s cold, probed, with NATS over TLS by reading, F414). The `Runtime` trait has no way to run
blocking work off the worker, so nothing was added (S1). The gRPC probe is a kept test pinning F410's
standing limit, and the connect scenario's waiting half is marked a pin in the test. The fix also
left that scenario's blocking half passing without the post-connect refusal it was there to show,
so the half now blocks once the connection is made, and fails without the refusal again (S5).

Files changed: for F411, `crates/ulo-rpc/src/client.rs`; for its scenario,
`crates/ulo-rpc-conformance/src/lib.rs` and `src/cases/deadlines.rs`, which also carries the pin's
marking; for the gRPC pin, `crates/ulo-codegen-tests/tests/clients.rs`, and one doc paragraph on
`outgoing` in `crates/ulo-grpc/src/client.rs`; in the workspace's `FRAMEWORK_GAPS.md`, F413, F414
and F415 new and F411 with a fix note, its commit written `<commit>`. The batch started from
`75222381`, and no commit was made during it. Versions are unchanged at `0.1.0-alpha.1`, and
`Cargo.lock` is unchanged.

## The signatures

```rust
// crates/ulo-rpc/src/client.rs, private
struct Limit { ends: Instant, passed: &'static str }          // was { after: Duration, passed }
impl Limit {
    fn call(ends: Option<Instant>) -> Option<Limit>;            // was Option<Duration>
    fn frame(idle: Option<Duration>, ends: Option<Instant>, now: Instant) -> Option<Limit>;
}
impl Spec { fn limit(&mut self, whole: Whole) -> Result<Option<Instant>, RpcError>; }
                                                                // was Result<(Option<Duration>, Option<Instant>), _>

// crates/ulo-rpc-conformance, crate-private
const BLOCKED_CALLS: usize = 16;
const UNANSWERED: &str = "10000";
pub async fn blocked_connect_s_timer_ends_with_its_bound<B: Broker>();   // in the private `cases` module
// cases/deadlines.rs, private to the module
enum Hold { Waits(Duration), Blocks(Duration), BlocksConnected(Duration) }   // `BlocksConnected` new

// crates/ulo-codegen-tests/tests/clients.rs, test-private
struct Ran { count: Arc<AtomicUsize>, deadlines: Arc<Mutex<Vec<Option<Instant>>>> }   // was Ran(Arc<AtomicUsize>)
struct SlowConnector { inner: HttpConnector, held: Duration, connects: Arc<Mutex<Vec<(Instant, Instant)>>> }
impl Service<Uri> for SlowConnector;
fn grpc_timeout(value: &str) -> Duration;
fn signed_millis(a: Instant, b: Instant) -> f64;
```

No public signature changes. What changes in meaning without a signature:

- A call of any shape answered `Timeout` by its own timer is answered at its bound's end, or at the
  first poll after work that blocked its thread past that end. Before the batch, when the
  `select!` in `bounded` polled the work first, the timer was armed for the whole bound only once
  the work's first poll returned, so a block of `b` moved the answer to `b` plus the bound. A
  stream's frame wait is the same wait and is anchored the same way.
- `outgoing`'s doc states the standing limit: its remaining time is read before the request reaches
  the `Channel`, which connects lazily, so on a new or re-established connection the callee is
  given up to the connect's length beyond the caller's deadline.
- The RPC conformance suite has one scenario more, so every link's count grows by one. It mounts no
  new handler. The connect scenario's blocking calls now block after their connection is made, not
  before.

## Decisions

Line numbers are those of `75222381` unless a line says otherwise.

### 1. F411's first part: the timer armed for what is left of the bound

- **What it read.** `bounded` (`crates/ulo-rpc/src/client.rs:833`) built its timer arm as
  `timer.sleep(limit.after)` (`client.rs:847`) inside an `async` block, which runs at its first poll,
  and its `select!` (`client.rs:861`) shuffles its three arms with Fisher-Yates
  (futures-util 0.3.34 `src/async_await/random.rs:13-17`), so the work comes before the timer half
  the time. tokio's `sleep` takes its deadline when called and smol's creates its timer when first
  polled; both count from the arm's first poll.
- **The fix.** `Limit` carries the instant its wait ends, `Spec::ends` for a call's whole bound
  (`client.rs:804` on the batch's tree), and the arm sleeps for what is left of it when it is first
  polled, answering at once when nothing is (`client.rs:854`). The response anchors the timer to
  `Spec::ends`; the `Timer` trait has `sleep(Duration)` and `now()` and no sleep to an instant, so the
  remainder is read from the clock at the arm's first poll. This works on both runtimes whenever
  each creates its timer, where F411's candidate fix, creating the sleep before the `select!`, would
  have left smol counting from its first poll. `Spec::limit` now answers the instant alone, since
  nothing read the duration it answered beside it. A stream's frame wait, an idle timeout or the
  stream's deadline, is anchored at the instant computed just before its `bounded`.
- **Before and after, batch 35's probe.** Sixteen requests with a 100 ms timeout, each on a fresh
  client over a link whose `connect` blocks 60 ms, to no server: before, six answered after 101 to
  102 ms and ten after 164.8 to 167.4 ms (batch 35's `broken/p02-f411-late-timer-0.txt`); after,
  all sixteen after 101.0 to 103.6 ms (`broken/p03-batch35-f411-probe-after-fix-0.txt`).

### 2. The kept scenario, and why it differs from the probe in two respects

- **What it does.** `a_call_s_timer_ends_with_its_bound_after_a_blocked_connect`
  (`cases/deadlines.rs:657` on the batch's tree): sixteen requests, each the first call of a fresh
  client whose link is the `Hold::Blocks` shape of batch 35's `SlowConnect`, to the stalling
  handler. Each must answer `Timeout` no later than the later of the bound's end and the block's
  end, plus a tenth of the settle budget, after it was awaited. Sixteen calls each have half a
  chance of a late timer, so a timer started late goes unseen with a chance of one in 65,536.
- **Not the server's own deadline.** The first version sent the calls to the stalling handler
  directly and passed against the code before the fix, every call answered 101.5 to 102.9 ms
  (`before/superseded/before-tcp-server-answers-its-deadline.txt`). F410's stamp gave the server
  what was left at the write, 40 ms, so the server's own `timeout` answered each call at its bound,
  ahead of the late timer. The probe had no server. The scenario now puts batch 35's public
  `Stamped` link below `SlowConnect`, writing the server a ten-second `deadline-ms`, so only the
  client's timer can end the call, and the client's `cancel` stops the handler.
- **200 ms and a 100 ms block, not the probe's 100 ms and 60 ms.** At a fifth of the settle budget
  with a block of three fifths of it, the scenario failed before the fix on seven links and passed
  on RabbitMQ (`before/superseded/block-three-fifths-of-a-fifth/`). There lapin's connect takes 45
  to 50 ms after the block, so the connect returned past the 100 ms bound and F410's refusal in
  `written` answered `Timeout` at 101.4 to 109.7 ms, ahead of a late timer
  (`p02-f411-numbers-before-rabbitmq-0.txt` there). The timeout is two fifths of the settle budget
  and the block half of it, which leaves the link's own connect half the timeout to finish in, so
  the frame is sent and the timer alone ends the call on every link.
- **The bound read as the later of the two ends.** The response's test reads "no more than the
  block's end plus one poll". With a block shorter than the timeout, which is the only case a late
  timer can show (decision 3), the bound's end is the later and the timer must fire there; the
  block's end governs only a block that outlasts the bound, where F410's refusal already answers
  at the connect's return. The scenario asserts the later of the two.
- **The margin.** A tenth of the settle budget, 50 ms at the default budget and 300 ms on Kafka.
  Measured after the fix, every call was answered 0.6 to 3.0 ms past its timeout on every link
  (`broken/p02-f411-numbers-after-fix-*.txt`), and before it a late call was answered about half a
  timeout past the margin's end. The margin is wide for the workspace test, which runs the TCP, UDP
  and smol suites with their scenarios in parallel.

### 3. F411's second part: does any shipped link block a thread while it connects?

None resolves a name on the runtime's worker. Two block for other reasons.

| Link | `connect` | Name resolution | Blocks a thread? |
| --- | --- | --- | --- |
| TCP, tokio (`crates/ulo-rpc-tcp/src/link.rs:109-117`) | `TcpStream::connect(addr)` on the link's runtime | IP only: `Core::client_addr` (`crates/ulo-rpc-tcp-core/src/link.rs:239-248`) takes `resolve_as_written`, `SocketAddr`'s parse (`crates/ulo-net/src/endpoint.rs:27-32`, `:118-123`) | no |
| TCP, smol (`crates/ulo-rpc-tcp-smol/src/link.rs:102-108`) | `async_net::TcpStream::connect(addr)` | IP only, the same `client_addr` | no |
| UDP (`crates/ulo-rpc-udp/src/link.rs:193-202`) | `UdpSocket::bind` and `connect` on the link's runtime | IP only: `address` (`link.rs:127-136`) | no |
| NATS (`crates/ulo-rpc-nats/src/link.rs:200-212`) | `ConnectOptions::connect` through `Tokio::run` | the library's async path: `ServerAddr::socket_addrs` (async-nats 0.46.0 `src/connector.rs:178`, `src/lib.rs:1656-1658`) is `tokio::net::lookup_host`, which resolves a name on tokio's blocking pool (tokio 1.53.2 `src/net/addr.rs:162-183`, `:192-220`) | over TLS, by reading: `config_tls` loads the native roots inline (`src/tls.rs:58-62`), F414 |
| Redis (`crates/ulo-rpc-redis/src/link.rs:143-155`) | `ConnectionManager::new` and `get_async_pubsub` through `Tokio::run` | the library's async path: `DefaultAsyncDNSResolver` (redis 1.7.1 `src/aio/mod.rs:245-256`) is `tokio::net::lookup_host` (`:259-266`), called from `connect_simple` (`src/aio/connection.rs:21`); the `to_socket_addrs` in `src/connection.rs:884`, `:948`, `:999` is the blocking client's, which the link does not use | no |
| MQTT (`crates/ulo-rpc-mqtt/src/link.rs:239-244`) | `AsyncClient::new` inline, the connect on the event loop's task | the library's async path: `socket_connect` (rumqttc 0.25.1 `src/eventloop.rs:321-325`, called from `src/v5/eventloop.rs:329`, `:336`) is `tokio::net::lookup_host` | over `mqtts://`, probed: `Target::options` (`link.rs:456-468`) loads the platform roots inline (rumqttc `src/lib.rs:366-370`), F414 |
| RabbitMQ (`crates/ulo-rpc-rabbitmq/src/link.rs:207-216`) | `Connection::connect_uri` through `Tokio::run` | the library's async path: lapin 4.12.0 `src/connection.rs:351-366` calls amq-protocol-tcp 10.6.3's `connect_with_config_async` (`src/lib.rs:128-145`), whose resolution, `hickory-dns` being off, is async-rs 0.8.13's `to_socket_addrs`, run with `spawn_blocking` (`src/util/addr.rs:62-69`) | not found by reading; the `amqps://` path's platform verifier was not read |
| Kafka (`crates/ulo-rpc-kafka/src/link.rs:284-302`) | `create_topics`, two `config.create()` and `subscribe` through `Tokio::run` | librdkafka's own threads: a broker thread resolves its host when it connects (rdkafka-sys 4.10.0 `librdkafka/src/rdkafka_broker.c:1099-1129`, `:4606-4608`); the one resolution on the creating thread is behind `client.dns.lookup=resolve_canonical_bootstrap_servers_only` (`:5480-5490`), which the link never sets | yes, probed: the `AdminClient`'s drop joins its polling thread, 102.2 to 110.4 ms on a `tokio-rt-worker` per `create_topics`, F413 |
| gRPC channel (`crates/ulo-grpc/src/client.rs:172-189`) | `Endpoint::connect_lazy`, the connect driven by the first request | the library's async path: tonic 0.14.6 builds an `HttpConnector` with hyper-util's `GaiResolver` (`src/transport/channel/endpoint.rs:540-541`), which resolves on tokio's blocking pool (hyper-util 0.1.21 `src/client/legacy/connect/dns.rs:117-118`) | the system roots for `https` load once, in the client's factory when the app connects (`client.rs:183-189`), not per connect |

`prepare` resolves no name on any link: the TCP and UDP links parse a socket address, and the
brokers parse a URL. `listen`, for the same links: the TCP links bind a parsed address through
`BoundListener::bind` (`crates/ulo-rpc-tcp-core/src/link.rs:224`, a non-blocking socket made by
`std`'s bind); UDP binds a parsed address (`link.rs:177`); the brokers' `listen` go through the same
library paths as their `connect`, and Kafka's blocks as its `connect` does, twice per `bind`
(`link.rs:243-244`); MQTT's `listen` loads the roots as its `connect` does (`link.rs:205`).

The probes: `probes/p01-kafka-connect-blocking-0.txt`, timing each `AdminClient` drop and the
other blocking calls in Kafka's `connect` on a live broker, and `probes/p04-mqtt-tls-roots-0.txt`,
timing `Target::options` for an `mqtts://` and an `mqtt://` URL, 2.31 s cold and 71.4 to 71.8 ms
warm against 1.5 to 2.4 µs. The probe's first run failed before timing anything: the test build
links both of rustls's providers, bollard's `ring` beside rumqttc's `aws-lc-rs`, so rustls chose
neither (`probes/superseded/p04-mqtt-tls-roots-0-no-crypto-provider.txt`); the second installs
`aws-lc-rs` first. A build of the MQTT link alone enables one provider, so this is not a defect of
the link.

### 4. The `Runtime` trait has no way off the worker

`Runtime` is `Timer + Spawn` (`crates/ulo/src/runtime.rs:24-38`): `Timer` has `sleep` and `now`
(`crates/ulo/src/timer.rs:21-24`) and `Spawn` has `spawn`, a future on the runtime's executor. A
blocking call spawned there blocks one of its workers as surely as one polled inline. Nothing was
added; see S1. Neither F413 nor F414 needs the trait for its own fix: the Kafka link already runs
librdkafka's other blocking calls on threads of its own and can drop its admin client the same
way, and MQTT's roots can be loaded once rather than per connect. Neither was built, the response
having asked for the premise and named the trait as the route.

### 5. gRPC's standing limit, pinned

- **The test.** `a_new_connection_gives_the_server_the_connect_beyond_the_caller_s_deadline_and_no_more`
  (`crates/ulo-codegen-tests/tests/clients.rs:353` on the batch's tree), batch 35's probe kept: three
  rounds, each the first call over a `Channel` of its own built by `connect_with_connector_lazy`,
  its connector waiting 250 ms before an `HttpConnector`, from an execution with 2 s left forwarded
  through `outgoing`. The connector records when each connect began and ended; the server's
  handler records the deadline it was given, and the deadline less the `grpc-timeout` written is the
  instant the server read the header. The test asserts the server's deadline no later than the
  caller's plus the connect, plus the transit from the connect's end to the server's read, plus
  1 ms, the allowance S2 of batch 35 set.
- **"The connect hold" read as measured.** The connector's own measure, from its start to its end,
  stands in for the hold, since it carries the 250 ms wait, tokio's timer rounding and the TCP
  connect. What the 1 ms then covers is the time between `outgoing`'s read and the connector's
  start, tonic handing the request to its buffer's worker, and the truncation of `grpc-timeout` to
  its unit. A stamp written earlier than the call, or a longer wait before tonic starts the connect,
  fails it.
- **What it shows.** Thirty rounds run alone and sixty within the whole test binary
  (`broken/p05-grpc-pin-numbers-0.txt`, `broken/p06-grpc-pin-numbers-whole-binary-0.txt`): the
  server's deadline 252.4 to 256.0 ms past the caller's, the connect 251.6 to 254.3 ms and the
  transit 0.4 to 2.8 ms, leaving 0.02 to 0.6 ms of the 1 ms used.
- **The break.** `outgoing` writing 50 ms more than is left (`broken/final-tree/b02-*.txt`): the pin
  failed in every round, 303.4 to 304.9 ms past, and so did
  `outgoing_forwards_the_execution_s_remaining_deadline`.
- **Doc.** `outgoing`'s doc states the limit, as the response asks the design to. Neither DESIGN.md
  was edited.

### 6. The connect scenario's waiting half, marked, and its blocking half moved after the connection

- **The marking.** `connect_past_the_deadline_is_refused_before_the_server_sees_it` says in its doc
  and at its loop that the `Hold::Waits` half pins behaviour that predates the post-connect check
  and only the blocking half is evidence for `written`, as batch 35's before-runs showed.
- **F411's fix took the blocking half's evidence away.** With `written`'s refusal removed, the
  scenario passed on TCP once and then twelve times in twelve
  (`broken/final/b03-no-refusal-after-the-connect-0.txt`, `b04-no-refusal-after-the-connect-repeated-0.txt`).
  `Hold::Blocks` blocks ahead of the link's own connect, which then awaits: the poll that ends the
  block returns pending, and the call's timer, now armed for what is left of the bound rather than
  for all of it, is due at the next poll and answers `Timeout` before the connect hands the frame
  over. Before the fix the timer was armed afresh after the block and the connect finished first.
- **The evidence put back.** A third hold, `Hold::BlocksConnected`, makes the connection and then
  blocks, so the connect returns in the poll that ends the block and the call writes its frame in
  that poll, which no timer can preempt. The scenario's evidence half uses it, and its doc says
  why. With the refusal removed it fails on every run: every blocked call reached the server, four
  of four on TCP in each of four runs, on the smol link and on NATS, two of two on UDP
  (`broken/final-tree/b05-*.txt`). `Hold::Blocks` stays the shape of F411's scenario, where the
  block has to delay the call's first poll.

### 7. tonic's deferred-fix premises, seen in passing

The first is answered by the code the batch's test already compiles: the generated client is
`impl<T> ClockClient<T> where T: tonic::client::GrpcService<tonic::body::Body>, T::Error:
Into<StdError>, T::ResponseBody: Body<Data = Bytes> + Send + 'static` (the build's
`codegen.probe.v1.rs:589-593`), so a service of ulo's own in place of `Channel` satisfies it if it
meets those bounds. The second, whether hyper's `http2::SendRequest::ready()` resolves only once the
connection is established, was not met in passing and was not read for.

## The tests

- **F411, one conformance scenario in `crates/ulo-rpc-conformance/src/cases/deadlines.rs`:**
  `a_call_s_timer_ends_with_its_bound_after_a_blocked_connect`, decisions 1 and 2.
- **F410's gRPC limit, one test in `crates/ulo-codegen-tests/tests/clients.rs`:**
  `a_new_connection_gives_the_server_the_connect_beyond_the_caller_s_deadline_and_no_more`,
  decision 5.
- **Changed:** `a_call_whose_connect_outlasts_its_deadline_is_refused_before_the_server_sees_it`,
  its doc, one comment and its blocking half's hold, now `Hold::BlocksConnected`, decision 6;
  `Clock::quick` in `clients.rs` records its deadline.

### Before and after

The scenario was run alone against the client as `75222381` has it, before the fix was written
(`before/`, `batch36/scripts/before.sh`). The breaks and probes went through `batch36/scripts/brk.py`,
batch 35's script with its paths moved, which replaces each span, asserting it occurs once, runs the
targets under bash, writes every touched file and `Cargo.lock` back byte for byte, and compares a
SHA-256 over `git diff HEAD` of `crates`, `Cargo.toml`, `Cargo.lock`, `.github`, `scripts` and
`unsafe-allowed.txt` and every untracked file under `crates` and `scripts`. Specs in
`batch36/specs/`, output in `batch36/broken/` and `batch36/probes/` with `summary.txt`. Every restore
reported `ok`. The before-runs ran with the scenario added and `client.rs` as `75222381` has it; two earlier
shapes of the scenario, each of which passed somewhere before the fix, are kept in
`before/superseded/` (decision 2). The probes ran on the tree hashed `0a331635..` (p01 to p04)
and `165fac4c..` (p05, p06); b03 and b04 on `99a056dc..` (`broken/final/`), the tree before the
connect scenario's evidence half moved; and b01, b02 and b05, as the table reports them, on the
final tree, hashed `de24020f..` (`broken/final-tree/`). b01 and b02 also ran first on `165fac4c..`
and b05 on `3642dde5..`, with the same outcomes (`broken/`, `broken/final/`).

| Run | Target | Result |
| --- | --- | --- |
| before F411, TCP | the scenario | failed: nine of sixteen answered 302.4 to 307.6 ms, seven 201.3 to 202.4 ms (`before/tcp.txt`) |
| the same, UDP | the scenario | failed: six at 303.6 to 307.5 ms, ten at 201.4 to 202.7 ms (`before/udp.txt`) |
| the same, smol TCP | the scenario | failed: seven at 303.0 to 306.6 ms, nine at 200.4 to 201.2 ms (`before/smol.txt`) |
| the same, NATS | the scenario | failed: eight at 302.9 to 308.1 ms, eight at 200.8 to 202.8 ms (`before/nats.txt`) |
| the same, Redis | the scenario | failed: six at 302.4 to 308.3 ms, ten at 201.3 to 202.8 ms (`before/redis.txt`) |
| the same, MQTT | the scenario | failed: eight at 305.3 to 310.2 ms, eight at 200.3 to 202.9 ms (`before/mqtt.txt`) |
| the same, RabbitMQ | the scenario | failed: eight at 303.9 to 308.3 ms, eight at 200.8 to 202.8 ms (`before/rabbitmq.txt`) |
| the same, Kafka | the scenario | failed: eight at 1,805.0 to 1,807.6 ms, eight at 1,201.4 to 1,202.8 ms, a 1.2 s timeout (`before/kafka.txt`) |
| p01. Kafka's connect timed | `unary_round_trip` and the F410 measurement scenario | six `AdminClient` drops of 102.2 to 110.4 ms on `tokio-rt-worker`; consumer and producer creation 0.26 to 0.50 ms; `subscribe` 37 to 45 µs |
| p02. the scenario, its report printed | eight links | passed on each, every call 200.6 to 203.0 ms, Kafka's 1,201.4 to 1,203.0 ms |
| p03. batch 35's F411 probe | `ulo-rpc` unit | all sixteen `Timeout`s at 101.0 to 103.6 ms |
| p04. MQTT's options for `mqtts://` timed | `ulo-rpc-mqtt` unit | 2.31 s, then 71.4 to 71.8 ms; `mqtt://` 1.5 to 2.4 µs |
| p05, p06. the gRPC pin, its report printed | the test alone ten times, the whole binary twenty | passed every time; decision 5's numbers |
| b01. the timer counted from its first poll again | the scenario on TCP, smol TCP and NATS | failed on each: seven, six and eight calls at 303.3 to 313.8 ms (`broken/final-tree/b01-*.txt`) |
| b02. `outgoing` writing 50 ms more | `clients` | the pin failed, 303.4 to 304.9 ms past; `outgoing_forwards_the_execution_s_remaining_deadline` failed too (`broken/final-tree/b02-*.txt`) |
| b03, b04. `written` without its refusal, the blocking half still `Hold::Blocks` | the connect scenario on TCP, thirteen runs | passed every run: the evidence half no longer reached the refusal (decision 6) |
| b05. `written` without its refusal, the blocking half `Hold::BlocksConnected` | the connect scenario on TCP four times, smol TCP, UDP, NATS | failed on every run, every blocked call delivered (`broken/final-tree/b05-*.txt`) |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `75222381`.

- §5.2's client paragraph on bounds: a call's own timer fires at the end of the bound `limit`
  measured, armed for what is left of it when first polled, so work blocking the call's first poll
  delays the `Timeout` to the poll after the block and no further (F411).
- §5.2's conformance paragraph: the scenario `a_call_s_timer_ends_with_its_bound_after_a_blocked_connect`,
  its `Stamped` server deadline, and the count grows by one; the connect scenario's waiting half is
  a pin of earlier behaviour, and its blocking half, which blocks once the connection is made, the
  evidence for the post-connect stamp.
- §5.4's link paragraph (line 1053), "Kafka's blocking librdkafka calls keep their threads of their
  own (57)": it holds for `fetch_metadata`, the offset reads, the commit and a consumer's drop, and
  not for the `AdminClient`'s drop in `create_topics` (F413). The same paragraph gains, per link,
  where a host name is resolved: on tokio's or async-rs's blocking pool, on librdkafka's threads,
  or nowhere, the TCP and UDP clients taking a socket address alone (F415).
- §5.4: an `mqtts://` MQTT link and a TLS NATS link read the platform's trust store on a worker for
  every connection (F414).
- §6.3's client paragraph (line 1190): the limit as `outgoing`'s doc now states it, the pin test,
  and F410's gRPC fix deferred with its cost, the generated clients taking any `GrpcService` meeting
  the bounds decision 7 quotes.
- A decision beside batch 35's on F410: F411's anchoring, the probe's numbers before and after, and
  why the kept scenario gives the server a deadline beyond the call's.

## Needs sign-off

Numbered from S1 for this batch.

### S1. A way to run blocking work off the runtime's worker

Decisions 3 and 4. No link resolves a name on a worker, the premise the response named. Two links do
block a worker while they connect: Kafka for about 100 ms per admin client, on every new client
connection and twice per `bind` (F413), and MQTT over `mqtts://` and, by reading, NATS over TLS for
a trust-store read, 71 ms warm and 2.3 s cold on macOS (F414). `Runtime` offers `spawn` and the
clock and nothing else, so by the response's rule nothing was added. The two defects can each be
fixed without the trait: the Kafka link through its own `Detached`, as its consumers are dropped,
and MQTT by building its TLS configuration once, off the call path, as `ulo-grpc` loads its roots.
What a trait method such as `spawn_blocking` would add is a route for a link with no thread of its
own and for the TCP and UDP clients if they are to take host names (F415). Wanted: whether F413 and
F414 are fixed link-locally in a later batch, and whether a blocking facility belongs on `Runtime`.

### S2. The scenario sets a 200 ms timeout and gives the server a deadline of its own

Decision 2. The response's test has a 100 ms timeout over a connect that blocks. At that size
RabbitMQ's own connect, 45 to 50 ms after the block, ended past the bound, and F410's refusal
answered the call before a late timer could show, so the scenario passed there before the fix. It
uses two fifths of the settle budget, 200 ms by default, with a block of half of it. It also writes
the server a ten-second `deadline-ms` through `Stamped`, because a server stamped with what was left
answers its own `timeout` at the bound and hides the client's timer, as the first version's
before-run showed on TCP. The 100 ms form is batch 35's probe, run before and after as p03.

### S3. The bound asserted is the later of the bound's end and the block's

Decision 2. "No more than the block's end plus one poll" cannot hold for a block shorter than the
timeout, where the timer must wait for the bound's end, and a block longer than the timeout is
answered by F410's refusal at the connect's return, whatever the timer does, so it cannot show
F411. The scenario blocks for half the timeout and asserts the later end, with a tenth of the settle
budget for the poll.

### S4. The gRPC pin measures the connect, and the response's slack

Decision 5. The connect hold is the connector's measured connect, 251.6 to 254.3 ms for a 250 ms
wait, not the 250 ms itself, and the 1 ms covers only what precedes the connect inside tonic and
the header's truncation. The observed use is 0.02 to 0.6 ms over ninety rounds. A slower machine
could exceed it; the alternative, counting from `outgoing`'s read to the connect's end, cannot fail
on a delay inside tonic before the connect, which is part of what the pin is for.

### S5. The connect scenario's evidence half blocks after the connection

Decision 6. F411's fix let the call's timer answer in the poll after a block that precedes an
awaiting connect, so batch 35's blocking half passed with `written`'s refusal removed and was
evidence for nothing. The half now makes the connection first and blocks after it, the one order in
which a frame is written in the poll that ends the block. The alternative was to drop the half and
leave `written`'s refusal to the unit tests, which have none for it.

### S6. `outgoing`'s doc states the limit

Decision 5. The response puts the limit's statement in the design. The function whose value the
limit is about now says it too, in one paragraph; the design is left to the fold.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch36/`: `probes/` for the
probes, `before/` for the runs against the code before the fix, `suites/` for the RPC suites, with
`summary.txt` and the container lists before and after each, `verify/` for the checks, the
workspace test, docs and clippy, `broken/` and `specs/` for the breaks and the printed probes, and
`scripts/`. A first full verification, on the tree before the connect scenario's evidence half
moved, passed every suite and every check but `ulo-rpc-conformance`'s docs, which refused an
intra-doc link to a private constant; it is kept in `verify/run1-superseded/` and
`suites/run1-superseded/`. Every run below ran on the tree hashed `de24020f..`, read before the
verification and after it (`verify/tree-hash-before-verify.txt`, `verify/final-tree-hash.txt`,
`verify/final-tree-hash-end.txt`).

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 51 passed and 3 ignored each (3.26 s each); the
  smol TCP link's three 51 and 3 each (3.24 s, 3.24 s, 3.25 s); UDP 48 and 6 (3.25 s). With
  `--features integration --test conformance` and the OpenSSL flags: NATS 53 and 1 (5.57 s); Redis
  52 and 3 (7.58 s); MQTT 58 and 1 (8.23 s); Kafka 54 and 1 (66.87 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 53 and 1 (28.38 s). Each count is batch 35's and one, the new
  scenario, declared not applicable on no link.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `builtins` 6, `clients` 8, one
  more than batch 35's, the pin, `compile_fail` 1, `deadlines` 20, `enhancers` 7, `grpc` 7,
  `replies` 12, `stream_end` 6, all passed; `ulo-grpc` (`verify/ulo-grpc-tests.txt`) 1 unit test and
  6 doctests ignored.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 22 unit; `bad_deadline` 7,
  `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 1 and 5 ignored, then the
  `compile_fail` one. **`ulo-rpc-conformance`'s** (`verify/rpc-conformance-tests.txt`): 2 unit,
  `harness` 5, and 2 doctests ignored.
- **The unsafe check,** `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines
  refused, 2 valid lines accepted", then 456 files scanned, 2 listed, 0 findings, exit 0
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
  exit 0, 1,262 passed, 0 failed, 142 ignored, over 198 lines starting `test result:`
  (`verify/workspace-test.txt`). Batch 35's run had 1,254 and 142 over 198. The 8 more: the new
  scenario in each of the seven suites the workspace test runs, TCP's three, UDP's and smol's
  three, the broker suites being behind `integration`, and the gRPC pin.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`, `ulo-grpc`
  and `ulo-rpc-conformance` (`--all-features`): exit 0 each (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-grpc -p ulo-codegen-tests -p ulo-rpc-conformance
  --all-targets --all-features --no-deps --locked`, with the OpenSSL flags: exit 0, 11 warning
  locations outside `crates/ulo/src` (`verify/clippy.txt`), batch 35's eleven at the same lines,
  `type_complexity` on `Conn::send` at `client.rs:219` and `result_large_err` on the conformance
  crate's `bound` at `cases/app.rs:643` among them. The batch introduced none.
- **Containers and images:** at the batch's start twelve containers, the user's among them, every
  one exited (`verify/docker-start.txt`), and 25 images (`verify/docker-images-start.txt`). Every
  before-run's, suite's and final break's container list after it matches its list before, the
  probes and earlier breaks that started a broker printing theirs into their own output, and
  the end's lists match the start's, the same twelve containers by id and the same 25 images
  (`verify/docker-end.txt`, `verify/docker-images-end.txt`). The batch's own containers were the
  suites' testcontainers, each removed by its scenario. The batch pulled no image, ran no `docker
  ... prune` and started, stopped or removed nothing of the user's. Docker answered throughout. No
  permission check refused an action.
- **Disk:** 181 GB free at the start, 177 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`).
