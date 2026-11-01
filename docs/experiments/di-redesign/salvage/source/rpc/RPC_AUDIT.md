# The RPC wire audit

The RPC wire across all seven transports — tcp, udp, nats, redis, rabbitmq, mqtt, kafka — against
the ADR-0032 stream grammar and against each broker's own published request-reply convention. The
brief is `RPC_AUDIT_HANDOFF.md`; its scope line names the four conventions `STANDARDS_AUDIT.md`
recorded as *"claimed and unchecked"* — AMQP `reply-to` / `correlation-id`, MQTT v5
response-topic / correlation-data, NATS request-reply, and Kafka headers.

Out of scope by the brief: guards, interceptors and the error chain on RPC, which the pipeline audit
took across all four transports at once.

## Where the findings live

`FRAMEWORK_GAPS.md`, **F216–F234**. This pass was invoked as an audit of the framework rather than of
the documentation site, so everything it found files there.

Four existing entries move. **F63's count is wrong** — `overloaded` is not the one status outside
the `ErrorKind` vocabulary, it is one of three, and F223 carries the other two. **A5 in
`DOCS_AUDIT.md` §17 is confirmed at its wider reading**, with the five adapters' panic sites counted:
nats 3, redis 2, rabbitmq 8, kafka 4, mqtt 0. **F46 gains a consequence** — dispatch by string match
inside one controller is also why two controllers can claim one pattern without anything noticing
(F226). **F215's wording is half right** — `WsConnectionCallbacks` is not the framework's odd one
out, because `RpcMessageCallbacks` has the same shape; the four adapters are traits and the two
dispatch callbacks are not (F234).

This memo does not restate the findings. It says what they add up to and which calls are the user's.

## Not a build

Nothing was changed. No branch exists. The probe was written into the tree, run, and removed;
`git status` is clean at `654c7183`. The probe is kept in `artifacts/` beside this memo as
`probe-cases.rs` (three cases stamped across the five broker suites through the existing
`conformance_suite!` macro) and `rpc_wire_probe.rs` (six cases for tcp and udp, which need no
broker). Both are promotable.

## Confidence marking

- **Probed** — run this session against master `654c7183`, against live brokers under Docker
  (nats 2.10, redis, mosquitto, rabbitmq, apache kafka, all from testcontainers) and against
  in-process tcp and udp listeners. The tables in §1, §2 and §3 are probe output.
- **Read** — read off the source, cited by file and line.
- **Spec** — the published document's own words, quoted rather than paraphrased. MQTT v5 was
  fetched from `docs.oasis-open.org` and read by section; the AMQP 0-9-1 class definitions were
  fetched from `rabbitmq.com/resources/specs/amqp0-9-1.xml` and read by field.
- **Unverified** — believed, not established. Marked inline and not acted on.

**The thing the brief said to settle first is settled: the suite runs.** `ulo-rpc-conformance` had
never executed. It does now, against all five brokers, and **all five pass all six cases** — nats
12 s, redis 8 s, mqtt 14 s, rabbitmq 30 s, kafka 578 s for nine cases — the six plus the three
probe cases below, which that run had already picked up. No case was changed to make that true. The suite
is therefore evidence, and §13 says what it is evidence *of*, which is narrower than its own crate
doc claims.

The one place a spec reading could have gone wrong, and was checked: **AMQP does not decide
request-reply.** The 0-9-1 class definition marks `correlation-id` and `reply-to` *"For application
use, no formal behaviour"*, the latter adding only *"may hold the name of a private response queue,
when used in request messages"*. An audit that took "ulo follows the AMQP convention" as a
conformance claim would have been checking against a document that declines to specify it. What ulo
follows there is RabbitMQ's own tutorial convention, which is a different kind of thing.

## The shape of it

**The framing was written down once and agreed; everything the framing did not name was decided
seven times and disagrees.**

ADR-0032's central claim delivered. `ulo::rpc::wire` owns `frame_response`, `parse_reply_frame`,
`drive_reply_stream`, `frame_cancel` and `Inflight`, every transport calls them, and the stream
grammar is identical on the wire across all seven. A stream arrives in order and ends on all five
brokers, and abandoning it cancels the producer on all five — probed, not assumed.

What the grammar does not name is everything around the frame, and each of those is a private
decision per adapter:

- **what a payload is.** `RpcData` has three variants. One of them survives the trip on one
  transport of seven. `Text("hello")` reaches the handler as `Binary` on four, as `Json` on two and
  as `Text` on one; a `Binary` payload reaches it as `Json(null)` on tcp and udp, which is the
  payload discarded with no word to the caller (§1, F216, F217).
- **who gets the message.** One `emit`, two server instances, one broker: the handler runs **twice**
  on nats, redis and mqtt and **once** on rabbitmq and kafka. Nothing in a declaration says which,
  and only Kafka exposes the knob that decides it (§2, F218).
- **what a miss is.** A pattern nothing handles answers `Remote { status: "NotFound" }` on tcp and
  udp, `Transport("no responders")` on nats, and `Timeout` on redis, mqtt, rabbitmq and kafka. The
  `Unrouted` event and the `#[catch(Unrouted)]` handler the framework documents for this cannot fire
  at all on redis, rabbitmq or kafka (§3, F219).
- **what a ceiling is.** The one transport that owns its own framing has none: a tcp connection sent
  64 MiB on a single unterminated line and the adapter buffered all of it without complaint (§4,
  F220).
- **what shutdown owes.** tcp and udp drain in-flight calls under a configurable deadline. The five
  brokers close the connection and abandon them (§5, F221).

Underneath those sits one seam the audit did not expect to find open. **The cancel notice ADR-0032
built is sent for streams and never for a single call** (§6, F222) — so the common case, a caller
that timed out, leaves the server running work nobody will read, on every transport.

And underneath *that*, the copies. `forward_stream` is written seven times; tcp's and udp's are
identical by the repository's own measurement rule, and the four broker copies sit 15–23 units apart
(§12, F228). `data_to_bytes` and `bytes_to_data` are one function written five times, three of them
byte-identical.

§1 to §6 are the spine. §7 to §11 are independent and can be read in any order. §12 is the copies,
§13 is what the suite covers, §14 the decisions.

## 1. Three payload variants, one that survives — F216, F217

**Probed.** One handler names the `RpcData` variant it was handed; one client sends five payloads
over each of the seven transports.

