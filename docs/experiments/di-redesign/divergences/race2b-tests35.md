# Divergences: race 2b, tests batch 35, a call's deadline measured once its connection is made (F410), a connect that outlasts it refused, and an `open` frame's headers on CBOR (F412)

The sixtieth response, signed off 2026-10-17, answers `race2b-tests34.md`. F410 was probed on both
transports before anything was built. On RPC, a client whose link held each new connection 250 ms
gave the server a deadline 251 to 315 ms later than its caller's on all seven links probed, and
1,662 ms later with Kafka's 1.5 s hold, each call's frame in transit for 0.1 to 17 ms of that. The
client now stamps `deadline-ms` with what is left of the call's bound once the connection is in
hand, immediately before the frame is written, and refuses there a bound that ended during the
connect, `Timeout` with nothing sent; afterwards each server's deadline was later than its
caller's by less than the frame's transit. On gRPC, a tonic `Channel` whose connector held
each connect 250 ms gave the server a deadline 252 to 254 ms later than the caller's. A ready
`Channel` is a free slot in its buffer, not a connection, and tonic 0.14.6 offers no point after its
connect where a value could be written, so F410 stays open on gRPC and is put for sign-off (S1).
F410's scenario then failed on both CBOR suites: on a CBOR link an `open` frame was written with a
map length one short, so a streamed request reached the server with no headers and no deadline
(F412); the length is fixed. Building the connect scenario showed that a call's own timer starts at
its arm's first poll, so a connect that blocks its thread delays the caller's `Timeout` by the
block (F411), filed and not built. The doc line the response's S3 asked for is on `RpcClient` and
`Emit::within`.

Files changed: for F410, `crates/ulo-rpc/src/client.rs`; for F412, `crates/ulo-rpc/src/codec.rs`;
for the scenarios, `crates/ulo-rpc-conformance/src/lib.rs`, `src/cases/app.rs` and
`src/cases/deadlines.rs`; in the workspace's `FRAMEWORK_GAPS.md`, F411 and F412 new, F410 and F412
with fix notes, their commit written `<commit>`. The batch started from `c0e43f81`, and no commit
was made during it. Versions are unchanged at `0.1.0-alpha.1`, and `Cargo.lock` is unchanged.

## The signatures

```rust
// crates/ulo-rpc/src/client.rs, private
pub(crate) struct Spec { /* … */ ends: Option<Instant>, stamps: bool }
fn written(headers: &mut CallHeaders, ends: Option<Instant>, stamps: bool, now: Instant) -> Result<(), RpcError>;

// crates/ulo-rpc-conformance, crate-private
pub(crate) const DEADLINE: &str = "conformance.deadline";
pub(crate) const DEADLINE_STREAM: &str = "conformance.deadline_stream";
pub(crate) const DEADLINE_SUM: &str = "conformance.deadline_sum";
pub(crate) struct Seen { pub(crate) mark: Option<String>, pub(crate) deadline: Option<Instant>, pub(crate) header: Option<String> }
impl Probe { pub(crate) fn seen(&self) -> Vec<Seen>; }
// cases/deadlines.rs, private to the module
enum Hold { Waits(Duration), Blocks(Duration) }
struct Handoffs(Arc<Mutex<Vec<(String, Instant)>>>);
struct SlowConnect<L> { link: L, hold: Hold, handoffs: Handoffs }
impl<L: Link> Link for SlowConnect<L>;
```

No public signature changes. What changes in meaning without a signature:

- `deadline-ms` on a `req` or an `open` is what is left of the call's bound when the frame is
  written, the connection already in hand, where it was the bound as `limit` read it when the call
  was awaited, before the connect. On a connection already made the two differ by the microseconds
  between the await and the write, so a value can be a millisecond lower than before.
- A call of any shape whose bound ends while its client connects fails `Timeout`, "the call's
  deadline passed while its client connected, so nothing was sent", with no frame sent, an `emit`
  included, whose bound is its publish's. Before the batch such a call was sent whenever its
  connect returned past the deadline inside the poll that writes the frame, and could be answered
  `Ok`.
- On a CBOR link an `open` frame carries its headers, so `send_stream` and `duplex` reach the server
  with `deadline-ms` and every header the caller set. Before the batch they arrived with none.
- The RPC conformance suite has two scenarios more, so every link's count grows by two, and the
  server mounts three handlers more, `conformance.deadline` on every link and
  `conformance.deadline_stream` and `conformance.deadline_sum` where the streaming controller is
  mounted.

