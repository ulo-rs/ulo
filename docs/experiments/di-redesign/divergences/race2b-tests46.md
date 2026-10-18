# Divergences: race 2b, tests batch 46, broker servers dropped holding a call, Kafka's server producer off the worker, and the NATS roots' logs (F429 to F432)

F429 holds, by probe, on Redis, RabbitMQ and Kafka: a server's link dropped holding a call whose
delivery nobody answers kept one connection at the broker 10 s (Redis, RabbitMQ) and 30 s (Kafka)
after the drop. MQTT kept no connection and kept the table the call sat in, as the entry read. Each
of the four links' `ServerSide` now clears its held calls on drop, NATS's shape, and each link's
drain watcher holds the side weakly, which the entry did not name: a drained server dropped holding
a call kept its side through the watcher, 4 connections on Kafka before any fix, and with the side's
drop in place 1 on Redis and RabbitMQ and 3 on Kafka (S1). F430 holds too: Kafka's server producer
dropped on the thread polling it held that thread 20.5 to 101.5 ms a drop over the runs measured;
it is held in the link's `Detached` from its creation. F431's unanswered load now logs a `warn`.
F432's correction is made in `Misconfigured`'s documentation and in the comment on `connect_error`
beside it. The two NATS `warn`s of batch 43's S3 are pinned at one per distinct cause.
`closed_app.rs` now requires the dropped app's address to refuse. Nine new or changed tests hold
the items, and each fails against a break.

Files changed: `crates/ulo-rpc-redis/src/link.rs`, `crates/ulo-rpc-rabbitmq/src/link.rs`,
`crates/ulo-rpc-kafka/src/link.rs`, `crates/ulo-rpc-mqtt/src/link.rs`,
`crates/ulo-rpc-nats/src/link.rs`, `crates/ulo-rpc/src/link.rs`, `crates/ulo-rpc/src/client.rs`;
the tests in the four broker crates' `tests/conformance.rs` and `crates/ulo-rpc/tests/closed_app.rs`.
`Cargo.lock` is unchanged. The batch started from `53222b56`, built in the real tree, made no commit,
and did not edit `FRAMEWORK_GAPS.md`, `RESPONSE.md`, either DESIGN or the integrations folder; the
ledger text is at the end of this log, its commit written `<commit>`. Versions are unchanged at
`0.1.0-alpha.1`.

## The signatures

No public signature changes.

```rust
// crates/ulo-rpc-{redis,rabbitmq,kafka,mqtt}/src/link.rs, private
impl Drop for ServerSide { /* clears the held calls */ }   // new on each

// crates/ulo-rpc-kafka/src/link.rs, private
async fn server_writer(producer: Detached<FutureProducer>, ..);   // was `producer: FutureProducer`
```

What changes in meaning without a signature:

- A broker server's link dropped without `close` closes its connections when it holds calls, and
  when it was drained first.
- Kafka's server producer drops on a thread of its own, at `close` and on the drop path.
- A `nats://` server whose roots' load ends unanswered logs a `warn`.

## Decisions

Line numbers are those of the batch's tree.

### 1. F429's premise, by probe

The probes are the new tests run on the unchanged links (decision 4), in the real tree, the tests
added and the links not yet touched. Output in `batch46/probes/`.

- **Redis (p01, `probes/p01-redis.txt`).** "the server's link dropped holding a call: 1 connections
  still open at the broker 10s after the link's drop". The server's relay carries the publisher
  connection the detached writer holds; the Pub/Sub connection closed with the aborted reader.
- **RabbitMQ (p02, `probes/p02-rabbitmq.txt`).** The same, 1 connection after 10 s: the writer's
  `Channel` keeps lapin's connection open.
- **Kafka (p03, `probes/p03-kafka.txt`).** 1 connection 30 s after the drop, the server having
  answered one call before holding the next. A first version of the test held its only call and
  answered none (`superseded/p03-kafka.txt`): the undrained phase passed, the producer leaked and
  holding no connection, and the drained phase failed with 4 connections, the watcher keeping the
  whole side, its consumers included. A producer that has produced nothing holds no connection
  once librdkafka has read the broker's metadata, read and probed here (S2).