| sent by the caller | tcp | udp | nats | redis | mqtt | rabbitmq | kafka |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `Text("hello")` | `Json("hello")` | `Json("hello")` | `Binary` | **`Text`** | `Binary` | `Binary` | `Binary` |
| `Text("{\"a\":1}")` | `Json("{\"a\":1}")` | `Json("{\"a\":1}")` | `Json({a:1})` | **`Text`** | `Json({a:1})` | `Json({a:1})` | `Json({a:1})` |
| `Binary(b"{\"a\":1}")` | **`Json(null)`** | **`Json(null)`** | `Json({a:1})` | **`Binary`** | `Json({a:1})` | `Json({a:1})` | `Json({a:1})` |
| `Binary([0xFF,0xFE])` | **`Json(null)`** | **`Json(null)`** | `Binary` | **`Binary`** | `Binary` | `Binary` | `Binary` |
| `Json("hello")` | `Json("hello")` | `Json("hello")` | `Json("hello")` | `Json("hello")` | `Json("hello")` | `Json("hello")` | `Json("hello")` |

**`RpcData::Json` is the only variant that means the same thing on every transport.** The other two
are transport-dependent, and a handler that matches on the variant — which `RpcData::as_text`,
`as_binary` and a `match` all invite — reads a different arm depending on which adapter is mounted.

Three mechanisms produce the three columns.

Four brokers put the payload on the wire as bare bytes and read it back by trying JSON first:
`bytes_to_data` returns `Json` when the bytes parse and `Binary` when they do not
(`ulo-rpc-rabbitmq/src/wire.rs:28-33`, `ulo-rpc-mqtt/src/wire.rs:28-33`,
`ulo-rpc-kafka/src/wire.rs:63-68`, and the same match inlined at
`ulo-rpc-nats/src/nats_adapter.rs:194-197`). `Text` has no wire representation there, so it is
unreachable as an inbound variant on four of seven transports.

Redis carries the externally-tagged `RpcData` inside a JSON `RequestEnvelope`
(`ulo-rpc-redis/src/wire.rs:29-36`), which is lossless. Its own module doc explains the choice —
*"the envelope is never meant to be hand-typed against `redis-cli`"* — so the one transport that
preserves the type did so as a local decision about its own wire, not as a framework rule.

tcp and udp put the payload in a JSON field and have nowhere to put bytes, so `data_to_json` maps
`Binary` to `Value::Null` (`ulo-rpc-tcp/src/tcp_client_transport.rs:267-274`,
`ulo-rpc-udp/src/udp_client_transport.rs:292-299`). **That is the finding in F217 rather than
F216**: the call succeeds, the handler runs, and it runs on `null`. A comment says binary is not
supported; nothing on the wire or in the `Result` says so.

## 2. One emit, one broker, two instances — F218

**Probed.** Two applications on one broker, both declaring the same `#[event_pattern]`, one `emit`
from one client.

| transport | handler runs | why |
| --- | --- | --- |
| nats | **2** | `client.subscribe(pattern)` with no queue group (`nats_adapter.rs:157`) |
| redis | **2** | Redis Pub/Sub fans out to every subscriber (`redis_adapter.rs:83`) |
| mqtt | **2** | a plain topic filter, not a shared subscription (`mqtt_adapter.rs:92`) |
| rabbitmq | **1** | two consumers on one queue compete (`rabbitmq_adapter.rs:176`) |
| kafka | **1** | one consumer group, one partition per topic (`kafka_adapter.rs:105`) |

**The same declaration means "every instance runs this" on three transports and "one instance runs
this" on two.** For an `#[event_pattern]` that writes to a database, that is the difference between
one write and N; for a `#[message_pattern]`, it is N handlers racing to answer one caller, which the
caller cannot see because it takes the first reply.

Each broker's own protocol has the mechanism to choose. NATS has queue groups, MQTT v5 has Shared
Subscriptions — the spec names this exact case in §4.10.1, *"The Responder can use a Shared
Subscription to allow for a pool of responding Clients"* — AMQP has competing consumers by
construction, and Kafka has the consumer group. `grep` across `crates/` finds **zero** occurrences
of `queue_group`, `queue_subscribe` or `$share`. `KafkaAdapter::with_group_id` is the only knob any
of the five exposes, and it is the one transport that already load-balances.

## 3. What a caller sees when nothing handles the pattern — F219

**Probed.** One `send` to a pattern no controller declares.

| transport | what the caller gets |
| --- | --- |
| tcp | `Remote { message: "Pattern not found: …", status: "NotFound" }` |
| udp | `Remote { message: "Pattern not found: …", status: "NotFound" }` |
| nats | `Transport("no responders: no responders")` |
| redis | `Timeout` |
| mqtt | `Timeout` |
| rabbitmq | `Timeout` |
| kafka | `Timeout` |

Three answers for one condition, and two of them are indistinguishable from a healthy server that is
slow.

**Read** for the cause, which is sharper than the table. The subscription list *is* the routing
table on every broker: `register_handlers` is handed `all_patterns` and the adapter subscribes
exactly those (`application.rs:661-678`), so a message that arrives always has a pattern in
`make_rpc_callbacks`'s map. The `Unrouted` event at `application.rs:1044` and the
`#[catch(Unrouted)]` handler `CLAUDE.md` documents for all four transports are therefore
**unreachable on redis, rabbitmq and kafka**, because those three subscribe exact names only. They
are reachable on nats and mqtt through one route: a pattern containing that broker's wildcard
subscribes successfully and then fails the exact-match lookup — which is the same mechanism
`CLAUDE.md` already records as *"wildcards subscribe but never route"*, seen from the error side.
They are reachable on every call on tcp and udp, which read the pattern out of the envelope.

So a framework event documented as transport-agnostic fires on two transports always, on two
transports only via a spelling the framework calls a mistake, and on three transports never.

## 4. The transport that owns its framing has no frame — F220

**Probed.** A raw socket wrote 67 108 864 bytes to a `TcpAdapter` with no `\n` anywhere in them. The
adapter accepted every byte and asked for more; the probe stopped because it ran out of iterations,
not because the server pushed back.

`handle_connection` reads with `reader.read_line(&mut line)` into a `String`
(`ulo-rpc-tcp/src/tcp_adapter.rs:287`), and `read_line` grows that `String` until it finds a
terminator or the peer goes away. One connection with no terminator is bounded by process memory,
and nothing caps the number of connections either: `with_max_inflight` bounds handler tasks, which
is downstream of the read.

Every other transport has a ceiling it did not choose but does have. UDP is bounded by the datagram
(`MAX_DATAGRAM = 65_507`, `udp_adapter.rs:15`), and the four brokers are bounded by their broker's
own maximum message size. **tcp is the only transport whose framing ulo wrote, and the only one with
no bound at all** — the same shape the HTTP audit found in actix's 256 KiB cap and rocket's 32 MiB,
except that there the number existed and here it does not.