## Decisions

Line numbers are those of `c0e43f81` unless a line says otherwise.

### 1. The RPC probe: the stamp was measured before the connect, so fixed (F410)

- **What it read.** `Spec::limit` (`crates/ulo-rpc/src/client.rs:470`) wrote `deadline-ms` from its
  own `now` when the call was awaited, and `Spec::open` (`client.rs:519`) connected afterwards,
  through `ClientInner::register` and `connection` (`client.rs:558`, `:570`), before writing the
  frame. The server opens the execution with a deadline of its timer's now plus the value when the
  delivery reaches its dispatcher, so its deadline is later than the caller's by everything between
  `limit`'s read and the server's: the connect and the frame's transit.
- **How the connect is held.** A client link, `SlowConnect`, waits for the given hold on the
  client's runtime before calling its link's `connect`, and records when it is handed each frame
  carrying `x-conformance-mark`. The response offered a listener that delays its accept, or a proxy
  that holds the handshake. Neither delays a TCP connect: the kernel completes the handshake into
  the listen backlog before any `accept`, three connects to a listener that never accepts each
  completing in under 0.1 ms with five bytes written (`probes/tcp-connect-before-accept.txt`), so a
  late accept or a relay holding bytes lengthens the frame's transit on the TCP links and UDP rather
  than the connect. A relay holding a broker's handshake would delay the brokers' connects alone.
  The link holds `Link::connect` itself, the same on every link.
- **What is measured.** Each server handler records the deadline its execution was given and the
  `deadline-ms` it came from, so `deadline − value` is the instant the server read the header; the
  link's record gives the instant the frame was handed to the connection. The difference is the
  frame's transit, measured on the one clock the scenario's client and server share. The caller's
  deadline is that of an execution opened with `Client::open`, and the request and the streamed
  request are given a timeout twice what the execution has left, so the execution's deadline is
  each call's own. The first run on Kafka used the client module's 10 s timeout, shorter than the
  31.5 s the execution had, and compared the server's deadline with the wrong one
  (`before/superseded/before-rpc-6.txt`).
- **What it showed.** The scenario run against `client.rs` as `c0e43f81` has it
  (`before/before-rpc-*.txt`), three calls made at once as a client's first, a request, a stream and
  a streamed request, each carrying `deadline-ms: 5249` of 5,250 ms, Kafka's `31499` of 31,500: the
  server's deadline was 250.9 to 251.0 ms past the caller's on TCP, 251.4 on UDP, 255.7 to 255.8 on
  NATS, 254.1 to 255.1 on Redis, 258.3 to 258.6 on MQTT, 315.1 to 315.2 on RabbitMQ and 1,661.8 to
  1,662.0 on Kafka, each frame in transit 0.1 to 1.3 ms, 17 ms on Kafka. The rest above the hold is
  each link's own connect, about 65 ms on RabbitMQ.