- **MQTT (p04, `probes/p04-mqtt.txt`; p05, `probes/p05-mqtt-table.txt`).** The broker test passed
  on the unchanged link: the event loop is aborted by the state's drop and the connection closes.
  The unit test failed, "the server's link dropped holding a call kept its table 5s after the
  drop": the leak is memory, the table and the request channel its paths hold.

Read, to locate each: a held call's `ReplyPath` (`ServerSide::hold`) holds the writer's queue
(Redis, RabbitMQ, Kafka) or the event loop's `AsyncClient` (MQTT) and the `Arc<Calls>` it sits in,
and the table holds the path. The writer ends only when every sender of its queue has dropped, so
the cycle keeps the writer and what it holds: Redis's `ConnectionManager`, RabbitMQ's `Channel`,
Kafka's `FutureProducer`. Each drain spawns a watcher holding `Arc<ServerSide>` until the table
reaches zero, which a held call never lets it do: Redis `:218`, RabbitMQ `:309`, Kafka `:397`, MQTT
`:350`.

### 2. A side's drop clears its held calls; the drain's watcher holds it weakly

Each of the four `ServerSide`s has NATS's `Drop`, clearing the table: Redis `:373`, RabbitMQ `:418`,
Kafka `:692`, MQTT `:636`. A path the core still holds keeps working, holding the writer's queue
and the table itself; what is cleared is the table's own copy, which only the lanes, aborted by the
state's drop, read. Each drain's watcher subscribes to the count before it is spawned and holds a
`Weak<ServerSide>`: the side's drop clears the table, the count reaching zero wakes the watcher,
and its upgrade fails and it returns. A watcher that finds the side alive does what it did before:
Redis and MQTT end the inbound stream if no call is held, RabbitMQ and Kafka take the delivery
sender.

### 3. Kafka's server producer is `Detached` from its creation

`listen` wraps the producer in `Detached` where it makes it, inside the task that makes the
consumers (`crates/ulo-rpc-kafka/src/link.rs:291`), and `server_writer` takes the `Detached` (`:983`),
producing through its `Deref`. The producer drops wherever the writer ends, at `close` once the
side has gone or after a dropped link's side has gone, or wherever the runtime drops the task, and
each drop runs on `drop_detached`'s thread. `Detached`'s documentation names the server's producer
beside the consumer and the admin client (`:1484`), and `drop_detached`'s names a producer for the
client's. `close` does not wait for the server producer's drop, as it did not wait for the writer
before (S8). The client's producer was already taken by `ClientSide::close` and by the state's drop.

In rdkafka 0.39.0, read: `ThreadedProducer`'s drop sets the stop flag and joins its polling thread,
which polls 100 ms at a time (`src/producer/base_producer.rs:690`, `:798`-`:813`), and the
`BaseProducer`'s drop then purges and flushes for up to 500 ms (`:629`-`:641`). Measured on the
thread (p06, `probes/p06-*-0.txt`), the server's producer took 42.5 and 46.7 ms, under the suite's
50 ms bound, which is why the test's bound is 25 ms (S3). A `rd_kafka_poll` returns once it has
served an event, so the join's wait depends on where in its poll the thread is: 36.0 to 102.9 ms for
the client's producer across the runs below.

### 4. The tests

New:

- `a_server_link_dropped_holding_a_call_closes_its_connection[s]` in each broker's
  `tests/conformance.rs`, behind `integration`: Redis `:185`, RabbitMQ `:138`, Kafka `:306`, MQTT
  `:415`. Per broker one container, a client connected outside the server's relay, and two phases,
  undrained then drained, each with a server's link of its own: the request reaches the server, the
  drained phase drains, and the delivery, the inbound stream and the link drop; the server's relay
  must carry no connection within `RELEASE`, 10 s and 30 s on Kafka. On Kafka each server answers
  one call before holding the next (S2), the answer checked at the client. The MQTT test is a guard,
  passing with or without the fix.