The client half reads the same way (`tcp_client_transport.rs:136`), so a hostile or broken server
can do this to a caller.

## 5. What shutdown owes — F221

**Read.** `TcpAdapter` and `UdpAdapter` track every spawned task in a `JoinSet` and `drain_tasks`
awaits it under `with_drain_timeout`, defaulting to 10 s, aborting what remains
(`tcp_adapter.rs:235-260`, `udp_adapter.rs` same function, 0 differing lines). The five brokers have
no equivalent: the shutdown callback flips a watch channel, the serve loop breaks, and the per-message
tasks it spawned are detached (`redis_adapter.rs:98`, `mqtt_adapter.rs:80-82`,
`rabbitmq_adapter.rs:200-201`, `kafka_adapter.rs:157`). NATS is a step further back — its shutdown
callback is `|| async { Ok(()) }` (`nats_adapter.rs:272-274`), so there is no shutdown signal at all
and the serve future ends only when the process does.

What a caller sees: a handler that had produced its answer but had not yet published it loses the
reply, and the caller waits out its timeout. On rabbitmq the delivery is already acked
(`rabbitmq_adapter.rs:282`, before the publish), so the request is gone too.

`with_drain_timeout` and `with_max_inflight` are documented on tcp and udp and exist nowhere else;
`CLAUDE.md` records that accurately per transport, but nothing records it as a decision.

## 6. The cancel notice is for streams, and a timeout is not a stream — F222

**Read, with the tcp half probed.** ADR-0032 built the abandonment signal: `RpcReplyStream::Drop`
before the end runs `on_cancel`, which publishes `{"cancel":true,"key":…}` to the broker's cancel
channel or writes `{"id":…,"cancel":true}` in band. It is wired on all seven and it works — the
conformance suite's cancel case passes on all five brokers.

It is never sent for a single-reply call. Every transport's `send` ends its timeout branch by
removing the pending entry and returning `RpcClientError::Timeout`
(`tcp_client_transport.rs:327-330`, `mqtt_client_transport.rs:235-239`,
`redis_client_transport.rs:229-232`, `rabbitmq_client_transport.rs:231-234`,
`kafka_client_transport.rs:226-229`; NATS relies on `async_nats`'s own request and never registers
one at all). The server has registered the call in `Inflight` and holds an abort handle for it
(`nats_adapter.rs:182`, and the equivalent in each of the other six). Nothing ever calls it.

The same gap has a second half: **the caller's timeout does not travel.** `with_timeout` is a
client-side `tokio::time::timeout` on all seven, `RpcContext::deadline()` returns `None` by
construction, and the test at `rpc/context.rs:103-107` pins that as intended — *"The wire carries no
deadline on this transport"*. That is true of the wire and not of the call: the caller set a budget,
the grammar has a field-free frame that could carry it, and gRPC on the same framework reads
`grpc-timeout` and reports it.

The consequence is one sentence: **on the most common failure in an RPC system — the caller gave
up — the server learns nothing and keeps working.**

## 7. Three statuses that are not kinds — F223, correcting F63

**Read.** F63 records `overloaded` as *"the one status that is not an `ErrorKind` name"*. There are
three.

| status | where | reached when |
| --- | --- | --- |
| `overloaded` | `tcp_adapter.rs:347`, `udp_adapter.rs:382` | `with_max_inflight` sheds a call |
| `unsupported` | `ulo/src/rpc/wire.rs:82` | a handler answers `Items::Many` on a transport with no stream grammar |
| `error` | `udp_adapter.rs:450` | a stream item will not fit in a datagram |

The third is inside a frame the second-order reader will not expect: `{"end":true,"err":{"status":
"error"}}`, where every other error end carries an `ErrorKind` name. F63 is deferred to the user on
its merits; what this adds is that a decision about `overloaded` has to be made about three words,
not one, and that `error` in particular is a status that means nothing.

**The neighbouring defect, same site.** The oversize *stream* item ends the stream loudly
(`udp_adapter.rs:440-455`). The oversize *single reply* is dropped and logged
(`udp_adapter.rs:469-476`), so the caller waits out its timeout while the server knows exactly what
happened. Two branches of one function, two policies.

## 8. A duplicate pattern is accepted, and the second one wins — F226

**Probed.** Two controllers in one module, both declaring `#[message_pattern("dupe.pattern")]`.
`create` succeeded, `bind` succeeded, and the call answered `"B"` — the second controller.

`all_patterns` is a `Vec<String>` built by `flat_map` over the controllers with no check
(`application.rs:661-666`), and `make_rpc_callbacks` builds its index with
`pattern_map.insert(pattern, wrapper)` (`application.rs:1022-1026`), so the later insert replaces
the earlier. On nats the list with its duplicate also reaches `register_handlers`, which subscribes
per entry (`nats_adapter.rs:152-157`) — **two subscriptions to one subject, so every message is
delivered and dispatched twice.**

HTTP refuses this. Two routes that agree up to trailing slashes collapse to a bind error (ADR-0011).
The RPC side of the same framework takes the last one and reports nothing, which is F46's
string-match dispatch producing a second consequence.

## 9. No broker transport has a ceiling of any kind — F224

**Read.** `with_max_inflight` exists on `ulo-rpc-tcp`, `ulo-rpc-udp`, `ulo-grpc` and the HTTP adapter
SPI, and on none of the five brokers, each of which spawns one unbounded task per inbound message.

RabbitMQ is the sharp case because AMQP's own flow control is sitting unused: `grep -rn basic_qos
crates/` returns **0**. Without a prefetch the broker pushes every message in the queue at once, so
a queue that built up while the service was down becomes N concurrent handler tasks the moment it
comes back. Kafka's `max.poll.records` and MQTT's receive-maximum are the same lever on those two
and are equally unset.

## 10. What the client's lifecycle API promises — F225

**Read.** `RpcClient::close` is documented as *"Flushes any pending messages before closing. Call
this in an `#[on_application_shutdown]` hook to ensure in-flight data is not lost before the process
exits"* (`rpc_client.rs:198-205`). `grep -n "async fn close" crates/ulo-rpc-*/src/*client*.rs`
returns **one** override, NATS's. The other six inherit the trait's no-op. MQTT's is the one where
this is load-bearing: `AsyncClient::publish` hands the packet to a 64-slot channel that the event
loop drains, so `emit` returning `Ok` means queued, not sent, and `close` does nothing to flush it.

Beside it, `RpcClientTransport::connect`'s doc says it is *"Called automatically by `RpcClient` at
application bootstrap so that connection failures surface at startup rather than on the first
request"* (`client_transport.rs:24-30`). Nothing in core calls it — `grep` for `.connect()` across
`crates/ulo/src` outside the two client files returns nothing, and `RpcClient::connect`'s own doc
two hundred lines away says the opposite, *"Call this explicitly"*. One of the two is wrong and it
is the one on the SPI, which is the one an integration author reads.