- **The fix.** `limit` (`client.rs:491` on the batch's tree) keeps the instant the call's bound ends
  in `Spec::ends`, and whether the shape is stamped in `Spec::stamps`, and writes no header.
  `written` (`client.rs:583`) reads the runtime's now, refuses a bound already ended, and otherwise
  writes `deadline-ms` with what is left of it. `Spec::open` calls it once `register` has handed
  over the connection, before the opening frame is built (`client.rs:546`), for every shape answered
  by a reply; `Emit` calls it after `connection` (`client.rs:991`), and it stamps nothing there. A
  call `written` refuses is settled first, so its registration is dropped without a `cancel`.
- **What it showed after.** The same scenario on the batch's tree, its report printed
  (`broken/p01-after-fix-numbers-*.txt`): the server's deadline from 0.19 ms before the caller's to
  1.12 ms after it on the six links held 250 ms, and 7.4 to 15.0 ms after on Kafka, in every case
  less than the frame's transit, by 0.01 to 0.6 ms, the truncation to whole milliseconds.
- **The unit test the fix changed.** `a_stream_sends_its_deadline_and_neither_its_idle_timeout_nor_the_client_s_timeout`
  asserted `300`, `2000` and `1500` exactly; measured at the write the values are those or a
  millisecond under, and the test accepts either.
- **Docs.** `RpcClient`'s paragraph on bounds says what `deadline-ms` is measured against and that a
  bound passing during the connect fails the call with nothing sent; `StreamCall::deadline` says the
  server is given what is left at the write; `Spec::ends`, `limit` and `written` say where the
  stamp is made and why.

### 2. The gRPC probe, and whether a ready `Channel` is a connection: it is not, so F410 stays open there

- **What it showed.** `probes/grpc-slow-connect.txt`, from a temporary test kept as
  `probes/grpc-slow-connect-probe.diff`: a `ClockClient` over a tonic `Channel` built with
  `Endpoint::connect_with_connector_lazy`, its connector an `HttpConnector` behind a 250 ms wait,
  three rounds, each from an execution with 2 s left forwarded through `outgoing`. `grpc-timeout`
  was written `1999971u` and `1999993u`, the connect finished 251.8 to 252.5 ms after the request
  was stamped, and the server's deadline was 252.3 to 253.6 ms past the caller's; `Quick` reported
  2,000 ms left each time.
- **The premise, read in tonic 0.14.6 and tower 0.5.3.** `outgoing` sets `grpc-timeout` through
  `Request::set_timeout`, which writes the header into the request's metadata at once
  (`tonic-0.14.6/src/request.rs:293-297`), before the generated
  client is called. The generated client awaits `self.inner.ready()` and then calls `unary`
  (`codegen.probe.v1.rs`, `Quick`); `Grpc::ready` polls the service's readiness
  (`src/client/grpc.rs:199-204`), and the service is the `Channel`, a `Buffer`
  (`src/transport/channel/mod.rs:66-69`) whose `poll_ready` delegates to the buffer
  (`mod.rs:213-215`). tower's `Buffer::poll_ready` reserves a permit on the worker's channel and
  nothing else (`tower-0.5.3/src/buffer/service.rs:97-108`). The worker polls the inner service's
  readiness only once it holds a message, a request already built, and puts the message back while
  the inner service is pending (`src/buffer/worker.rs:153-185`). The inner service is
  `Connection`, built by `Channel::new` from `Connection::lazy` (`mod.rs:151-166`), whose stack is
  `AddOrigin`, `UserAgent`, tonic's `GrpcTimeout` and the optional limits over `Reconnect`
  (`src/transport/channel/service/connection.rs:66-85`); `Reconnect::poll_ready` makes the
  connection, the connector and hyper's HTTP/2 handshake (`connection.rs:197-216`), and answers
  ready only once connected (`service/reconnect.rs:62-138`). A ready `Channel` is therefore a free
  buffer slot, and the connect is driven by the request itself, after its header was written.
- **Where a stamp could sit.** Inside the stack, after `Reconnect` is ready, is the point:
  `GrpcTimeout::call` runs there and reads `grpc-timeout` to arm tonic's own client timer, and does
  not rewrite the header (`src/transport/service/grpc_timeout.rs:41-61`). `Connection::new` fixes
  that stack, and `Endpoint`'s public API (`src/transport/channel/endpoint.rs:65-669`) has no way to
  add a layer to it; the custom connector of `connect_with_connector_lazy` (`endpoint.rs:611`) is a
  `Service<Uri>` that sees no request. Making the connection ready before `outgoing` builds the
  request has no path either: readiness does not drive the connect, and `Endpoint::connect`, which
  connects before the buffer is built (`connection.rs:98`), covers the first connection alone, since
  `Reconnect` returns to `Idle` when a connection fails and reconnects at the next request
  (`reconnect.rs:95-108`, `:125-127`). No workaround was built; see S1.

### 3. A connect that outlasts the deadline: refused once the connect returns

- **What the scenario does.** For each shape a link carries, a fresh client whose link holds its
  connect for twice what its execution has left, a fifth of the broker's settle budget: 200 ms of
  100 ms, 1.2 s of 600 ms on Kafka; once with a hold that waits and once with one that blocks its
  thread; then
  a request outside, on the fixture's own client, answered, and the server's tapped link has
  delivered that request alone, no event handled.
- **The waiting hold was already refused.** With a connect that waits, the call's own timer in
  `bounded` ends the call at its deadline and the connect is dropped unfinished: before the fix every
  such call on every link failed `Timeout` with nothing sent (`before/before-rpc-*.txt`, the
  `Waits` outcomes). That half passes against the code before the fix.
- **The blocking hold was sent.** With a connect that blocks, the connect returns inside one poll of
  the call, which then writes the frame before `bounded`'s `select!` can choose its expired timer.
  Before the fix the server received two or three of the four blocked calls on TCP, NATS, Redis,
  MQTT, RabbitMQ and Kafka and one of two on UDP, a request or a streamed request among them
  answered `Ok` after its deadline on every link but UDP. After the fix `written` sees the bound
  ended and the call fails `Timeout` before its frame exists. See S3.
- **Why a blocking connect.** It is what makes the connect's end and the passed deadline fall in one
  poll every time; a connect whose own wait ended at the deadline's instant would reach the same
  state at random. A link whose `connect` does blocking work reaches it in use.

### 4. F412: an `open` frame's map length, found by F410's scenario

The first verification run failed `a_call_s_deadline_is_measured_once_its_connection_is_made` on
TCP CBOR and smol CBOR alone: the server's handler for the streamed request recorded no mark, no
`deadline-ms` and no deadline (`suites/run1/tcp-cbor.txt`, `suites/run1/smol-cbor.txt`).
`Wire::serialize` (`crates/ulo-rpc/src/codec.rs:161`) counts an `open`'s entries with an `evt`'s,
`2 + d` (`codec.rs:165`), where an `open` writes `id`, `p` and `h` beside `t`; ciborium writes the
declared length into the map's header, a reader stops after three entries, and `decode_frame`,
reading one item, ignores the rest, so `h` was dropped. serde_json ignores the declared length, so
JSON links were unaffected, and no earlier scenario read an `open`'s headers on CBOR. The probe,
`every_frame_kind_reads_back_as_written_in_both_codecs`, written as the test it is now
(`probes/f412-codec-round-trip.txt`): of one frame of every kind through both codecs, CBOR's
`open` alone read back different, its two headers gone. The arm now reads
`Frame::Req { .. } | Frame::Open { .. } => 3 + d` (`codec.rs:164` on the batch's tree). It is fixed
here because the batch's own scenario cannot pass on a CBOR link without it, and it is the class
F410 is: a callee given no deadline at all while its caller times out. See S4.

### 5. F411: filed, not built

`bounded` (`client.rs:790`) creates the call's timer inside its `expired` arm (`client.rs:802`), and
the unbiased `select!` (`client.rs:818`) can poll that arm after the work, so the wait starts once
the work's first poll returns. The blocked calls in the before-runs that were answered `Ok` after
their deadline were this. A probe on the batch's tree (`broken/p02-f411-late-timer-0.txt`): sixteen
requests with a 100 ms timeout, each on a fresh client whose link's connect blocks 60 ms, to a
server that never answers; six were answered `Timeout` after 101 to 102 ms and ten after 165 to
167 ms. With F410's fix the server stops at the deadline the client stamped, so what is left is a
caller told `Timeout` later than its bound, not a callee outliving it. The response did not ask for
the timer, and the fix touches every runtime's `sleep`, so it is filed for the designer.

### 6. How the scenarios are built

- **First calls share one connect.** The measurement scenario makes its three calls at once on a
  client that has made none, so all three are stamped, before the fix, before the one connect they
  wait on, and each, after it, once the connect has returned. On a link without streamed shapes
  only the request is made.
- **Server-side first.** The connect scenario asserts what the server's link delivered and the
  events its handler counted before it asserts what each caller was told, and its failure carries
  every call's outcome.
- **An execution's handle holds its app's close.** A handle to an execution opened with
  `Client::open` and still alive held the client's app at its close for its 10 s bound, eight times
  over in the connect scenario's first run (`probes/rpc-tcp-measured.txt`, 81.76 s). Each scenario
  drops the handle before closing the client, and a comment says why.
- **Every frame kind through both codecs.** The F412 test is `ulo-rpc`'s first codec unit test, one
  frame of every kind each carrying every field its kind has, so a kind whose length miscounts any
  field fails it, not the `open` alone.

## The tests

- **F410 on RPC, two conformance scenarios in `crates/ulo-rpc-conformance/src/cases/deadlines.rs`:**
  `a_call_s_deadline_is_measured_once_its_connection_is_made`, decision 1, the probe kept: each
  server's deadline no later than its caller's past the frame's measured transit and one
  millisecond, the unit `deadline-ms` is written in; and
  `a_call_whose_connect_outlasts_its_deadline_is_refused_before_the_server_sees_it`, decision 3.
- **F412, one unit test in `crates/ulo-rpc/src/codec.rs`:**
  `every_frame_kind_reads_back_as_written_in_both_codecs`.
- **Changed:** `a_stream_sends_its_deadline_and_neither_its_idle_timeout_nor_the_client_s_timeout`
  in `crates/ulo-rpc/src/client.rs`, decision 1.
- **F410 on gRPC:** no test; the probe's source is `probes/grpc-slow-connect-probe.diff`, not in the
  tree, since it would assert the defect.

### Before and after

Each fix was tested before it was made: the scenarios against `crates/ulo-rpc/src/client.rs` as
`c0e43f81` has it, put back whole through `batch35/scripts/brk.py`, batch 34's script with its
paths moved (`batch35/specs-before/`, output in `batch35/before/`); F412's test was its probe, run on
the code before its fix. The breaks went through the same script, which replaces each span,
asserting it occurs once, runs the targets under bash, writes every touched file and `Cargo.lock`
back byte for byte, and compares a SHA-256 over `git diff HEAD` of `crates`, `Cargo.toml`,
`Cargo.lock`, `.github`, `scripts` and `unsafe-allowed.txt` and every untracked file under `crates`
and `scripts`. Specs in `batch35/specs/`, output in `batch35/broken/` with `summary.txt` and the
container list before and after each. Every restore reported `ok`, on the tree hashed `45fdc57c..`
(`verify/tree-hash-before-breaks.txt`). A first round of the same runs, on the tree before F412's
fix, is kept in `before/superseded/tree-720e6eb4/` and `broken/superseded/tree-720e6eb4/`.

| Run | Target | Result |
| --- | --- | --- |
| before F410: the RPC client as it stood, TCP | the two scenarios | both failed: the server's deadline 250.9 to 251.0 ms past the caller's; three of the four blocked calls delivered, a stream, a streamed request and an event (`before/before-rpc-0.txt`) |
| the same, UDP | the two scenarios | both failed: 251.4 ms past; the blocked event delivered (`before/before-rpc-1.txt`) |
| the same, NATS | the two scenarios | both failed: 255.7 to 255.8 ms past; two blocked calls delivered, the request and the streamed request answered `Ok` (`before/before-rpc-2.txt`) |
| the same, Redis | the two scenarios | both failed: 254.1 to 255.1 ms past; two delivered, both answered `Ok` (`before/before-rpc-3.txt`) |
| the same, MQTT | the two scenarios | both failed: 258.3 to 258.6 ms past; two delivered, both answered `Ok` (`before/before-rpc-4.txt`) |
| the same, RabbitMQ | the two scenarios | both failed: 315.1 to 315.2 ms past; two delivered, both answered `Ok` (`before/before-rpc-5.txt`) |
| the same, Kafka | the two scenarios | both failed: 1,661.8 to 1,662.0 ms past with a 1.5 s hold; two delivered, both answered `Ok` (`before/before-rpc-6.txt`) |
| before F412: the codec as it stood | `codec::tests` | failed: CBOR's `open` read back with no headers, every other kind in both codecs as written (`probes/f412-codec-round-trip.txt`) |
| p01. the measurement scenario, its report printed | seven links | passed on each; the numbers in decision 1 |
| p02. F411's probe | `ulo-rpc` unit | six of sixteen `Timeout`s at 101 to 102 ms, ten at 165 to 167 ms |
| b01. the stamp written by `limit` again, `written` stamping nothing | TCP and NATS, the two scenarios | the measurement scenario failed on both, 251.5 to 251.7 and 256.5 to 256.6 ms past; the connect scenario passed |
| b02. `written` without its refusal | TCP and NATS, the two scenarios | the connect scenario failed on both, three blocked calls delivered on TCP and one on NATS; the measurement scenario passed |
| b03. `open` counted as `2 + d` again | `codec::tests`, TCP CBOR's measurement scenario | both failed, the CBOR `open` read back with no headers, the server given no deadline |
| the restored tree | every target above | every test passed |

## Left for the transports DESIGN fold

Line numbers are those of `c0e43f81`.

- §5.2's grammar paragraph (line 946): "The client stamps `deadline-ms` on every call answered by one
  reply … as the shorter of the call's timeout … and the calling execution's remaining time" gains
  when: what is left of that bound when the frame is handed to a connection already made, so a
  connect gives the server none of its length, and a bound that ends during the connect fails the
  call `Timeout` with nothing sent, an `emit` included (F410).
- §5.2's codec text: an `open` carries `h` in both codecs, and a unit test writes every frame kind
  through both and reads it back (F412).
- §5.2's conformance paragraph (line 962): the handlers gain `deadline`, `deadline_stream` and
  `deadline_sum`, which record the deadline the server computed and the `deadline-ms` it came from;
  the scenario count grows by two.
- §5.4's client paragraph (line 1096): the post-connect stamp and refusal; and a `Timeout` once a
  frame has been handed to the link leaves the outcome unknown on every shape, an `emit`'s event
  possibly published on a broker that confirms publishes (the sixtieth response's S3).
- §6.3's client paragraph (line 1190): "`outgoing` writes the header when it is called … (F410,
  open, §5.4)" stays open, now with the premise found: a ready `Channel` is a free buffer slot,
  tonic's connect is driven by the request after `outgoing` has written its header, and tonic
  offers no layer below its buffer (decision 2, S1).
- Decision 106 (line 1620): "the client knows a call's timeout when it builds the frame" becomes
  that the frame carries what is left of the call's bound when it is written.
- Decision 127 (line 1641): its last sentence, "stamping once the connection is in hand … is open
  and unbuilt", becomes built on RPC and open on gRPC.
- A decision after 128: a callee never outlives its caller, the sixtieth response's ruling, with
  the probe's numbers on both transports, the rule that the remaining time is measured as the frame
  is handed to an established connection, why the connect scenario blocks its connect, and F411
  beside it.

## Needs sign-off

Numbered from S1 for this batch.

### S1. F410 on gRPC: tonic offers no point after its connect

Decision 2. The defect is confirmed, 252 to 254 ms past the caller's deadline with a 250 ms connect.
`outgoing` writes `grpc-timeout` before the request reaches the `Channel`; a ready `Channel` is a
buffer slot (`tower-0.5.3/src/buffer/service.rs:97-108`); the connect is driven by the buffer's
worker once it holds the built request (`src/buffer/worker.rs:153-185`,
`tonic-0.14.6/src/transport/channel/service/reconnect.rs:62-138`); and the one point after it,
inside `Connection`'s fixed stack where tonic's own `GrpcTimeout` reads the header
(`connection.rs:66-85`, `grpc_timeout.rs:41-61`), takes no layer through `Endpoint`. An eager
`Endpoint::connect` covers the first connection only, `Reconnect` reconnecting lazily after a lost
one. Nothing was built, and `outgoing`'s doc is unchanged. What a caller sees stays as F410 has it:
a call that opens a new connection gives the callee up to the connect's length past the caller's
deadline.

### S2. The kept test allows the frame's transit

Decision 1. The response asks that the server's deadline be no later than the caller's, within one
encoding unit. The server reads `deadline-ms` after the frame has crossed the link, and no value
written before it is sent can account for the crossing, 0.1 to 1.7 ms on the links held 250 ms and
7 to 17 ms on Kafka. The scenario asserts the server's deadline no later than the caller's plus the
frame's transit, measured from the link's handoff to the server's reading of the header on the
clock both share, plus one millisecond. Asserting the response's bound alone would fail on Kafka
and at times on the brokers whatever the client wrote.

### S3. The connect scenario fails before the fix only when the connect blocks

Decision 3. The response asks for a test that a connect longer than the deadline fails locally with
`Timeout` and the server receives nothing, failing against the code before the fix. A connect that
waits was already refused: the call's timer ended it at the deadline. The scenario runs that half
too, and it passes before and after. The half that fails before the fix blocks its thread in the
connect, the one way the connect's end and the passed deadline fall in one poll every time.

### S4. F412 is fixed in this batch

Decision 4. The response asked for F410 alone; F412 surfaced as the batch's own scenario failing on
both CBOR suites, and the scenario cannot pass there without it. The fix is the one arm's count. The
alternative was declaring the measurement scenario not applicable on CBOR, which would have hidden
a link sending streamed requests with no deadline.

### S5. The refusal after the connect covers an emit

Decision 1. The response names the call failing when the connect uses up its deadline. An `emit`
is bounded the same way, its publish by the client's timeout and the execution's remaining time
(batch 34's S3), and before the fix a blocked emit past its bound was published on TCP and UDP
and answered `Ok` after its bound. `written` refuses it as it
refuses a request, and stamps nothing on it.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `batch35/`: `probes/` for the
probes, `before/` for each run against the code before its fix, `suites/` for the RPC suites, with
`summary.txt` and the container lists before and after each, `verify/` for the checks, the
workspace test, docs and clippy, `broken/` and `specs/` for the breaks and the printed probes,
`specs-before/` for the before-runs, `scripts/`. A first verification run, on the tree before
F412's fix, found it and was stopped once the broker suites had finished
(`suites/run1/`, `verify/run1-stopped/`). Every run below ran on the tree hashed `45fdc57c..`, read
before the breaks, before the verification and after it (`verify/tree-hash-before-breaks.txt`,
`verify/final-tree-hash.txt`, `verify/final-tree-hash-end.txt`).

- **The RPC suites, once each, all twelve,** one target at a time, `--locked`, libtest's time
  (`suites/summary.txt`): TCP, TCP CBOR and TCP TLS 50 passed and 3 ignored each (2.58 s, 2.57 s,
  2.59 s); the smol TCP link's three 50 and 3 each (2.54 s, 2.57 s, 2.58 s); UDP 47 and 6 (1.54 s).
  With `--features integration --test conformance` and the OpenSSL flags: NATS 52 and 1 (5.61 s);
  Redis 51 and 3 (7.45 s); MQTT 57 and 1 (7.93 s); Kafka 53 and 1 (62.22 s); RabbitMQ with
  `ULO_CONFORMANCE_PARALLEL=6` 52 and 1 (26.68 s). Each count is batch 34's and two, the two new
  scenarios, none declared not applicable on any link.
- **gRPC's tests**: `ulo-codegen-tests` (`verify/codegen-tests.txt`), `builtins` 6, `clients` 7,
  `compile_fail` 1, `deadlines` 20, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6, all
  passed; `ulo-grpc` (`verify/ulo-grpc-tests.txt`) 1 unit test and 6 doctests ignored.
- **`ulo-rpc`'s own tests** (`verify/ulo-rpc-tests.txt`): 22 unit, one more than batch 34's, the
  codec test; `bad_deadline` 7, `drain_window` 3, `runtime` 11, `stream_end` 5, the doctests 1 and 5
  ignored, then the `compile_fail` one. **`ulo-rpc-conformance`'s**
  (`verify/rpc-conformance-tests.txt`): 2 unit, `harness` 5, and 2 doctests ignored.
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
  exit 0, 1,254 passed, 0 failed, 142 ignored, over 198 lines starting `test result:`
  (`verify/workspace-test.txt`). Batch 34's run had 1,239 and 142 over 198. The 15 more: the two
  scenarios in each of the seven suites the workspace test runs, TCP's three, UDP's and smol's
  three, the broker suites being behind `integration`, and the codec test.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc`, `ulo-grpc`
  and `ulo-rpc-conformance` (`--all-features`): exit 0 each (`verify/doc-*.txt`).
- `cargo +1.98.1 clippy -p ulo-rpc -p ulo-grpc -p ulo-codegen-tests -p ulo-rpc-conformance
  --all-targets --all-features --no-deps --locked`, with the OpenSSL flags: exit 0, 11 warning
  locations outside `crates/ulo/src` (`verify/clippy.txt`), batch 34's eleven, two of them moved by
  the lines the batch added above them: `type_complexity` on `Conn::send`, `client.rs:219` where it
  was `:212`, and `result_large_err` on the conformance crate's `bound`, `cases/app.rs:643` where it
  was `:587`. The batch introduced none.
- **Containers and images:** at the batch's start twelve containers, the user's among them, every
  one exited (`verify/docker-start.txt`), and 25 images (`verify/docker-images-start.txt`). Every
  probe's, before-run's, break's and suite's container list after it matches its list before, and
  the end's lists match the start's, the same twelve containers by id and the same 25 images
  (`verify/docker-end.txt`, `verify/docker-images-end.txt`). The batch's own containers were the
  suites' testcontainers, each removed by its scenario. The verification's first run was stopped
  after the RabbitMQ suite had finished and its container list matched, while it ran `ulo-rpc`'s
  tests, which start none. The batch pulled no image, ran no `docker ... prune` and started,
  stopped or removed nothing of the user's. Docker answered throughout. No permission check refused
  an action.
- **Disk:** 187 GB free at the start, 182 GB at the end (`verify/disk-start.txt`,
  `verify/disk-end.txt`).
