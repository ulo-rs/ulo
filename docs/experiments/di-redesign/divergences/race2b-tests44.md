# Divergences: race 2b, tests batch 44, a link's loops stop when the link drops (F428)

F428 is fixed in the six non-NATS RPC links, as the sixty-sixth transports response set out: each
loop a link spawns is held through a handle that aborts on drop, held by the link and not by the
state the loop holds, so the socket or connection goes when the link goes. The premise the response
asked to check holds for every broker link: the server sides of MQTT, Redis, RabbitMQ and Kafka each
keep a detached read loop holding their connection after a drop without `close`, and so do their
client sides. Probed before the fix (p01 to p07), every server side leaked; the client sides of the
four brokers leaked, and the TCP links' client connection ended with its outbound. `close` is
unchanged on every link: it takes the same handles and runs the same steps. A drop after it finds
the broker links' state emptied by it, and aborts TCP's forwarding task and UDP's writer, which
`close` leaves running (decisions 2 and 3). Eleven tests hold it, each failing against a break that restores
the detached loop. Two findings are new. The UDP server's leak ends at the next datagram once its
inbound stream has dropped (S2). A leaked MQTT server stayed in its shared-subscription group and
answered the requests the broker routed to it `unavailable`, a leaked RabbitMQ server kept its
consumers registered, and a leaked Kafka server stayed a member of its group (S5).

Files changed: `crates/ulo-rpc-tcp-core/src/link.rs`, `crates/ulo-rpc-tcp/src/link.rs`,
`crates/ulo-rpc-tcp-smol/src/link.rs`, `crates/ulo-rpc-udp/src/link.rs`,
`crates/ulo-rpc-mqtt/src/link.rs`, `crates/ulo-rpc-redis/src/link.rs`,
`crates/ulo-rpc-rabbitmq/src/link.rs` and `crates/ulo-rpc-kafka/src/link.rs`; the tests in
`crates/ulo-rpc-tcp/tests/dropped_link.rs`, `crates/ulo-rpc-tcp-smol/tests/dropped_link.rs`,
`crates/ulo-rpc-udp/tests/dropped_link.rs` (new) and the `tests/conformance.rs` of the four broker
links. `Cargo.lock` is unchanged. The batch started from `d2e6b26a`, made no commit and did not
edit the workspace's `FRAMEWORK_GAPS.md` or either DESIGN; the F428 fix note is at the end of this
log for the coordinator to paste. Versions are unchanged at `0.1.0-alpha.1`.

## The signatures

No public signature changes.

```rust
// crates/ulo-rpc-tcp-core/src/link.rs, private
struct Owned(TaskHandle);                       // new; Drop aborts the task
pub struct Core { /* + */ accept: Mutex<Option<Owned>>, forward: Mutex<Option<Owned>> }
struct State { /* - */ accept: Mutex<Option<TaskHandle>> }

// crates/ulo-rpc-udp/src/link.rs, private
struct Owned(tokio::task::JoinHandle<()>);      // new; Drop aborts the task
pub struct Udp { /* + */ receiving: Mutex<Option<Owned>>, writing: Mutex<Option<Owned>> }
pub(crate) struct State { /* - */ receiving: Mutex<Option<JoinHandle<()>>> }

// crates/ulo-rpc-{mqtt,redis}/src/link.rs, crate-private
impl Drop for State { /* aborts the server's and the client's task */ }

// crates/ulo-rpc-rabbitmq/src/link.rs, crate-private
pub(crate) struct State {
    server: Option<(Arc<ServerSide>, Vec<AbortHandle>)>,      // was Option<Arc<ServerSide>>
    client: Option<(Connection, Tokio, [AbortHandle; 2])>,    // was Option<(Connection, Tokio)>
}
impl Drop for State { /* aborts both sides' tasks */ }

// crates/ulo-rpc-kafka/src/link.rs, crate-private
pub(crate) struct State {
    server: Option<(Arc<ServerSide>, [AbortHandle; 2])>,      // was Option<Arc<ServerSide>>
    client: Option<Arc<ClientSide>>,
}
impl Drop for State { /* aborts the server's two lanes and the client's router, drops its producer detached */ }
```