## 11. The four conventions, against their own documents

This is the brief's explicit question, and the answer is the same for all four: **ulo follows each
convention where the convention exists, and every gap found is on an axis the convention does not
cover.**

**AMQP (Spec).** The 0-9-1 class definition marks `correlation-id` and `reply-to` *"For application
use, no formal behaviour"*. There is no conformance to fail. ulo uses RabbitMQ's direct reply-to
correctly: the no-ack consumer on `amq.rabbitmq.reply-to` is established before any request names it
(`rabbitmq_client_transport.rs:103-114`), which is the one ordering RabbitMQ's own documentation
requires. The gaps are §9's missing prefetch and §5's missing drain.

**MQTT v5 (Spec).** §3.3.2.3.5 *"The presence of a Response Topic identifies the Message as a
Request"* and §4.10.1 *"the Responder copies this property into the Response Message … The Response
Message does not include a Response Topic property."* ulo does exactly this — the reply carries the
correlation data and no response topic (`mqtt_adapter.rs:243-250`), and `emit` sets no response
topic at all. `[MQTT-3.3.2-14]` forbids wildcards in a Response Topic and ulo's is
`ulo/rpc/reply/{client_id}`, which contains none. Conformant. One lossy edge: §3.3.2.3.7 says User
Properties may repeat a name and `[MQTT-3.3.2-18]` requires the server to preserve their order;
`user_properties_to_metadata` collects them into a `HashMap` (`mqtt/src/wire.rs:38-40`), so
duplicates collapse and order is gone. The module doc says so. The same flattening is in every
transport, and **the three brokers whose header model allows repeats resolve them differently** —
NATS keeps the *first* value of a multi-valued header (`nats_adapter.rs:201-205`), MQTT and Kafka
keep the *last*.

**NATS (Read).** Request-reply is the reply subject on the message, and ulo reads `msg.reply` to
decide request versus event (`nats_adapter.rs:168, 212-219`) and uses `client.request` on the client
side. Conformant. The gap is §2's missing queue group.

**Kafka (Read).** There is no Kafka request-reply convention to conform to; headers are KIP-82 and
`ulo-reply-to` / `ulo-correlation-id` are ulo's own names, stripped from user metadata inbound
(`kafka/src/wire.rs:113-131`). Category (3) under `project-the-spec-conformance-seam`, correctly.
The gaps are §2's and F227's.

**What this says about the seam.** All four conventions cover the same narrow thing — how a reply
finds its way back — and none of them covers delivery cardinality, flow control, resource lifecycle
or what a miss looks like. Those are category (2) questions under
`project-the-spec-conformance-seam`: the protocol leaves them open, the framework must choose, and
the framework has not. That is the same failure that audit named for HTTP, on a different protocol
family: *a category-(2) question is invisible precisely because nothing refuses it.*

## 12. The copies — F228

Measured with `AUDIT3_DISPATCH.md` §4's method: strip comments, normalise whitespace, substitute the
transport noun, count differing lines. §4's collapse band is 0–6 units and its split line is 35–45.

**`forward_stream`**, the client-side loop that turns reply frames into the caller's stream and
publishes the cancel notice, exists in all seven transports:

| pair | units apart |
| --- | --- |
| tcp / udp | **0** |
| kafka / mqtt | 15 |
| mqtt / redis | 17 |
| mqtt / rabbitmq | 18 |
| kafka / redis | 20 |
| kafka / rabbitmq | 21 |
| rabbitmq / redis | 23 |
| nats / anything | 47–62 |

tcp's and udp's copies are the same function written twice. The four broker copies cluster at 15–23,
between §4's two lines, and what differs is the channel type and one publish call. NATS is genuinely
different — it forwards from a subscription rather than a channel — so the seam there is real.

**`data_to_bytes` and `bytes_to_data`** are byte-identical across rabbitmq, mqtt and kafka after
whitespace normalisation — **0 differing lines for both functions in all three pairs** — with a
fourth copy of the decode inlined in `nats_adapter.rs:194-197` and a fifth in
`redis_adapter.rs:257-260`. This is the same function that §1 shows deciding what a payload *is*, so
collapsing it and deciding the question are one change rather than two.

**`format!("{pid}-{nanos}")`**, the process-unique instance id, appears seven times under four names
(`make_client_id`, `client_id`, `instance_id`, and two mqtt variants) across five crates.

The `wire.rs` files as wholes are still far apart — 41 to 146 units pairwise — so §4's original
verdict that *"that seam is genuine and already drawn correctly"* still holds at file level. It is
the functions inside them that are copies, and they are the ones ADR-0032 did not reach.

## 13. What the suite is evidence of — F233

The crate doc opens *"The contract every RPC transport satisfies, written once"* and *"Seven
transports speak ulo's wire grammar"*. The suite runs against five. `ulo-rpc-tcp` and `ulo-rpc-udp`
have no `Broker` implementation, and they are the two that need no container at all — `start()`
would bind a port.

The six cases cover: a request round-trips, an emit reaches its handler, a header survives, a stream
arrives in order and ends, abandoning a stream cancels the producer, traffic resumes after a
disruption. **Every finding in this memo is outside them.** No case asserts anything about an error
frame, a payload variant, delivery cardinality, a size ceiling, shutdown, or what a miss looks like —
and §1, §2 and §3's tables were produced by three cases written in the same file, in the same shape,
stamped by the same macro. The suite is not the wrong instrument; it is a nearly-empty one.

`#[cfg(feature = "integration")]` on the five test files means `cargo test -p ulo-rpc-nats --test
conformance` reports `ok. 0 passed` having compiled nothing, and CI's one job that compiles the
suite runs no case. That is the brief's own observation and it is unchanged: **nothing in CI runs
this, and now that the cases pass, nothing will notice when they stop.**

## 14. Decisions

**All eight are settled, answered by the user on 2026-09-23.** D7 was raised by the user on reading
the out-of-scope list and is not a question this audit asked. A recommendation kept beside a
settled decision is its argument, not a choice still open. D4's *number* is the one thing chosen
here rather than by the user, and it is one constant. D8 was raised inside F229, never promoted, and nearly built past;
it was promoted and then answered against this memo's own first recommendation, by a probe.

### D1 — does `RpcData` mean one thing on the wire, or does the transport decide? **Settled**

§1 is the memo's largest finding and the one that cannot be fixed without a call.