- `link::tests::a_server_link_dropped_holding_a_call_lets_go_of_its_table`
  (`crates/ulo-rpc-mqtt/src/link.rs:1529`), no broker: a `ServerSide` built with an `AsyncClient`
  whose event loop is never polled, a pending task standing in for the loop, and no patterns, so
  the drain waits for no UNSUBACK; a call held and its path dropped, the side put in the link's
  state, drained in the second phase, and the link dropped; a `Weak` of the table must die within
  5 s (S7).
- `closing_and_dropping_links_does_not_stall_the_thread` (`crates/ulo-rpc-kafka/tests/conformance.rs:374`),
  behind `integration`: on `OnTokio::block_on_one_thread` beside a ticker sleeping a tenth of
  `DROP_STALL`, 25 ms (`:360`), a server answers a call and is closed, a second answers one and is
  dropped, and the client that called both produces an event and is dropped, each phase waiting
  until its relay carries nothing, so the producer's drop falls inside the window. The longest gap
  is kept per phase and each must stay under the bound; the failure lists every phase's longest
  gap. The probe's hooks are replaced with empty ones before the client's drop (S4), and the event
  puts the client producer's polling thread at the start of a poll (decision 3).
- `link::tests::a_nats_server_s_roots_log_one_warn_per_cause` and
  `link::tests::a_nats_server_whose_roots_load_ends_unanswered_says_so`
  (`crates/ulo-rpc-nats/src/link.rs:2203`, `:2237`), behind `integration` for the process
  subscriber `capture`, no broker (decisions 5 and 6).

Changed:

- `a_bound_app_dropped_unserved_drops_its_server_s_link` (`crates/ulo-rpc/tests/closed_app.rs`):
  reads the server's address through `App::addresses` (`:133`), requires it to accept a connection
  while bound, and after the app's drop requires a connection to it refused with
  `ConnectionRefused` within 5 s (`:140`), batch 44's `refuses` (`:78`). The comment saying the
  address is not read is gone, and the module documentation names F428.
- The four broker conformance files' module documentation names the new test; Kafka's names the
  stall test.
- `LOADS` and `GATES` in the NATS tests widen to 28 and 5 slots; `gated_unreadable` (`:1494`) is
  new, behind `integration`.

### 5. F431: an unanswered load logs a `warn`

`Roots::awaited`'s `Ok(None)` arm (`crates/ulo-rpc-nats/src/link.rs:1105`) logs one `warn`: the
load ended without an answer, the runtime it ran on having shut down, so the server connects without
the file, with the expiry's sentence on what an upgraded connection then does. The documentation of
`awaited` says when a load ends unanswered. `warn` and not `debug` (S5): the load may have run on
another link's runtime, a link joining the process's load, and a server on a runtime that is still
up then connects without the file for its app's life, which is the expiry's consequence and the
expiry's level. Where the link's own runtime has shut down, the connect that follows fails and is
reported, and the `warn` is one extra line in a shutdown. The test starts the link's load on a
runtime shut down on a thread of its own, its handle kept, with `roots_timeout` unbounded: the wait
answers `None`, one `warn` carries the text, and the loader never ran.

### 6. The NATS server's two `warn`s are one per cause

Batch 43's S3, read: `prepare` calls `Nats::roots(.., false)` (`:273`), which on `nats://` calls
`Roots::ready` (`:1132`); a load still running there arms `warn_when_failed` (`:1142`, `:1228`), a
task that logs the load's failure through `Load::warn` (`:1217`), once per load by its `warned`
flag. `listen` calls `Nats::roots(.., true)` (`:287`), `Roots::awaited`, whose expiry logs its own
`warn` (`:1121`) and whose failure arm goes through `Load::warn`. A load outlasting the bound and
failing later logs two: the expiry's and the failure's with its cause. The count is
pinned at that, one per distinct cause, and not made one: the expiry says the server connected
without the file because the load was slow, the failure carries why the load failed, which an
operator needs to mend the store, and the process's load is shared with the `nats://` clients that
joined it (S6). `a_nats_server_s_roots_log_one_warn_per_cause` makes both calls on a link whose
load is gated past a 200 ms bound and then fails: one expiry line and one failure line; then on a
link whose load fails at once: still one failure line for it, the wait and the armed task sharing
the flag; each link loads once.