What changes in meaning without a signature: a link dropped without `close` ends every loop it
spawned. A TCP server's listener closes, its accepted connections close and its address refuses
connections and can be bound again; a UDP server's socket closes; a broker link's connections close,
its subscriptions, consumers and group memberships going with them at the broker. Each crate's link
documentation says so.

## Decisions

Line numbers are those of the batch's tree.

### 1. What the brief asked to verify, by probe

The probes ran in the mirror (decision 6) on `d2e6b26a` unchanged, through `batch44/scripts/brk.py`,
batch 41's runner pointed at this batch's mirror and crates, which copies each probe file into the
mirror, runs it, removes it and compares the tree hash and every touched file with `cmp`; every
restore reported `ok`. Each probe binds or connects a link, drops it and what it answered without
`close`, and reads the address or the broker at intervals after the drop.

- **TCP, tokio (p01, `probes/p01-tcp-0.txt`).** Server side: at 1.3 ms, 117 ms, 420 ms and 1.53 s
  after the link's drop, a new connection was accepted, a rebind answered `AddrInUse`, and a
  connection opened before the drop was still open. Client side: with the link dropped and its
  outbound kept the peer's connection stayed open; 200 ms after the outbound's drop the peer read
  the end of the stream.
- **TCP, smol (p02, `probes/p02-smol-0.txt`).** The same on `OnSmol`'s runtime: accepted, `AddrInUse`
  and the early connection open from 10 µs to 1.52 s after the drop; the client connection ended
  with its outbound.
- **UDP (p03, `probes/p03-udp-0.txt`).** A rebind answered `AddrInUse` while bound and at 1.3 ms,
  15 ms, 217 ms and 1.22 s after the drop, with no datagram sent. One request datagram sent then
  was not answered, and the rebind after it succeeded, as did one 200 ms later. The receive loop
  reads the datagram, finds the inbound stream dropped, and returns, which releases its `State` and
  with it the socket (S2). A first run, which sent a datagram before each rebind and so released the
  socket at its first sample, is in `superseded/p03-first/`.
- **Redis (p04, `probes/p04-redis-0.txt`).** Server and client each through a relay of their own,
  Redis read from a direct connection: before the drop each relay carried 2 connections, Redis
  listed 4 other clients and the channels `p04.x`, `ulo:rpc:control` and the client's reply channel
  had subscribers. Unchanged at 1.8 ms, 208 ms, 1.2 s and 4.2 s after the server link's drop, 1 s
  after the client link's drop with its outbound kept, and 200 ms and 3 s after the outbound's drop.
- **RabbitMQ (p05, `probes/p05-rabbitmq-0.txt`).** `rabbitmqctl` inside the container: 2 running
  connections, and consumers on `p05.x` and the server's control queue, unchanged at 1.5 ms, 3.2 s
  and 7.4 s after the server link's drop and 1 s after the client link's and its outbound's; each
  relay kept its 1 connection throughout.
- **MQTT (p06, `probes/p06-mqtt-0.txt`).** Each relay kept its 1 connection at every sample. A
  request on the pattern published by a fresh client was taken by the server before the drop, and
  answered `unavailable`, "the server is shutting down", at every sample after it: the dropped
  server's event loop still received what the broker routed through its shared subscription and
  refused it, its inbound stream gone (S5).
- **Kafka (p07, `probes/p07-kafka-0.txt`).** A third listener and relay read the groups: the
  server's group `p07-server` and the client's reply group `p07-replies` each `Stable` with 1
  member, and the server's relay carrying 5 connections and the client's 3, unchanged at 1.2 ms,
  2.2 s and 7.3 s after the server link's drop, 3 s after the client link's drop and 5 s after its
  outbound's.

Read, to locate each leak:

- TCP: `Core::listen` kept the accept loop's `TaskHandle` in `State`, which the accept loop holds
  through `Server::state`, and spawned the forwarding task with its handle dropped. A `TaskHandle`
  detaches on drop (decision 47), so the link's drop reached neither task, and the accept loop holds
  the listener. Its connection tasks are in the loop's `TaskSet`; their writers end once `State` and
  every reply path drop, `State` holding each connection's write queue.
- UDP: the receive task's `JoinHandle` was in `State`, which the task holds; the server socket's
  writer was spawned with its handle dropped.
- MQTT: `State` keeps each side's event loop `JoinHandle`, which owns the connection; `State` is the
  link's alone, and a dropped `JoinHandle` detaches. The client's loop ends only at `close` or a
  failed connection, since it does not watch its reply lane.