- **(a) One envelope everywhere.** Every transport carries the externally-tagged `RpcData` the way
  redis already does. All three variants survive on all seven, `data_to_bytes`/`bytes_to_data`
  collapse into core, and F217 disappears because `Binary` has somewhere to go on tcp and udp. Cost:
  the payload on nats, mqtt, rabbitmq and kafka stops being the raw bytes a foreign publisher
  writes, so `nats pub order.create '{"item":"x"}'` — which the `NatsAdapter` doc offers as the way
  to test — stops working. That doc promise is the real price.
- **(b) Keep the raw-bytes wire, and narrow the type.** `RpcData` drops to `Json` and `Binary`,
  `Text` becomes a constructor that produces `Json(Value::String)`, and the matrix has two rows
  instead of five. Foreign publishers keep working. Cost: `Text` is a public variant today and a
  handler matching on it stops compiling — cheap pre-release, and D13 says delete rather than
  deprecate.
- **(c) Envelope where a caller asks for it.** A per-call flag chooses. Cost: two wire formats per
  transport, which is the thing §12 says already went wrong once.

**Settled 2026-09-23 as (b) plus a mechanism the options above did not carry: (d), a capability
declaration.** The user's framing was *"the transports say what they can carry, unified where it
can"*, which is better than (b) alone and is not a substitute for it.

**Why both, and not (d) alone.** A capability decides what a transport may be *asked* to carry. It
cannot recover what the wire did not record: on the four raw-byte brokers an inbound payload carries
no tag, so `Text` is unrecoverable however well the SPI is modelled. The type question — is there a
tag on the wire — stays, and the answer is no.

**(b), stated exactly.** `RpcData` narrows to `Json | Binary`; `RpcData::text` becomes a constructor
producing `Json(Value::String)`. Inbound then has one total rule on every transport: the bytes parse
as JSON, or they do not.

**The detail that makes (b) uniform, which the options above missed.** Redis has to stop tagging.
Its envelope stays — Pub/Sub has no reply address, so `reply_to` needs it — but `data` becomes a
bare value under the same rule. Left as it is, redis keeps answering `Binary` where nats answers
`Json` for identical bytes and the matrix is still not uniform. One field.

**(d), stated exactly.** `RpcCapabilities`, read at `bind()`, declaring payload support, maximum
frame, delivery modes, pattern-name rules and streaming support. It subsumes F217's refusal, D2's
redis refusal, D4 entirely and F231's validation into one mechanism rather than four bespoke ones.
§15 carries its shape on both SPIs.

**It is the same shape the HTTP build introduces**, so it is co-designed with `AdapterCapabilities`
rather than invented twice — see `HTTP_AUDIT.md` §1 and the WebSocket build's D8, which is the same
move on a third transport. That changes the sequencing: Phase 2 of this build is no longer
independent of the HTTP build.

**F217 remains independent and does not wait.** Whatever `RpcData` ends up meaning, sending a value
the transport cannot carry has to fail rather than succeed on a substitute.

### D2 — is an RPC pattern broadcast or load-balanced, and who says so? **Settled**

§2. Three options, and the first is the only one that is not a choice:

- **(a) Leave it transport-defined and document it.** Cost: the same code means different things on
  different transports, which is what a framework exists to prevent, and nothing in a declaration
  reveals it.
- **(b) One knob, uniform default.** `#[patterns(delivery = "broadcast" | "balanced")]`, or an
  adapter-level default with a per-pattern override. NATS gets a queue group, MQTT gets
  `$share/{group}/`, redis gets — nothing, because Pub/Sub has no competing-consumer mode, so redis
  would have to refuse `balanced` at startup. rabbitmq and kafka already do `balanced` and would
  need a broadcast mode (a per-instance exclusive queue bound to a fanout; a per-instance consumer
  group), which both protocols support and neither does today.
- **(c) Pick one meaning and make every transport meet it.** Cleanest contract, largest build, and
  redis can only meet `balanced` by stopping being Pub/Sub.

**Settled 2026-09-23 as (b), in its adapter-level-default-with-a-per-pattern-override form**, with
`balanced` the default for `#[message_pattern]` and `broadcast` for `#[event_pattern]`. Under D1's
(d) the redis refusal is a capability mismatch reported at `bind()` rather than special-case code.
The reasoning that carried it: A request-reply pattern answered by N instances is a defect on
any reading; an event fanned to N instances is a legitimate design and is what three transports do
today. Splitting the default by attribute matches what the two attributes already mean. The redis
refusal is a feature: it says out loud which transport cannot do which thing.

### D3 — does a caller's abandonment reach the server? **Settled**

§6. The machinery exists and is unused for single calls.

- **(a) Send the cancel notice on a `send` timeout too.** One change per client transport, reusing
  `frame_cancel` and the registry already keyed by the same correlation. Cost: one extra publish per
  timed-out call, on a path that is already failing.
- **(b) Carry the deadline on the wire** — a reserved header, `RpcContext::deadline()` reading it,
  the handler's token firing when it passes. Cost: a field in the grammar on seven transports, and a
  decision about clock skew that gRPC avoids by sending a duration rather than an instant.
- **(c) Both.** (a) handles a caller that walked away; (b) handles a server that should never have
  started.

**Settled 2026-09-23 as (a) now, (b) filed with a trigger.**

**Where it folds, which is not where it first appears to.** This lives in seven `send`
implementations, all of them client-side. The server-side pipeline — guards, interceptors, the error
chain — never sees it. So it folds into the `RpcClientTransport` collapse (§15), not into pipeline
work, and done there it is one timeout branch in one place rather than seven. The deadline half does
not fold and keeps its trigger. (a) is small, uses only what ADR-0032 already
built, and closes the case where the server burns work for a caller that is gone. (b) is a grammar
change and should wait for a second reason to want it.

### D4 — what is a tcp frame's maximum size? **Settled**

§4. This is a category-(2) question with no answer at all today, which is the worst of the three
states.

- **(a) A default cap with a builder override**, e.g. 1 MiB and `TcpAdapter::with_max_frame`,
  refusing an over-long line with an error frame and closing the connection. Matches what actix and
  rocket do for HTTP bodies, and what `MAX_DATAGRAM` already is for udp.
- **(b) A cap with no knob.** Simpler; wrong the first time someone sends a large payload.

**Settled 2026-09-23 as (a), and it folds into D1's (d): `max_frame` is a declared capability
rather than a bespoke builder.** tcp declares a default and `TcpAdapter::with_max_frame` overrides
it; every other transport declares the ceiling it already inherits, so the number stops being
invisible on all seven rather than on one.