### 7. F432 and the comment beside it

`Misconfigured`'s documentation (`crates/ulo-rpc/src/link.rs:430`-`:437`) names the fourth cause, a
`tls://` NATS link whose roots' load outlasted `roots_timeout`, and says no retry mends the other
three, whose cause the link keeps, while a later call mends the fourth once the load has ended. The
kind is unchanged. `connect_error`'s comment in `crates/ulo-rpc/src/client.rs:795`, "which every
call meets", said the same of every `Misconfigured` and now names the exception. Neither has a
test: both are documentation, and batch 43's
`a_tls_link_whose_roots_outlast_roots_timeout_is_refused` already holds the later call joining the
running load.

### 8. The breaks

Through `batch46/scripts/brk.py`, batch 44's runner pointed at the real tree: each touched file
copied aside before the edit, compared with `cmp` after the restore, and a tree hash over the
batch's crates, `Cargo.toml` and `Cargo.lock` compared before and after; every restore reported
`ok`. `cmp`'s exit on two different files was checked first (1). Output in `batch46/broken/`, with
`summary.txt` and `runner.txt`.

- b01, Kafka's server producer dropped on the worker again (`drop(producer.into_inner())` at the
  writer's end), at the suite's 50 ms bound: the stall test passed, the drop taking 42.5 and
  46.7 ms (p06). b01r, the same at 25 ms, three runs: each failed, the closing phase at 70.2, 45.0
  and 101.5 ms and the dropping phase at 20.5, 40.1 and 49.0 ms. m03, the same, the assertion made to
  fail to print the gaps: closing 38.6 to 45.8 ms, dropping 40.1 to 47.3 ms.
- b02, the client producer's take removed from the state's drop, at 50 ms: failed, "dropping the
  client 106.3 ms". b02r at 25 ms with the event before the drop, three runs: each failed, 36.0, 93.7
  and 102.9 ms. Without the event (`superseded/b02r/`) one run of three failed, at 35.8 ms, which is
  why the event is there.
- m02, the fixed tree, the assertion made to fail to print the gaps, three runs: every phase 3.8
  to 13.6 ms.
- b03, b05, b07, Redis's, RabbitMQ's and Kafka's side drop a no-op: each held-call test failed in
  its undrained phase, 1 connection open after 10 s, 10 s and 30 s.
- b04, b06, b08, each watcher holding the side strongly: each failed in its drained phase, 1, 1 and
  3 connections open.
- b09, b10, MQTT's side drop a no-op, then its watcher strong: the broker test passed both times and
  the unit test failed, "kept its table 5s after the drop", undrained and then drained.
- b11, the unanswered load's `warn` removed: the F431 test failed, "logged other than once: []".
- b12, the expiry setting the load's `warned` flag, which makes the count one: the per-cause test
  failed, `(1, 0)`.
- b13, `Load::warn` logging on every call: the per-cause test failed at the second link, `(1, 3)`.
- b14, `Owned`'s drop a no-op in `ulo-rpc-tcp-core`, the accept loop detached again: the
  `closed_app` test failed, "the address still accepts connections 5s after the app's drop".

## The tests

- **F429:** `a_server_link_dropped_holding_a_call_closes_its_connection[s]` in the Redis, RabbitMQ,
  Kafka and MQTT `tests/conformance.rs`, behind `integration`;
  `link::tests::a_server_link_dropped_holding_a_call_lets_go_of_its_table` in `ulo-rpc-mqtt`.
  Decisions 1, 2, 4 and 8.
- **F430 and batch 44's S3 guard b14:** `closing_and_dropping_links_does_not_stall_the_thread` in
  `ulo-rpc-kafka`'s `tests/conformance.rs`, behind `integration`. Decisions 3, 4 and 8.