- Redis: `State` keeps each side's Pub/Sub reader as an `AbortHandle`, which does not abort when
  dropped. The server's reader holds its side, which holds the Pub/Sub sink and the writer's queue;
  the client's holds the reply subscription.
- RabbitMQ: the request and control lanes and the client's writer and reply router were spawned with
  their handles dropped. Each holds a side whose `Channel` keeps lapin's connection open
  (lapin 4.12's `ConnectionCloser` closes it once the last handle drops).
- Kafka: the request and control lanes were spawned with their handles dropped and hold the server's
  side, and they end only on a phase change, the phase's sender being in that side. The client's
  router is in `ClientSide::routing`, a field of the side it holds, and its stop signal is in that
  side too.

### 2. TCP: the loops' handles move to the link, and abort on drop

`Core` holds the accept loop and the forwarding task in `accept` and `forward`
(`crates/ulo-rpc-tcp-core/src/link.rs:94`, `:97`), each an `Owned` (`:102`) wrapping the
`TaskHandle`, whose `Drop` (`:104`) calls `abort`. `State` no longer holds the accept loop's handle,
which breaks the cycle: the loop holds `State`, and the link alone holds the loop. `listen` stores
both (`:249`, `:250`) and still refuses a second `listen` while `accept` is set. `close` takes
`accept` and awaits the handle through `&mut` (`:320`), so the `Owned` drops after the task has
ended and its `abort` does nothing; the steps of `close` are unchanged. `close` leaves `forward` in
place, as it left the task running before: the forwarding task answers or hands over what is still
queued once the accept loop ends, and it ends when every sender of its queue has. A drop after
`close` aborts it, ended or ending.

Aborting the accept loop drops its future, which drops the listener and the loop's `TaskSet`, whose
drop aborts the connection tasks. Once those and the forwarding task are gone, `State` drops with its
map of write queues, and each connection's writer ends and shuts its half; the reader half dropped
with its connection task. `Owned` sits in the core, so the smol link gets the same fix with no
change of its own beyond its documentation. On smol, `TaskHandle::abort` cancels through
`Task::cancel`, polled once at the call (decision 47), which is safe from a drop on any thread.

### 3. UDP: the same, for the receive task and the writer

`Udp` holds the receive task and the server socket's writer in `receiving` and `writing`
(`crates/ulo-rpc-udp/src/link.rs:70`, `:72`), each an `Owned` (`:77`) over tokio's `JoinHandle`,
aborting on drop (`:79`); `State` no longer holds the receive task's handle. `close` takes
`receiving`, aborts it and awaits it as before (`:261`), then runs its final read through the writer,
which it leaves in place; a later drop aborts the writer, its refusals already sent, since `close`
awaits each one. `State` keeps the socket for `close`'s final read and drops it once the link, the
aborted tasks and every reply path have let go of it.

### 4. The broker links: the link's state aborts what it spawned

Each broker link's `State` is held by the link alone, in a `Mutex`, so a `Drop` on it runs when the
link drops and never at `close`, which takes each side out of it first:

- **MQTT** (`crates/ulo-rpc-mqtt/src/link.rs:100`): aborts the server's and the client's event loop.
  Decision 82's `AbortOnDrop` guards a loop until the broker answers; this covers it after.
  `connect` replaces the client's entry on a reconnect, which happens once the old loop has
  returned, read, and the replaced `JoinHandle` drops without an abort as before.
- **Redis** (`crates/ulo-rpc-redis/src/link.rs:67`): aborts the two Pub/Sub readers through the
  `AbortHandle`s `close` aborts. The side then drops, with its sink and its writer's queue, and the
  writer ends, dropping the publisher connection.
- **RabbitMQ** (`crates/ulo-rpc-rabbitmq/src/link.rs:99`): `State` keeps the `AbortHandle` of each
  request lane and the control lane beside the server's side (`:222`), and of the client's writer and
  reply router beside its connection (`:276`), and aborts them. `close` closes the connections and
  drops the handles unaborted, as before. The client's writer is held for a cycle found by the
  client test (S4).
- **Kafka** (`crates/ulo-rpc-kafka/src/link.rs:121`): `State` keeps the request and control lanes'
  `AbortHandle`s beside the server's side (`:313`) and aborts them; for the client it takes the
  router's `JoinHandle` out of `ClientSide::routing` and aborts it, and takes the producer out and
  drops it through `drop_detached`, as `ClientSide::close` does. Each consumer is `Detached`, so the
  abort drops it on a thread of its own and its leaving the group holds no runtime thread.

The drop is an abort, not a `close`: MQTT writes no DISCONNECT, Redis no UNSUBSCRIBE, and the Kafka
server does not run its synchronous commit. lapin closes a RabbitMQ connection itself once its last
channel handle drops, read. Each broker ends the session, subscription, consumer or membership when
the connection closes. Each crate's link documentation says a link dropped without `close` closes
its connections.

### 5. The tests

Each requires first what the drop must undo, so a check that cannot fail is caught on its own line:
the TCP and UDP tests that the bound address cannot be bound again while the link listens, and the
TCP test that it accepts a connection; the broker tests that the relay carries a connection.

- `crates/ulo-rpc-tcp/tests/dropped_link.rs` and `crates/ulo-rpc-tcp-smol/tests/dropped_link.rs`,
  `a_server_link_dropped_without_close_releases_its_address` (`:46`, `:47`): binds `127.0.0.1:0`
  through `prepare` and `listen`, opens a connection, drops the inbound stream and the link, and
  requires within 5 s a connection to the address refused with `ConnectionRefused`, a
  `TcpListener::bind` of it to succeed, and the early connection to read its end. Batch 42's p06
  established that `ConnectionRefused` separates a closed listener from an open or full one here.
- `crates/ulo-rpc-udp/tests/dropped_link.rs`, `a_server_link_dropped_without_close_releases_its_address`
  (`:29`): binds, drops, and requires a `UdpSocket::bind` of the address to succeed within 5 s. It
  sends no datagram, which would release the socket without the fix (S2).
- In each broker's `tests/conformance.rs`, behind `integration`, a server test and a client test,
  each against a container of its own from the suite's `Broker::start`, which CI's broker job runs
  with `--test conformance`. Each link reaches the broker through a relay (`relay::Relay`), and the
  test requires, after dropping the link and what it answered, that the relay carries no connection
  within `RELEASE`: a relayed connection ends once both directions have, the broker closing its end
  on reading the link's. The server's relay is a second one started by the test on Redis, MQTT and
  RabbitMQ, and the suite's server relay on Kafka; the client's is the suite's client relay.
  `RELEASE` is 10 s, 30 s on Kafka, whose consumers leave their groups from a thread of their own.
  - Redis (`:152`, `:167`), MQTT (`:382`, `:397`), RabbitMQ (`:100`, `:115`), Kafka (`:261`, `:284`):
    `a_server_link_dropped_without_close_closes_its_connection[s]` and
    `a_client_link_dropped_without_close_closes_its_connection[s]`, plural where the side holds more
    than one connection.
- Each conformance file's module documentation names the two tests.

### 6. Building in a mirror

Batches 43 and 45 edited `crates/ulo-rpc-nats` and `crates/ulo` in the real tree throughout. Every
probe, break, suite run and check ran in `batch44/mirror`, an export of `d2e6b26a` through
`git archive`, with the batch's crates copied from the real tree before each run by
`batch44/scripts/sync.sh`, which copies by checksum, never carries an mtime over, and shows each crate
identical with `diff -r`. The runner's tree hash covers the mirror's copies of the batch's crates,
`Cargo.toml` and `Cargo.lock`. Each measured run started from `cargo clean -p` of the crates it
tests, the TCP breaks cleaning `ulo-rpc-tcp-core` too. Batch 43 started NATS containers during the
batch, so the runner compares container lists with the `nats:*` lines set aside; the full lists are
kept beside each run. The workspace test ran in the real tree at the end (S7).

## The tests

- **F428, TCP and UDP:** `ulo-rpc-tcp`'s, `ulo-rpc-tcp-smol`'s and `ulo-rpc-udp`'s
  `tests/dropped_link.rs`, `a_server_link_dropped_without_close_releases_its_address`. Decisions 2,
  3 and 5.
- **F428, the brokers:** in `ulo-rpc-redis`'s, `ulo-rpc-mqtt`'s, `ulo-rpc-rabbitmq`'s and
  `ulo-rpc-kafka`'s `tests/conformance.rs`, behind `integration`,
  `a_server_link_dropped_without_close_closes_its_connection[s]` and
  `a_client_link_dropped_without_close_closes_its_connection[s]`. Decisions 4 and 5.

### Before and after

Specs in `batch44/specs/`, output in `batch44/probes/` and `batch44/broken/`, each with
`summary.txt` recording every `cmp` and `runner.txt` the container comparison. The breaks ran on the
fixed tree, each restoring the detached loop or removing one guard. Every restore reported `ok`, and
every container comparison `unchanged` with the `nats:*` lines aside.

| Run | Target | Result |
| --- | --- | --- |
| p01, p02. the TCP links, unchanged | `ulo-rpc-tcp`, `ulo-rpc-tcp-smol` | the server's address accepting and `AddrInUse` 1.5 s after the drop, an early connection open; the client's connection ended with its outbound |
| p03. the UDP link, unchanged | `ulo-rpc-udp` | `AddrInUse` 1.2 s after the drop; released by the next datagram |
| p04 to p07. the broker links, unchanged | each broker crate | every relay's connections kept at every sample, the last 2 s to 8 s after each side's drop; Redis's channels, RabbitMQ's consumers, the MQTT server's subscription and the Kafka groups' members kept |
| the fixed tree | the eleven tests | all passed (`runs/`) |
| b01. `Owned`'s drop a no-op in the core | both TCP tests | each failed, "the address still accepts connections 5s after the link's drop" |
| b02. the forwarding task's `Owned` forgotten | both TCP tests | passed (S3) |
| b03. `Owned`'s drop a no-op in UDP | the UDP test | failed, "the address cannot be bound again 5s after the link's drop: Address already in use" |
| b04. the UDP writer's `Owned` forgotten | the UDP test | passed (S3) |
| b05, b06. MQTT's drop without the server's, then the client's, abort | the MQTT tests | the server test failed, then the client test, "1 connections still open at the broker 10s after the link's drop"; the other passed each time |
| b07, b08. the same on Redis | the Redis tests | the server test failed with 2 connections open, then the client test with 2; the other passed |
| b09, b10. the same on RabbitMQ | the RabbitMQ tests | the server test failed with 1 open, then the client test with 1; the other passed |
| b11. RabbitMQ's client writer left detached, the router aborted | the RabbitMQ tests | the client test failed with 1 open (S4) |
| b12, b13. the same on Kafka, the client's router and producer both left | the Kafka tests | the server test failed with 5 open after 30 s, then the client test with 3 |
| b14. Kafka's client router aborted, the producer kept | the Kafka tests | both passed (S3) |
| b15. Kafka's client producer taken, the router left | the Kafka tests | the client test failed with 2 open |
| the restored tree | every target in the verification | every test passed, the Kafka suite on its second run |

## Left for the DESIGN folds

Line numbers are those of `d2e6b26a`.

- The transports DESIGN, decision 47 (line 1573): "the TCP and UDP RPC links keep the `TaskHandle` or
  `JoinHandle` of their accept and receive loops in state those loops hold too, and only `close`
  takes it, so a link dropped without `close` … keeps its listener or socket bound for the rest of
  the process (F428, open)" is false now. Each link holds its loops through handles that abort on
  drop, outside the state the loops hold: TCP's and UDP's an `Owned` in `Core` and `Udp`, the broker
  links' a `Drop` on their `State`. The rule stated beside it stands: a task its spawner detached
  outlives the spawner's drop with whatever it holds.
- The transports DESIGN, §5.3 (line 1081): "A link dropped without `close` … ends only what its drop
  reaches: the TCP and UDP links' accept and receive loops are tasks they spawned and detached …
  (F428, open, 47)". A link dropped without `close` ends every loop it spawned, with an abort rather
  than the orderly steps of `close`: no DISCONNECT, UNSUBSCRIBE or synchronous commit. The broker links' client sides are covered too (S1).
- The transports DESIGN, decision 149 (line 1675): its F428 clause, "A server whose socket lives in a
  task it spawned and detached keeps the socket after its drop … and that F428 is filed rather than
  fixed is pending sign-off", and "it reads no socket, since the TCP link's listener outlives the
  link (F428)": the links release their sockets and connections on drop, by the sixty-sixth
  response. The eleven tests join its list or 47's.
- The transports DESIGN, decision 82 (line 1608): the `JoinHandle` kept in the link's state after the
  CONNACK and SUBACK is now aborted by the state's drop, so a loop is guarded for the link's whole
  life, not only until the broker answers.
- The core DESIGN, §9.5 step 6 (line 911): "A server whose socket lives in a task it spawned and
  detached keeps the socket after its drop, as the TCP and UDP RPC links do (F428, open)" names no
  shipped server now.
- `crates/ulo-rpc/tests/closed_app.rs:123`, outside this batch's crates: the comment "The address is
  not read: the link's accept loop is a task of its own, holding the listener, which outlives the
  link" is false now, and the test can require the address to refuse, as batch 42's first version
  did.

## Needs sign-off

Numbered from S1 for this batch.

### S1. The broker links' client sides are fixed under this entry

Decision 1 and 4. The response asked about the broker links' server sides. Their client sides had
the same shape and the same `State` holds both, so the one `Drop` covers each: MQTT's event loop,
Redis's reply reader, RabbitMQ's writer and router and Kafka's router and producer. A client link
dropped without `close` is what a dropped `RpcClient` leaves when the runtime drops the `close` it
spawned unrun, and what a module's client leaves in an app dropped before its destroy hook. Before
the fix the comment on `ClientInner`'s drop, "Its connection is released as its fields drop,
whatever the runtime does with the task" (`crates/ulo-rpc/src/client.rs:713`), held for TCP and UDP
and not for the brokers (p04 to p07). The TCP and UDP client sides need no
change: their tasks end with the outbound, read, and p01 and p02 showed it on TCP.

### S2. The UDP server's leak ends at the next datagram once its inbound stream has dropped

Decision 1, p03. F428 says the UDP address keeps "receiving datagrams for the rest of the process".
With the inbound stream dropped, the first datagram read ends the receive loop, which fails to hand
it over, and the socket is released. With the server dropped, its inbound stream drops with it, so
on the F428 path the socket was held until the next datagram arrived. The UDP test sends none.

### S3. Three guards no test holds

b02, b04, b14. The forwarding task's `Owned` (TCP), the writer's `Owned` (UDP) and the Kafka
client's producer taken in the drop each pass their tests when removed. The forwarding task and the
UDP writer end on their own once the accept or receive loop has and the inbound stream and reply
paths are gone, which is the case when a server drops; the guard makes each task's end independent
of those. The producer is taken so its drop, which flushes for up to half a second, runs on a thread
of its own as at `close`, and not on whichever runtime thread drops the last `ClientSide`. The
response's design holds every loop through an aborting handle, and they are kept.

### S4. A cycle through the conformance probe in RabbitMQ's and Kafka's client sides

Decision 4, b11. With `conformance-probe`, which the crates' own test builds enable, the probe's
`opened` hook holds a clone of the client writer's queue, and `ClientSide` holds the probe: the writer
never sees its last sender go, and holds `ClientSide`. On RabbitMQ that side holds the channel, which
held the connection open, and the client test failed until the writer was aborted too. On Kafka the
writer and side stay in memory in probe builds with no connection, the producer taken. Production
builds have no probe and no cycle, read.

### S5. A leaked broker server kept taking requests

Decision 1, p05 to p07. The MQTT server stayed in its shared-subscription group and answered every
request routed to it `unavailable`, probed; a live instance beside it would have lost that share of
the requests. The RabbitMQ server's consumers stayed registered, probed, and a request delivered to
one reaches an inbound stream nobody reads, its acknowledgment dropped unsettled, read. The Kafka
server stayed a member of its group, probed, its partitions assigned to a member that hands nothing
over, read. F428 described the TCP and UDP addresses alone.

### S6. The drop aborts and does not close

Decision 4. A link dropped without `close` aborts its loops and writes no farewell of its own, and
each broker ends what the link held when its connection closes. Running `close` from a drop would
need an await, a runtime to run it on and a bound, which is what `RpcClient`'s spawned close already
is, and what an app's close step is.

### S7. The batch built in a mirror of `d2e6b26a`

Decision 6. Every probe, break, suite run and check ran on `d2e6b26a` with this batch's crates, not
on the real tree with batches 43's and 45's edits; the workspace test ran on the real tree, their
edits included.

## Verification

Full output in the session scratchpad, `batch44/`: `probes/` with `probes/src/` for the probes,
`broken/` and `specs/` for the breaks, `runs/` for the new tests on the fixed tree, `verify/` with
`summary.txt` for the checks, `superseded/` for runs a later one replaced, and `scripts/`.
`batch44/scripts/verify.sh` ran the checks below in the mirror, one cargo command at a time, after
`cargo clean -p` of the eight crates, the OpenSSL flags set; the tree hash before and after each
group is in `verify/tree-hash.txt`.

- **The TCP and UDP suites:** `ulo-rpc-tcp` `conformance`, `conformance_cbor` and `conformance_tls`
  52 passed and 3 ignored each, `client_address` 3, `client_close` 1, `deadline_answers` 2,
  `drain_end` 1, `dropped_link` 1, `handle` 5, `interop` 2, `late_goaway` 1, 1 doctest ignored;
  `ulo-rpc-tcp-smol`'s three suites 52 and 3 each, `client_address` 1, `client_close` 1,
  `drain_end` 1, `dropped_link` 1, `late_goaway` 1, `runtime` 6; `ulo-rpc-tcp-core` none;
  `ulo-rpc-udp` `conformance` 49 and 6 ignored, `client_address` 3, `client_close` 1, `close_read` 1,
  `drain_end` 1, `dropped_link` 1, `handle` 2.
- **The broker suites and link unit tests with `integration`:** Redis `conformance` 55 passed and 3
  ignored, `handle` 2; MQTT unit 4, `abandoned_connect` 2, `conformance` 61 and 1 ignored, `handle` 2;
  RabbitMQ, with `ULO_CONFORMANCE_PARALLEL=6`, `conformance` 56 and 1 ignored, `handle` 2; Kafka unit
  4, `conformance` 57 and 1 ignored, `handle` 2. The first Kafka run failed one scenario,
  `panic_is_internal`, at startup, "the Kafka container did not start: … End of stream reached before
  finding message", the container exiting before its ready line with the load average at 11
  (`superseded/verify1/kafka-integration.txt`); a second run passed every scenario. **Without
  `integration`:** MQTT unit 3, `abandoned_connect` 2, `handle` 2; Kafka unit 3, `handle` 2; Redis and
  RabbitMQ `handle` 2 each; each `conformance` 0, behind the feature.
- **`ulo-rpc` and gRPC's tests:** `ulo-rpc` 23 unit, `bad_deadline` 7, `closed_app` 2,
  `drain_window` 3, `runtime` 11, `stream_end` 5, doctests 2 passed and 4 ignored and the
  `compile_fail` one; `ulo-codegen-tests` `builtins` 6, `clients` 8, `compile_fail` 1, `deadlines`
  20, `enhancers` 7, `grpc` 7, `replies` 12, `stream_end` 6.
- **The unsafe check:** "self-test: 6 planted lines refused, 2 valid lines accepted", 464 files
  scanned, 2 listed, 0 findings, exit 0.
- **The runtime-levels check,** `/usr/bin/python3 scripts/runtime_levels.py --locked`: 50 members,
  54 entries, 33 checked, 0 failures, 0 errors, exit 0.
- `cargo check --workspace --all-targets --all-features --locked` and `cargo check --workspace
  --all-targets --locked`, and `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo
  --exclude ulo-graphql-async-graphql --locked` in `target/msrv-1.88`: exit 0 each, `ulo`'s 17 known
  warnings and no warning located outside `crates/ulo/src`.
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for the eight crates: exit 0
  each.
- **Clippy,** `cargo +1.98.1 clippy` over the eight crates `--all-targets --all-features --no-deps
  --locked`: exit 0. Two of its locations fall in the batch's crates, each on a line the batch did not
  change: a `clone` on a `Copy` reason code in MQTT's `server_loop`
  (`crates/ulo-rpc-mqtt/src/link.rs:855`) and the non-binding `let` on `drop_detached` in Kafka's
  `Detached::drop` (`crates/ulo-rpc-kafka/src/link.rs:1491`).
- **The workspace test,** in the real tree, `cargo test --workspace --no-fail-fast --locked` with the
  OpenSSL flags: exit 0, 1,319 passed, 0 failed, 141 ignored, over 206 lines starting `test result:`,
  where batch 42 had 1,310 and 141 over 203. It ran on the tree as it stood, batches 43's and 45's
  uncommitted edits to `crates/ulo-rpc-nats`, `crates/ulo` and `crates/ulo-runtime-tokio` included
  (`verify/real-tree-status-before-workspace-test.txt`), so its counts mix the three batches; this
  batch adds three test binaries of one test each, and its broker tests sit behind `integration`.
  The SHA-256 of each of this batch's source and test files matched before and after the run
  (`verify/our-files-start.txt`, `our-files-end.txt`); only this log changed in between.
- **The break runner's restore check:** every probe and break reported `restore ok`, the tree hash
  and each touched file's `cmp` against its copy aside and against the real tree identical
  (`probes/summary.txt`, `broken/summary.txt`).
- **The prose:** batch 42's scan for the banned register and for unfilled template braces, asserted
  first against a planted line (`verify/planted.txt`), over this log and every added line under the
  batch's crates (`verify/added-lines.txt`), found none; a scan for paragraphs stacking `so`,
  `because`, `since` or `therefore`, asserted against a planted line, found one, an "and so do" that
  marks no inference. The scans check vocabulary only.
- **Containers and images:** at the batch's start twelve containers, the user's, every one exited,
  and 25 images (`batch44/docker-start.txt`); at its end the same twelve by id, still exited, and the
  same 25 images (`batch44/docker-end.txt`). Every probe's, break's and verification group's
  container list after it matches its list before, with batch 43's `nats:*` containers set aside
  (decision 6). The batch's own containers were the tests' testcontainers, each removed with its
  test. The batch pulled no image, ran no `docker ... prune` and started, stopped or removed nothing
  of the user's.

## Ledger text for the coordinator

### Fix note, appended under F428

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 44, in
the six links, as the sixty-sixth transports response set out: each loop a link spawns is held
through a handle that aborts on drop, held by the link and not by the state the loop holds. TCP's
`Core` holds the accept loop and the forwarding task as `Owned` handles
(`crates/ulo-rpc-tcp-core/src/link.rs:94`, `:97`, `:102`), out of the `State` the loop holds, for
both TCP links; UDP's `Udp` holds its receive task and writer the same way
(`crates/ulo-rpc-udp/src/link.rs:70`, `:72`, `:77`). The broker links' `State`, the link's alone,
aborts both sides' tasks in its `Drop`: MQTT's event loops (`crates/ulo-rpc-mqtt/src/link.rs:100`),
Redis's Pub/Sub readers (`crates/ulo-rpc-redis/src/link.rs:67`), RabbitMQ's consumers and the
client's writer and router (`crates/ulo-rpc-rabbitmq/src/link.rs:99`), and Kafka's two lanes and the
client's router, its producer dropped on a thread of its own (`crates/ulo-rpc-kafka/src/link.rs:121`).
`close` is unchanged. The premise held for every broker link, server and client side: batch 44's
`probes/p04-*` to `p07-*`, on `d2e6b26a`, kept every relayed connection at every sample, the last
2 s to 8 s after each side's drop, with Redis's channels, RabbitMQ's consumers, the MQTT server's shared subscription, which
answered routed requests `unavailable`, and the Kafka groups' members. `p01-*` and `p02-*`: both TCP
links' addresses accepted and answered `AddrInUse` 1.5 s after the drop. `p03-*`: the UDP address
answered `AddrInUse` 1.2 s after the drop and was released by the next datagram, the receive loop
ending on the inbound stream's drop, so on this entry's path the socket was held until a datagram
arrived. The TCP and UDP client sides end with their outbound. Held by
`a_server_link_dropped_without_close_releases_its_address` in `ulo-rpc-tcp`'s, `ulo-rpc-tcp-smol`'s
and `ulo-rpc-udp`'s `tests/dropped_link.rs`, and by
`a_server_link_dropped_without_close_closes_its_connection[s]` and
`a_client_link_dropped_without_close_closes_its_connection[s]` in each broker's `tests/conformance.rs`,
behind `integration`, each counting through a relay the connections the broker still holds. With the
aborting drop removed each fails (`broken/b01-*`, `b03-*`, `b05-*` to `b13-*`, `b15-*`); the TCP
forwarding task's guard, UDP's writer guard and the Kafka client's producer take hold no test
(`b02-*`, `b04-*`, `b14-*`).