**The default is 1 MiB, approved by the user on 2026-09-23.** The reasoning: udp's hard ceiling is 65 507 bytes and tcp is the transport a caller picks
precisely when a payload will not fit in a datagram, so defaulting to the datagram size defeats the
reason for choosing it; 1 MiB is sixteen times that and still bounds one connection to something a
server can absorb thousands of. actix's 256 KiB and rocket's 32 MiB bracket the range and both are
defensible. **It is one constant, and changing it stays a one-line decision.**

### D5 — do the five brokers drain on shutdown? **Settled**

§5. tcp and udp already have `with_drain_timeout`; the five brokers have no tracking to drain.

- **(a) Give the brokers the same `JoinSet` + `drain_tasks` tcp and udp have.** The function is
  already written twice identically (§12), so this is one shared helper and five call sites.
- **(b) Leave it, and document that a broker transport does not drain.** Cost: a reply produced but
  not published is lost on shutdown, on the four transports whose whole point is not losing
  messages.

**Settled 2026-09-23 as (a)**, which is what "doing it properly" means here: `drain_tasks` is
already written twice with **0** differing lines, so giving it to the five is the collapse rather
than a separate fix. NATS additionally needs a shutdown signal, having none today.

### D6 — does the suite become the gate, and what runs it? **Settled**

§13. The suite passes and nothing runs it.

- **(a) A CI job with the five brokers under Docker,** on merge rather than on every push.
- **(b) A local `make` target and a line in `CONTRIBUTING`,** run by hand before a release.
- **(c) Leave it.** Cost: the six cases now pass, so the next regression is invisible, and this memo
  will have been the only time they ran.

**Settled 2026-09-23 as (a) for nats, redis, mqtt and rabbitmq — 64 s for the four together — and (b) for
kafka.** Kafka is nine minutes of CI for one transport, and its budgets (`boot: 90s`,
`recovery: 120s`) say that is inherent rather than incidental.

**Separately and not a decision: tcp and udp should implement `Broker`.** They need no container,
the suite claims them in its own doc, and it costs two `start()` implementations that bind a port.

### D7 — does request-side streaming ride with this build? **Settled**

Raised by the user on reading the out-of-scope list: `project-rpc-request-side-streaming` records
that RPC streams replies and not requests, and nothing in this audit changes that reading, so the
question is whether the two missing call shapes belong here.

**Settled 2026-09-23: the grammar and the SPI ride; the handler surface does not.**

The argument is stronger than adjacency. **Request-side streaming is the request half of ADR-0049.**
That ADR made the *answer* an envelope and a cardinality — `Items::{Empty, One, Many}`. The request
is the same move on the other direction, and `RpcMessageCallbacks::message` takes exactly one
`RpcData` today (`crates/ulo/src/rpc/adapter.rs:45-47`). Settling the SPI without it settles it
twice, which is the mistake the WebSocket pass made when it costed a new adapter SPI for a seam the
HTTP build already builds.

What rides:

- `RpcMessageCallbacks` takes `Items<RpcData, RpcError>` rather than `RpcData`. Every adapter passes
  `Items::One(data)` on the day it merges, so nothing changes on the wire and no call shape appears.
- The inbound frame grammar is reserved in `ulo::rpc::wire` beside the reply grammar it mirrors,
  rather than invented later against seven collapsed carriers.
- `RpcCapabilities` carries `streaming`, and a transport that cannot order inbound frames says so.
  **UDP is the one that cannot** — a datagram transport has no frame order to offer — and it
  declares that rather than pretending.

What does not ride *in PR 13*: the two call shapes, the handler-side extractor (a `Payload<T>`
counterpart holding an inbound stream, which is ADR-0043 territory and a macro-surface decision),
and the per-transport request correlation. **Those are the build's Phase 3** — four PRs, in the
sequence rather than filed beside it, by the user's call on 2026-09-23, and not started until
Phase 2 has landed. Phase 3 is its own approval: it adds a feature where Phases 1 and 2 close
defects.

**The risk this closes.** D1 decides what a payload is. A streamed request is a sequence of
payloads. Narrowing `RpcData` without the inbound direction in view is how a type gets narrowed
wrongly and re-widened a release later.

### D8 — what bounds a reply stream on the client? **Settled, against the recommendation this memo first made**

**Probed 2026-09-23** at `654c7183`, tcp, in-process. A handler streams 1 KiB items as fast as
anything downstream will take; one case reads an item every 100 ms, the control reads flat out. The
gap between items produced and items pulled is what is buffered.

| | t=1s | t=2s | t=3s | t=4s | RSS |
| --- | --- | --- | --- | --- | --- |
| control, consumer reads flat out | gap **2** | **2** | **2** | **2** | 16 → 17 MiB |
| consumer reads 1 item / 100 ms | gap **36 312** | **72 613** | **108 817** | **145 111** | 18 → **176 MiB** |

Two readings, and the second is the one that decides this.

**The gap is linear**, so nothing bounds it. **And the producer's rate is identical in both rows** —
about 36 000 items/s either way. This is not weak backpressure. It is **none**: the handler runs at
full speed whether the caller is reading or not, and ~38 MiB/s accumulates for as long as it does.
The control's flat gap of 2 is what says the harness measures buffering rather than production
speed.

### What that does to the three options

**Shedding is a timeout in disguise, and this memo recommended it.** A high-water mark buys
`threshold ÷ rate` seconds of consumer slowness. At 38 MiB/s, a 64 MiB mark buys 1.7 seconds and a
1 GiB mark buys 27. **There is no number that is both safe for memory and generous to a consumer**,
because they are the same number divided by a rate the framework does not set. A streaming API in
which a consumer that pauses loses its stream is not a streaming API, and a slow consumer is the
reason streaming exists.

**And the shed as first drafted would not even have told the server.** `RpcReplyStream::poll_next`
sets `ended = true` on `Ready(Some(Err(_)))` and `Drop` fires `on_cancel` only `if !self.ended`
(`rpc_reply_stream.rs:47-86`), pinned by `an_err_item_is_terminal`. Delivering an overrun as an
`Err` marks the stream ended and suppresses the cancel notice, so the producer would have run on —
F222's leak reached from the opposite direction. It is not a live defect today, because each
transport's gap-timeout path publishes the cancel itself before sending its `Err`; it is a trap laid
for whoever writes the shed path.

**Backpressure to the read loop is correct for one call and broken across calls.** Bounding the
queue makes the reader stop, the socket fill and the producer block — right for that call, and it
stalls every other call on the connection, because one reader serves them all. That is HTTP/1.1
pipelining's head-of-line problem, and it is why HTTP/2 has per-stream windows instead of one.