- **F431:** `link::tests::a_nats_server_whose_roots_load_ends_unanswered_says_so`, behind
  `integration`. Decision 5.
- **Batch 43's S3:** `link::tests::a_nats_server_s_roots_log_one_warn_per_cause`, behind
  `integration`. Decision 6.
- **F428's address in `ulo-rpc`:** `a_bound_app_dropped_unserved_drops_its_server_s_link` in
  `tests/closed_app.rs`. Decision 4.

### Before and after

| Run | Target | Result |
| --- | --- | --- |
| p01 to p03. the held-call tests on the unchanged links | Redis, RabbitMQ, Kafka | 1 connection open after 10 s, 10 s and 30 s |
| p03 first version, no call answered | Kafka | undrained passed with the producer leaked and idle; drained 4 connections |
| p04, p05. the same on MQTT | broker test, unit test | broker test passed; the table kept 5 s |
| p06. the server producer's drop timed on the worker | Kafka | 42.5 and 46.7 ms |
| the fixed tree | every new and changed test | all passed (`runs/`) |
| m02. the fixed tree's gaps | the stall test | 3.8 to 13.6 ms |
| b01. server producer on the worker, 50 ms | the stall test | passed (S3) |
| b01r, m03. the same, 25 ms | the stall test | failed 3 of 3; closing 38.6 to 101.5 ms, dropping 20.5 to 49.0 ms |
| b02, b02r. the client producer kept | the stall test | failed at 106.3 ms; 3 of 3 at 25 ms, 36.0 to 102.9 ms |
| b03, b05, b07. side drop a no-op | the held-call tests | failed undrained, 1 connection each |
| b04, b06, b08. watcher strong | the held-call tests | failed drained, 1, 1 and 3 connections |
| b09, b10. the same on MQTT | broker and unit tests | broker test passed; unit test failed |
| b11. unanswered load unlogged | the F431 test | failed |
| b12, b13. one `warn` in all, or one per call | the per-cause test | failed, `(1, 0)` and `(1, 3)` |
| b14. TCP accept loop detached | `closed_app` | failed, the address accepting after 5 s |

## Left for the DESIGN folds

Line numbers are those of the transports DESIGN at `53222b56`.

- Decision 152 (line 1678): the broker links' server sides also clear their held calls on drop,
  and each drain's watcher holds the side weakly (F429); the held-call tests and the MQTT unit test
  join its list. A drained server dropped holding a call kept its side through the watcher, which
  152 does not name.
- Decision 151 (line 1677): "a server side's drop clears its held calls" holds on all five broker
  links. Its pending item, "that this link's fix read the other broker links without probing
  them, which 152 does", is answered for held calls by this batch's probes.
- Decision 149 (line 1675): "Kafka's link moves its flushing drops to a thread of its own" was false
  for the server's producer and holds now (F430).
- Decisions 133 and 141 (lines 1659, 1667): `Detached` holds the server's producer from its
  creation, beside the admin client and the reply consumer; the stall test, its 25 ms bound and its
  numbers.
- Decision 150 (line 1676): "logs two `warn`s" holds and is pinned by the per-cause test; the
  unanswered load's `warn` (F431) is a third path to a server connecting without the file.
- Decision 149's "it reads no socket" in its fifth test: `closed_app` now reads the address.
- §5.3's `Link::connect` block (line 994): "A refusal no retry mends ... is answered
  `Misconfigured`" omits the expired `tls://` roots wait, which a later call mends (F432).
- Decision 135 (line 1661): the same, if it repeats that no retry mends a `Misconfigured`.

## Needs sign-off

Numbered from S1 for this batch.

### S1. The drain's watcher is fixed under F429

Decision 2. F429 names the reply path cycle. A drained server holds a second holder of its side,
the watcher waiting for the table to empty, and dropped holding a call it kept the side and its
connections even with the side's drop in place (b04, b06, b08); p03's first version showed it on
Kafka before any fix, 4 connections. A drained server is dropped without `close` when an app's
shutdown is abandoned during the drain. The watcher now holds the side weakly on all four links.