**Leaving it unbounded understates what happened.** MQTT's client is `AsyncClient::new(opts, 64)`
and tcp and udp are bounded by the socket. `PendingSlot::Stream(mpsc::UnboundedSender<…>)` sits in
front of those. ulo took a bounded pipeline and made it unbounded; that is an accident, not a
policy.

### The answer: per-call credit

**Settled 2026-09-23: credit-based flow control, scheduled rather than filed with a trigger.** The
user's call, and the measurement is what makes it the only correct option — the other two each fail
on the case the probe shows is the normal one.

The client tells the server how many items it is willing to receive; `drive_reply_stream` waits for
credit before sending; `forward_stream` issues more as the caller consumes. Memory is bounded by
credit × item size, **per call**, so no other call is stalled, nothing is shed, and no number stands
in for a timeout.

**This memo overstated its cost when it first called it a road not taken.** It is two places in core
— `drive_reply_stream` on the server, `forward_stream` on the client, both of which PRs 11 and 12
collapse to one copy each — plus one control frame on **the carrier the cancel notice already
uses**. Every transport already carries a client→server control message for a specific in-flight
call. Credit is a second message type on a channel that exists.

**The one genuine complication, and it is per transport.** The brokers' cancel carrier is a shared
best-effort fanout: *"a notice racing this registration is dropped"* is written in five adapters.
Cancel can be best-effort because a missed cancel wastes work; **credit cannot, because a missed
credit frame stalls the stream forever.** So credit needs a reliable per-call path, and whether one
is cheap differs:

- **tcp, udp** — in-band on the same connection or datagram flow, ordered and reliable. Free.
- **nats, redis, mqtt** — subject, channel and topic namespaces are free, so a per-call control
  address costs nothing. **Unverified**: reasoned from each broker's addressing model, not probed.
- **kafka, rabbitmq** — a per-call topic or queue is a durable object, which is F227's disease. These
  two declare through `RpcCapabilities` that they cannot carry credit, and fall back to a bounded
  queue with shed-and-cancel as a **safety valve** — which is what shedding is good for, rather than
  as the design.

That declaration is `RpcCapabilities`, which PR 9 builds regardless. The fallback is not papering
over the answer: it is the capability mechanism applied to the one question that genuinely differs
per transport, with the two transports that take the worse behaviour named out loud.

**Phase 3 wants this too.** A client streaming a *request* needs the server to bound it, which is
the same mechanism in the other direction — so credit is built before the call shapes, and they reuse
it rather than inventing a second answer.


## 15. What both SPIs look like after the collapse

D1, D3 and D5 all reduce to one move, and the move has to be made on **two** SPIs rather than one.
`RpcAdapter` is the server path. `RpcClientTransport` is the client path, and it carries more of the
duplication and all of D3. "Pipeline" in this workspace has meant the server-side guard, interceptor
and error-chain run, which is a third thing and is not what either of these is.

### The measurement, including the part that argues against a naive collapse

§12 measured the pieces. One more measurement, taken for this section: the per-message dispatch
block — `handle_message` on redis and kafka, `handle_publish` on mqtt, `handle_delivery` on rabbitmq
— is **43 to 77 units apart** pairwise under §4's rule. §4's split line is 35-45. **So those four
functions are at or past the line for staying separate, and reading them as copies would be wrong.**

What is identical in them is not the lines, it is the sequence: catch the panic, decide whether a
reply is owed, match the outcome, drive a stream or frame a single reply, publish. Every one of the
seven runs that sequence and `catch_unwind` appears once in each of the seven adapter files. The
43-77 units are the publishing, which is genuinely per-broker.

**That is the design, stated by the measurement rather than by preference: core owns the sequence,
the adapter owns the publish.** A collapse that tried to merge the four `handle_*` functions
wholesale would be fighting the numbers; one that lifts the sequence out and leaves a `publish`
behind is what the numbers describe.

### Server: `RpcAdapter` becomes a carrier plus a declaration

```rust
pub trait RpcAdapter: Send + Sync + 'static {
    /// Read once at `bind()`. What this transport can carry, how big, which delivery
    /// modes it can honour, what a legal pattern name is. D1's (d).
    fn capabilities(&self) -> RpcCapabilities;

    fn register_handlers(&mut self, patterns: &[String], callbacks: Arc<RpcMessageCallbacks>)
        -> AdapterResult;

    async fn into_lifecycle(self: Box<Self>) -> AdapterResult<RpcLifecycleHandle>;
}
```

What moves into core, each closing a finding rather than tidying: the payload conversion (F216), the
inbound refusal (F217), the `{pid}-{nanos}` helper and `drain_tasks` (F221, F228), the concurrency
bound (F224), the duplicate-pattern check and the pattern-name validation (F226, F231), and the
`catch_unwind`-and-frame sequence the seven adapters each write.

What the adapter keeps, and should: acquiring its connection, subscribing, putting bytes on its own
carrier, and its own reconnect posture. That is the 43-77 units, and it is the seam `AUDIT3_DISPATCH.md`
§4 drew correctly.

**And it becomes a trait, which is the one thing worth borrowing from the WebSocket pass — F234.**
`RpcMessageCallbacks` is a boxed closure behind a struct: a private `Arc<dyn Fn(..)>`, a
`pub(crate)` constructor, one public method. Read against `WsConnectionCallbacks` the two are
structurally identical, which corrects F215's *"odd one out"* wording — the four adapters are
traits, the two dispatch callbacks are not, and RPC's is the second instance rather than an
exception. The cost is growth, not encapsulation: adding a parameter means editing the field type,
the constructor bound and the method, and a closure type cannot carry a default body where a trait
method can. That is the mechanism F204 hit on WebSocket, where the disconnect reason had nowhere to
go. **The conversion belongs inside the PR that makes it grow and nowhere else**, since that PR
edits every one of those sites regardless.

**What is deliberately not borrowed.** D8 removed WebSocket's callbacks from the SPI entirely,
because core took the connection loop. That is not available here, and the measurement says so:

| | units apart | §4 verdict |
| --- | --- | --- |
| five WebSocket connection loops (F215) | 1–40 | inside or near the collapse band — core can take them |
| seven RPC **receive** loops | **83–257** | far past the 35–45 split line — they stay |
| seven RPC **client** loops (§12) | 0–23 | collapsible — which is why the client SPI *does* invert |

tcp and udp, identical in `forward_stream` and `drain_tasks`, are still 83 apart here. The receive
loop is where subscribe, reconnect, resubscribe-on-ConnAck, ack and consumer-group membership live,
and that is genuinely per-transport. **Same rule, same question, opposite answers on the two sides
of one transport** — which is the rule working rather than an inconsistency.

**The one signature change D7 buys:**

```rust
// today
Fn(RpcData, RpcCallInfo) -> Future<Output = RpcHandlerResult>
// after
Fn(Items<RpcData, RpcError>, RpcCallInfo) -> Future<Output = RpcHandlerResult>
```

Every adapter passes `Items::One(data)` on the day it merges. Nothing changes on the wire. The
request direction then has the same cardinality the answer has had since ADR-0049, and the two call
shapes become a follow-on rather than a second SPI.

### Client: `RpcClientTransport` stops being a call API

This is the larger half and the one the audit did not go looking for. Today the trait is
`send` / `emit` / `open_stream`, and each of the seven implements correlation, a pending map, a
`PendingSlot::{Single, Stream}` enum, the timeout, the per-frame gap deadline, the cancel notice and
a `forward_stream`. That is why `forward_stream` exists seven times (§12) and why D3 is seven
timeout branches.

```rust
pub trait RpcCarrier: Send + Sync + 'static {
    fn capabilities(&self) -> RpcCapabilities;

    async fn connect(&self) -> Result<(), RpcClientError>;
    /// Core calls it, and means it — F225's half that is not a doc fix.
    async fn close(&self) -> Result<(), RpcClientError>;

    /// One outbound frame. `reply` names where answers land; `None` is fire-and-forget.
    async fn dispatch(&self, out: Outbound<'_>) -> Result<(), RpcClientError>;

    /// Every inbound frame addressed to this client, one stream for the transport.
    fn inbound(&self) -> BoxStream<'static, InboundFrame>;
}
```

Core then owns, once: correlation ids, the pending map and both slot kinds, the call timeout, the
gap deadline, **the cancel notice on a `send` timeout (D3)**, `forward_stream`, and the reply-frame
parse that already lives in `ulo::rpc::wire`. `RpcClient::send` / `emit` / `stream` sit on top
unchanged, so nothing an application wrote has to move.

NATS is the honest exception on this side as it is on the other: its `send` is `async_nats`'s own
request against the library's multiplexed inbox, and its `forward_stream` measured 47-62 units from
every other. It implements `dispatch` and `inbound` over its subscription rather than pretending to
be a channel.

### What this does and does not solve

Checked against the eighteen findings rather than asserted. **Ten fall out of it**: F216, F217,
F221, F222, F224, F225, F226, F228, F229, F232. **Eight do not**, and they are not tidying:

- **F218** — Redis Pub/Sub has no competing-consumer mode. One place to declare and enforce delivery
  is what the collapse buys; the capability redis lacks is not something core ownership creates.
- **F219** — structural. On a subject broker the message is never delivered, so there is nothing for
  a core-owned path to route. What is gained is one honest answer instead of three.
- **F223** (which word), **F227** (Kafka topic lifecycle), **F230** (Kafka's literal), **F233** (CI)
  are each their own decision, and **F220** and **F231** are half-solved: core enforces, the adapter
  still has to declare its ceiling and its name rules.


## What is out of scope, and why

- **Guards, interceptors and the error chain on RPC.** The brief excludes them; the pipeline audit
  took all four transports at once and settled them.
- **gRPC.** A separate transport with its own ADRs (0033, 0038, 0039, 0043) and its own filed
  follow-ups. It appears here twice as a control — it reads a deadline off the wire where RPC does
  not (§6), and it answers `ResourceExhausted` where RPC answers `overloaded` (§7).
- **Nothing from request-side streaming.** An earlier revision put the two call shapes here. They
  are the build's **Phase 3** as of 2026-09-23, four PRs behind their own approval;
  `project-rpc-request-side-streaming` holds the background.
- **JSON-RPC 2.0 adoption.** `STANDARDS_AUDIT.md` §5 settled the shape of the question and F65 holds
  it. Nothing probed here bears on it, except to confirm that the three things the grammar carries
  which JSON-RPC does not — stream items, end markers, cancel notices — are all load-bearing.

## Couplings the triage pass should carry

- **F228's collapse and D1, D2 and D5 are one build, not four.** `data_to_bytes`/`bytes_to_data` is
  where D1's answer goes; `forward_stream` is where D3's answer goes; `drain_tasks` is D5's. Doing
  the collapse first and the decisions second writes each of them once; doing them in the other
  order writes them five times and then collapses five copies of the new behaviour.
- **F223 and F63 are one decision.** F63 is already deferred to the user; F223 says the same
  decision covers three words rather than one, and `error` is not defensible under any of F63's
  three candidate answers.
- **F226 and the DI audit's ambiguity finding are the same defect.** "An ambiguous question answered
  by whatever a hash map hands back first, reporting nothing" is `DI_AUDIT.md`'s summary of its own
  central finding; `pattern_map.insert` is a fourth site of it. Whoever fixes the DI sites should
  look at this one.
- **F219 and the extraction audit's F163 rhyme and are not the same.** Both are one seam answering
  fewer distinct outcomes than the situation has. F163 is a status a macro hardcodes; F219 is an
  event three transports structurally cannot raise, which no status change fixes.
- **D2 is the first decision in this set that changes a declaration's meaning.** `#[patterns]` and
  `#[event_pattern]` are macro surface, so it touches `ulo-macros` where nothing else here does.
- **D1's (d) is the HTTP build's `AdapterCapabilities` on a third transport, and must be co-designed
  with it rather than invented separately.** `HTTP_AUDIT.md` §1 introduces the type and the
  WebSocket build's D8 is the same move on WebSocket; RPC would be the third. One
  `#[non_exhaustive]` shape, one ADR, three transports — which is also the GraphQL audit's D7
  position on the engine SPI. **This changes a sequencing claim made before D1 was settled**: Phase
  1 of the RPC build still blocks on nothing, and Phase 2 now rides the HTTP build's PRs 1-9.
- **D7 reserves a signature the WebSocket and gRPC surfaces already have opinions about.**
  `Items<RpcData, RpcError>` on the request is ADR-0049's cardinality in the inbound direction;
  gRPC's `Inbound<T>` extractor (ADR-0043) is the handler-side answer to the same question on a
  transport that already has it. Whoever builds the follow-on call shapes should read that extractor
  before inventing a second one.

## What is owed

Nothing from this pass. `RPC_BUILD_HANDOFF.md` sits beside this file and carries the sequence.

Two things are owed *by* it and belong to other sessions:

- **An ADR for the capability SPI**, written with the HTTP build's rather than after it, since the
  two decide one type.
- **A gRPC audit**, with its own handoff and its own folder. Asked for by the user on 2026-09-23 and
  deliberately not started: the aim is to settle RPC first. gRPC appears in this memo only as a
  control — it reads a deadline off the wire where RPC does not (§6), and it answers
  `ResourceExhausted` where RPC answers `overloaded` (§7).