### S2. An idle Kafka producer holds no connection

Decision 1. A producer that has produced nothing closed its bootstrap connection once it had the
broker's metadata, and opens one to the partition leader only to produce, so a leaked server that
never answered leaks its producer, its polling thread and librdkafka's threads, and no connection
the relay can count. The Kafka held-call test answers one call first. The leak without an answer
is held through the same `Drop`, which the test's undrained phase reaches either way.

### S3. The Kafka stall test's bound is 25 ms

Decisions 3 and 8. The server's producer dropped on the thread held it 42.5 and 46.7 ms, under the
suite's 50 ms `Budget::stall`, and b01 passed at 50. At 25 ms every break run failed and the fixed
tree's longest gap was 13.6 ms. MQTT's and NATS's TLS stall tests use 25 ms for their own reasons.
One break run had the dropping phase at 20.5 ms, under the bound, its closing phase failing the run.

### S4. The probe's hooks are replaced before the client's drop

Decision 4. With `conformance-probe`, which the crate's test builds enable, the probe's `opened`
hook holds the client writer's queue and the client's side holds the probe, so the side, and the
producer in it, never drops (batch 44's S4). With the take removed and the hooks left, the test
would pass with the producer leaked. Attaching empty hooks drops the old ones, which a production
build never had.

### S5. F431's line is a `warn`

Decision 5. The brief allowed `debug` or `warn`. A link that joined the process's load can find it
unanswered while its own runtime runs on, and its server then connects without the file for its
app's life.

### S6. The two `warn`s are kept, one per cause

Decision 6. Making it one would drop the failure's cause or the expiry's statement; the test pins
one per cause.

### S7. MQTT's memory is held by a unit test without a broker

Decision 4. The relay cannot see memory. The unit test builds the side by hand and stands a pending
task in for the event loop; the drained phase works only because a server with no patterns waits
for no UNSUBACK. The broker test is a guard for the connection.

### S8. Kafka's server `close` does not wait for its producer's drop

Decision 3. The client's `close` awaits `drop_detached` on its producer; the server's `close`
returns once its commit has run, and the writer, and the producer's flush on its thread, end after.
This was so before the batch, the writer being detached.

## Verification

Full output in the session scratchpad, `batch46/`: `probes/`, `broken/`, `specs/`, `runs/`,
`verify/` with `summary.txt` and `tree-hash.txt`, `superseded/` and `scripts/`.
`batch46/scripts/verify.sh` ran the checks below in the real tree, one cargo command at a time, the
OpenSSL flags set.

- **The broker suites with `integration`:** Redis `conformance` 56 passed and 3 ignored, `handle`
  2; MQTT unit 5, `abandoned_connect` 2, `conformance` 62 and 1 ignored, `handle` 2; RabbitMQ, with
  `ULO_CONFORMANCE_PARALLEL=6`, `conformance` 57 and 1 ignored, `handle` 2; Kafka unit 4,
  `conformance` 59 and 1 ignored, `handle` 2; NATS unit 28, one thread at a time, `conformance` 54
  and 1 ignored, `handle` 2, `two_providers` 3. Each broker `conformance` is one more than batch
  44's, Kafka's two. The first Kafka run failed one scenario, `two_instances`, at startup, "the Kafka
  container did not start: ... End of stream reached before finding message", the container exiting
  before its ready line, as batch 44's first run did (`superseded/kafka-integration-run1.txt`); the
  second run passed every scenario. **Without `integration`:** MQTT unit 4, `abandoned_connect` 2,
  `handle` 2; Kafka unit 3, `handle` 2; NATS unit 17, `handle` 2, `two_providers` 3; Redis and
  RabbitMQ `handle` 2 each; each `conformance` 0, behind the feature.
- **The TCP and UDP suites,** `closed_app.rs` having changed: `ulo-rpc-tcp` `conformance`,
  `conformance_cbor` and `conformance_tls` 52 passed and 3 ignored each, `client_address` 3,
  `client_close` 1, `deadline_answers` 2, `drain_end` 1, `dropped_link` 1, `handle` 5, `interop` 2,
  `late_goaway` 1; `ulo-rpc-tcp-smol`'s three suites 52 and 3 each, `client_address` 1,
  `client_close` 1, `drain_end` 1, `dropped_link` 1, `late_goaway` 1, `runtime` 6;
  `ulo-rpc-tcp-core` none; `ulo-rpc-udp` `conformance` 49 and 6 ignored, `client_address` 3,
  `client_close` 1, `close_read` 1, `drain_end` 1, `dropped_link` 1, `handle` 2.
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
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p <crate> --locked` for `ulo-rpc-redis`,
  `ulo-rpc-rabbitmq`, `ulo-rpc-kafka`, `ulo-rpc-mqtt`, `ulo-rpc-nats` and `ulo-rpc`: exit 0 each.
- **Clippy,** `cargo +1.98.1 clippy` over the same six crates `--all-targets --all-features
  --no-deps --locked`: exit 0. None of its locations is on a line the batch changed: batch 44's two
  in the broker crates, the `clone` on a `Copy` reason code in MQTT's `server_loop`
  (`crates/ulo-rpc-mqtt/src/link.rs:866`) and the non-binding `let` in Kafka's `Detached::drop`
  (`crates/ulo-rpc-kafka/src/link.rs:1508`), and eight in `ulo-rpc` (`src/__private.rs:216`, `:220`,
  `:224`, `src/client.rs:219`, `src/link.rs:167`, `:219`, `src/extract.rs:53`, `:60`).
- **The workspace test,** `cargo test --workspace --no-fail-fast --locked` with the OpenSSL flags:
  exit 0, 1,320 passed, 0 failed, 141 ignored, over 208 lines starting `test result:`, where batch 44
  had 1,319 and 141 over 206; the one more is MQTT's unit test, the batch's other tests sitting
  behind `integration`. The SHA-256 of each changed file matched before and after the run
  (`verify/our-files-start.txt`, `our-files-end.txt`).
- **The tree hash:** the first verification run's start and end hashes differ
  (`verify/tree-hash.txt`): two comments changed during it, `connect_error`'s in
  `crates/ulo-rpc/src/client.rs` after the Redis group had run and one in the Kafka stall test after
  the Kafka group had run. The Kafka suite was run again on the final tree, and the Kafka rerun and
  the workspace test each start and end on the final hash.
- **The break runner's restore check:** every probe, measurement and break reported `restore ok`,
  the tree hash equal before and after and each touched file's `cmp` against its copy aside
  identical (`probes/summary.txt`, `broken/summary.txt`).
- **The prose:** batch 44's scan for the banned register and for unfilled template braces, asserted
  first against a planted line (`verify/planted.txt`), over this log and every added line under
  `crates/` (`verify/added-lines.txt`), found one `just`, in a comment of the Kafka stall test, which
  was replaced; a scan for paragraphs stacking `so`, `because`, `since` or `therefore`, asserted
  against a planted line, found three, trimmed where a marker marked no step. The scans check
  vocabulary only.
- **Containers and images:** at the batch's start 12 containers, the user's, every one exited, and 25
  images (`batch46/docker_start.txt`); at its end the same 12 by id, still exited, and the same 25
  images (`batch46/docker_end.txt`). Every probe's, break's and verification group's container list
  after it matches its list before. The batch's containers were the tests' testcontainers, each
  removed with its test, and one `redis:7-alpine` left by a verification run the batch stopped,
  removed by its id, `dc281c0069d3`, which the start list does not hold. The batch pulled no image,
  ran no `docker ... prune` and started, stopped or removed nothing of the user's. The temp
  directory's `ulo-rpc-nats-roots-*` entries were left as they were, 1,477 at the end; the batch's
  NATS tests write no roots file.

## Ledger text for the coordinator

### Fix note, appended under F429

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 46.
The premise held, by probe (batch 46's `probes/p01-*` to `p05-*`, the new tests on the unchanged
links): a server's link dropped holding a call nobody answers kept 1 connection at the broker 10 s
after the drop on Redis and RabbitMQ and 30 s after it on Kafka, its server having answered a call
first, since a Kafka producer that has produced nothing holds no connection; MQTT kept no connection
and kept the table. A drained server dropped holding a call kept its side through the drain's
watcher too, 4 connections on Kafka. Each of the four links' `ServerSide` now clears its held calls
on drop, NATS's shape (Redis `crates/ulo-rpc-redis/src/link.rs:373`, RabbitMQ
`crates/ulo-rpc-rabbitmq/src/link.rs:418`, Kafka `crates/ulo-rpc-kafka/src/link.rs:692`, MQTT
`crates/ulo-rpc-mqtt/src/link.rs:636`), and each drain's watcher holds the side weakly (`:218`,
`:309`, `:397`, `:350`). Held by `a_server_link_dropped_holding_a_call_closes_its_connection[s]` in
each broker's `tests/conformance.rs`, behind `integration`, undrained and drained, counting through
a relay the connections the broker still holds, and by
`link::tests::a_server_link_dropped_holding_a_call_lets_go_of_its_table` in `ulo-rpc-mqtt`, which
requires the table to drop. With the side's drop a no-op each broker test fails undrained
(`broken/b03-*`, `b05-*`, `b07-*`), with the watcher strong drained (`b04-*`, `b06-*`, `b08-*`); on
MQTT the broker test passes both breaks and the unit test fails each (`b09-*`, `b10-*`).

### Fix note, appended under F430

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 46.
`listen` holds the server's producer in `Detached` from its creation
(`crates/ulo-rpc-kafka/src/link.rs:291`) and `server_writer` takes it (`:983`), so the producer drops
on `drop_detached`'s thread wherever the writer ends or the runtime drops it. Measured on the thread
polling it, the producer's drop took 42.5 and 46.7 ms (batch 46's `probes/p06-*`). Held by
`closing_and_dropping_links_does_not_stall_the_thread` in `tests/conformance.rs`, behind
`integration`: on one thread beside a ticker, a server answers a call and is closed, a second
answers one and is dropped, and the client is dropped after an event, each phase's longest gap under
25 ms; the fixed tree's was 3.8 to 13.6 ms. With the producer dropped on the worker again it fails
3 of 3 runs, the closing phase at 45.0 to 101.5 ms (`broken/b01r-*`); at the suite's 50 ms it
passed (`b01-*`). The same test holds the client's producer take in the state's drop, batch 44's
unheld guard: with it removed the test fails 3 of 3, 36.0 to 102.9 ms (`b02r-*`).

### Fix note, appended under F431

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 46.
`Roots::awaited`'s `Ok(None)` arm (`crates/ulo-rpc-nats/src/link.rs:1105`) logs one `warn`: the
load ended without an answer, the runtime it ran on having shut down, and the server connects
without the file. A `warn`, as the expiry's: a link that joined the process's load can find it
unanswered while its own runtime runs on. Held by
`link::tests::a_nats_server_whose_roots_load_ends_unanswered_says_so`, behind `integration`, its
load started on a runtime shut down first; with the line removed it fails (`broken/b11-*`). The
two `warn`s a load outlasting the bound and failing later produces are pinned at one per cause by
`link::tests::a_nats_server_s_roots_log_one_warn_per_cause` (`b12-*`, `b13-*`).

### Fix note, appended under F432

*Appended 2026-10-18:* Fixed on `experiment/di-redesign` in `<commit>` by race 2b's test batch 46.
`Misconfigured`'s documentation (`crates/ulo-rpc/src/link.rs:430`-`:437`) names a `tls://` NATS
link whose roots' load outlasted `roots_timeout` and says a later call mends it once the load has
ended, where no retry mends the other three causes; the kind is unchanged. The comment on
`connect_error` (`crates/ulo-rpc/src/client.rs:795`), "which every call meets", names the exception
too. No test: both are documentation, and batch 43's
`a_tls_link_whose_roots_outlast_roots_timeout_is_refused` holds the later call joining the load.
