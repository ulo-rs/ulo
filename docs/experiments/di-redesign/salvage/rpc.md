# Salvage: the RPC wire audit

**Subject.** The RPC wire across seven transports (tcp, udp, nats, redis, rabbitmq, mqtt, kafka), audited 2026-09-20..23 against the old framework at `654c7183`: payload meaning, delivery cardinality, miss reporting, frame ceilings, shutdown, cancellation, client buffering, and each broker's request-reply convention. Findings F216-F234 in `FRAMEWORK_GAPS.md`, decisions D1-D8, a 23-PR build plan in three phases, and three probes.

**Read in full** (under `docs/experiments/di-redesign/salvage/source/rpc/`):

| File | Words |
| --- | --- |
| `RPC_AUDIT.md` | 8,974 |
| `RPC_AUDIT_HANDOFF.md` | 586 |
| `RPC_BUILD_HANDOFF.md` | 4,069 |
| `artifacts/probe-cases.rs` | 457 |
| `artifacts/rpc_wire_probe.rs` | 605 |
| `artifacts/probe-output-brokers.txt` | 342 |
| `artifacts/rpc_backpressure_probe.rs` | 645 |
| `artifacts/probe-output-backpressure.txt` | 84 |
| total | 15,762 |

Also read: the RPC rows of `source/BUILD_SEQUENCE.md` (rows 8-15, 41, 78, 108-122, 132; sign-offs S3, S9, S10, S11; Halt 1), and the ledger entries F216-F234, F63, F64, F65, F1, F46, F86, F215 in `/Users/ifeanyi/Projects/Personal/open-source/toni_rs/FRAMEWORK_GAPS.md`.

**Checked against** `experiment/di-redesign` at `331c35b9`, 2026-11-01: `crates/ulo-rpc`, `crates/ulo-rpc-{tcp,tcp-core,tcp-smol,udp,nats,redis,rabbitmq,mqtt,kafka}`, `crates/ulo-rpc-conformance`, `crates/ulo-rpc-macros`, `.github/workflows/ci.yml`, `Makefile`, and transports `DESIGN.md` §2.8, §5 and §13. Nothing was built or run. "DESIGN" below means `docs/experiments/di-redesign/transports/DESIGN.md`; its §5.2 paragraphs sit at lines 959 (deadline), 961 (codecs), 963 (broker lanes), 965 (misses), 967 (delivery), 969 (uniform errors), 971 (binary), 973 (flow control).

## Summary

| Status | Count |
| --- | --- |
| built | 41 |
| superseded | 32 |
| open | 9 |
| dropped | 5 |
| UNVERIFIED | 2 |
| **items** | **89** |

The rebuild absorbed most of this audit. Every one of the five "spines" (payload meaning, delivery, misses, ceilings, shutdown) is now a declared `Capabilities` field read in `prepare` (`crates/ulo-rpc/src/link.rs:272-315`); the cancel notice fires on a unary timeout and the caller's deadline travels as `deadline-ms`; the conformance suite runs on all seven links in CI; and the request-streaming call shapes that the audit put in a separately-approved Phase 3 exist. What did not carry over is the flow-control half of D8, pattern-name validation, and two parts of D2.

### The five most valuable open items

1. **F229 / D8: nothing bounds a call's buffered frames, in either direction.** The client's per-call reply queue is `mpsc::unbounded` (`crates/ulo-rpc/src/client.rs:240`, `:664-668`), and the server's per-call request-stream feed is too (`crates/ulo-rpc/src/dispatch.rs:150-157`, whose comment names `credit` as the missing window). `credit` is reserved in the grammar and sent by no link (DESIGN §5.2 line 973, §13 item 15). The request direction is not filed anywhere.
2. **F231: pattern names are not checked against the broker's naming rules or against ulo's own control lanes.** `check_pattern` refuses empty, whitespace and control characters only (`crates/ulo-rpc/src/server.rs:396-407`); a pattern equal to `ulo.rpc.control` (nats, rabbitmq, kafka), `ulo/rpc/control` (mqtt) or `ulo:rpc:control` (redis) is accepted.
3. **F227 residual: an unnamed Kafka client still creates a topic and a consumer group per connection and deletes neither** (`crates/ulo-rpc-kafka/src/link.rs:326-338`).
4. **The D8 trap (M7):** a client-side overrun delivered as an error marks the call settled, and a settled call sends no `cancel` (`client.rs:743-760`). Whoever writes any shed fallback for credit has to send `cancel` before the error.
5. **The request-direction mirror of F229**, listed separately because no ledger entry holds it: a client streaming faster than a handler reads grows the server's heap without bound, per call.

## Item table

Status abbreviations in prose: built, superseded, open, dropped. "Ledger" names the `FRAMEWORK_GAPS.md` entry that holds an open item.

### A. Findings F216-F234

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F216 | `RpcData`'s `Json`/`Text`/`Binary` arrive as different variants per transport | superseded | `RpcData` is gone; a payload is a codec-encoded `Data(Bytes)` (`crates/ulo-rpc/src/frame.rs:91`) decoded into the handler's `Payload<T>`; the kind is `PayloadKind::{Serde, Binary}` (`frame.rs:142-146`), D1(b)'s two-variant narrowing in another form | the probe matrix, below |
| F217 | a `Binary` payload reaches a tcp/udp handler as `Json(null)` | built | client refuses before I/O, `BadRequest` / `binary_unsupported` (`client.rs:446`); server refuses a binary handler on a JSON link in `prepare` (`server.rs:154-160`); case `payloads::binary` | none |
| F218 | one `emit` runs on every instance on nats/redis/mqtt and on one on rabbitmq/kafka | superseded, with loss | `Capabilities::delivery` (`link.rs:285`): NATS queue group and MQTT `$share/<group>/` make both `Competing` (`ulo-rpc-mqtt/src/link.rs:332`), AMQP and Kafka compete, Redis `FanOut` (`ulo-rpc-redis/src/link.rs:109`), tcp/udp `Addressed`; group defaults to the root module's path (DESIGN §5.2 line 967); case `delivery::two_instances` | see paragraph |
| F219 | an unhandled pattern answers three different errors; `Unrouted` unreachable on three brokers | built | `Unavailable` + `reason` `pattern_unhandled` (tcp/udp) or `no_destination` (NATS no-responders, Redis receiver count 0, AMQP `basic.return` on `mandatory`, MQTT PUBACK/PUBREC 0x10); `Timeout` where `miss_signal` is false: Kafka with auto-create on, MQTT QoS 0 (DESIGN §5.2 line 965; `Capabilities::miss_signal`, `holds_unserved`); case `misses::unhandled_pattern` | the Kafka and QoS 0 residuals are documented, not fixable |
| F220 | tcp buffered 64 MiB on one unterminated line | built | 4-byte big-endian length prefix bounded by `max_frame`, oversized prefix closes before the body is read (DESIGN §5.2 line 961); `DEFAULT_MAX_FRAME = 4 MiB` (`ulo-rpc-tcp-core/src/link.rs:25-27`), override `Tcp::max_frame` (`ulo-rpc-tcp/src/link.rs:87`) | default differs from D4's approved 1 MiB (designer Q3); no server-side hostile-prefix test found (§"Not verifiable") |
| F221 | tcp/udp drain, brokers abandon in-flight calls, NATS has no shutdown signal | built | `Link::drain` on every link (`link.rs` trait); cases `drain::{drain, window, spent_cap}` on all seven; NATS declares `confirms_drain(false)` (`ulo-rpc-nats/src/link.rs:296`) with the reason in `Capabilities::confirms_drain`'s doc (`link.rs:299-308`); RabbitMQ acks after the handler completes (DESIGN §5.3 table, RabbitMQ row) | the old rabbitmq ack-before-publish ordering lost the request as well as the reply; a regression test asserting ack follows the reply publish is worth keeping |
| F222 | cancel notice never sent for a unary timeout; caller's deadline does not travel | built | `Pending::drop` sends `cancel` unless settled, covering timeout, cancel and drop (`client.rs:743-760`); `deadline-ms` reserved header carries the remaining milliseconds, stamped after connect (DESIGN §5.2 line 959); cases `deadlines::*` (28 scenarios) | residual U17 in DESIGN line 963: on a broker a `cancel` overtaking its `req` is dropped and the call runs to its end (`ordered_control: false`) |
| F223 | three wire statuses (`overloaded`, `unsupported`, `error`) are not `ErrorKind` names; udp oversize single reply dropped, stream ended | superseded, built differently | shed answers `unavailable` + `RetryAfter` (DESIGN §2.8, §5.2 line 973); a stream on a link without streamed shapes answers `internal` naming the link (DESIGN §5.1); an oversize or unencodable reply answers `err` `internal` under the same id for unary and stream alike (`dispatch.rs:612-640`) | departs from S3's signed words (designer Q2) |
| F224 | no broker transport bounds concurrent handlers; `basic_qos` unused | built | `max_inflight` on every link, 1,024 unset (DESIGN §2.8); AMQP `basic.qos` `global = true` from the bound, Kafka pauses partitions (`native_backpressure`, DESIGN §5.2 line 973; `Link::max_inflight`); case `admission::over_the_bound` | none |
| F225 | `RpcClient::close` claims a flush six transports skip; `connect` doc claims an automatic call | built | `Link::close` per link (`link.rs` trait), case `client_close::client_close`; connection is lazy through `ClientInner::connection` (`client.rs:676`), no bootstrap-connect claim | none |
| F226 | two controllers may claim one pattern; later wins; NATS delivers twice | built | `prepare` refuses "both handle pattern `x`" (`server.rs:164-168`); routes are a map keyed by pattern, listened once each (`server.rs:201-203`) | the probe (`probe_duplicate_pattern`) is the regression test shape |
| F227 | Kafka client creates a topic and group per process, deletes neither; single-partition unreplicated defaults | open (residual) | `reply_topic(name)` exists (`ulo-rpc-kafka/src/link.rs:174-177`); unset, each connect makes `ulo.rpc.reply.{uuid}` and a group of that name (`:326-338`); no topic deletion in the crate; partitions and replication now settable (`:188`, `:235-240`); server cancel group clause gone (control topic assigned, group commits nothing, DESIGN §5.3 Kafka row). Ledger: F227's "narrowed" note, whose cancel-group clause is now obsolete | see paragraph |
| F228 | `forward_stream` written seven times; payload converter five times | superseded | core owns the client (`crates/ulo-rpc/src/client.rs`) and the dispatcher (`dispatch.rs`); links implement `Link` only; tcp and tcp-smol share `ulo-rpc-tcp-core` | the measurement method, M5 |
| F229 | nothing bounds a reply stream's client buffer; producer runs at full speed | open | `type Waiting = UnboundedSender<..>` (`client.rs:240`), per-call `mpsc::unbounded()` (`client.rs:664-668`); `credit` reserved, unsent (DESIGN §5.2 line 973, §13 #15). Ledger: F229, citations to old code | see paragraph |
| F230 | Kafka `with_timeout` bounds the reply wait, not the call (5 s literal) | superseded | the client's bound covers connect, publish and reply (DESIGN §5.2 line 959, `deadline-ms` stamped after connect); the server's reply goes through the link's writer queue with `QUEUE_TIMEOUT` 5 s on queue-full and librdkafka `message.timeout.ms` 30 s (`ulo-rpc-kafka/src/link.rs:62-65`, `:1396-1420`) | whether a reply publish outlives the call's deadline is UNVERIFIED; the dispatcher task is no longer what it holds |
| F231 | patterns reach brokers unvalidated; four naming rule sets; reserved cancel channels unprotected | open (partial) | `Pattern::new` validates nothing (`link.rs:239-242`); `check_pattern` refuses empty/whitespace/control only (`server.rs:396-407`, mirrored in `ulo-rpc-macros/src/message.rs:100-102`); control lanes `ulo.rpc.control` (`ulo-rpc-nats/src/link.rs:52`, `ulo-rpc-rabbitmq/src/link.rs:49`, `ulo-rpc-kafka/src/link.rs:54`), `ulo/rpc/control` (`ulo-rpc-mqtt/src/link.rs:46`), `ulo:rpc:control` (`ulo-rpc-redis/src/link.rs:30`) not reserved. Ledger: F231, old cancel-channel names | see paragraph |
| F232 | `ulo::rpc::wire` lane doc enumerates statuses its own branches break | superseded | the `err` frame carries `e:{kind,message,details}` for any kind (DESIGN §5.2 grammar); there is no two-lane rule to document | the corrected sentence (a lane means "reached no controller", not a status list) is moot |
| F233 | conformance suite never runs; omits tcp/udp; covers six cases | built | CI `broker integration` job runs nats, redis, mqtt, rabbitmq and kafka on every push (`.github/workflows/ci.yml:300-345`); tcp, udp and tcp-smol suites in the test jobs (`ci.yml:84-141`); `make conformance-kafka` for local runs (`Makefile:1-13`); 18 case modules (`crates/ulo-rpc-conformance/src/cases/`) | broker job is push-only, not on pull requests |
| F234 | `RpcMessageCallbacks` is a boxed closure that cannot grow | superseded | `Link::listen` returns an `Inbound` stream of `Delivery { frame, reply, ack }` and core dispatches (`link.rs` trait, `dispatch.rs:139-160`); no callback type | the §15 measurement that receive loops stay per-link still matches: each link owns its receive loop |

**F216, value.** The probe matrix (`probe-output-brokers.txt`, `RPC_AUDIT.md` §1) recorded at `654c7183`: `Text("hello")` arrived as `Json("hello")` on tcp/udp, `Binary` on nats/mqtt/rabbitmq/kafka, `Text` on redis; `Binary(b"{\"a\":1}")` arrived as `Json(null)` on tcp/udp and `Json({a:1})` on the four raw-byte brokers. The lesson that outlives the type: on a raw-bytes broker an inbound body carries no tag, so any variant distinction the wire does not record is unrecoverable however the SPI is modelled. The rebuild keeps raw bodies on NATS, AMQP, MQTT and Kafka (DESIGN §5.2 line 963, `nats req order.create '{..}'` reaches a handler), which was D1's stated price of the rejected tagged-envelope option. A typed `Payload<T>` makes the matrix moot for serde payloads; it does not test that a JSON link and a CBOR link decode one `Bytes` payload alike, which `payloads::binary` covers only for the CBOR case.

**F218, value.** D2 settled two things the rebuild does not have. (1) A per-pattern delivery override with `broadcast` as the default for `#[event_pattern]`. The rebuild has one `DeliveryMode` per link, so on NATS, MQTT, AMQP and Kafka every event competes: an event meant for every instance (cache invalidation, config reload) has no spelling. D2's argument, `RPC_AUDIT.md` §14 D2: "A request-reply pattern answered by N instances is a defect on any reading; an event fanned to N instances is a legitimate design." The mechanisms D2 named per broker: NATS plain subscribe, MQTT a non-shared filter, AMQP a per-instance exclusive queue bound to a fanout, Kafka a per-instance consumer group. (2) A Redis refusal of `balanced` at bind. The rebuild leaves Redis `FanOut` for requests too: every instance runs the handler and the client drops the second reply (DESIGN §5.3 Redis row, `client.rs:19`). The side effect runs N times while the caller sees one answer. Also kept: MQTT v5 §4.10.1, "The Responder can use a Shared Subscription to allow for a pool of responding Clients", which the rebuild now follows.

**F227, value.** Kafka's reply topic and its group are durable broker objects; a NATS inbox, a Redis channel, MQTT's reply topic and RabbitMQ's direct reply-to all vanish with the connection. An unnamed client therefore leaks one partition and one group per connect, held until `offsets.retention.minutes` (7 days default) expires the group and forever for the topic, against a cluster-wide partition ceiling. A service restarting on every deploy reaches it. The rebuild already derives a stable default for the server's consumer group from the root module's path (DESIGN §5.2 line 967); the same derivation for the client's reply topic would close the residual. "Nothing declines auto-creation" also still holds: the link creates handler topics at `bind` (DESIGN §5.2 line 965).

**F229, value.** The probe (`artifacts/rpc_backpressure_probe.rs`, output `probe-output-backpressure.txt`), tcp in-process, 1 KiB items: consumer reading one item per 100 ms, gap produced-minus-consumed 36,312 / 72,613 / 108,817 / 145,111 at 1-4 s, RSS 18 MiB to 176 MiB; control reading flat out, gap 2, flat, RSS 16 to 17 MiB, at the same production rate of about 36,000 items/s. Identical rates mean no backpressure at all rather than weak backpressure, about 38 MiB/s of growth; the control's flat gap is what proves the harness measures buffering. The arguments that settled D8 as credit (`RPC_AUDIT.md` §14 D8):

- a high-water mark buys `threshold ÷ rate` seconds of consumer slowness (64 MiB buys 1.7 s and 1 GiB 27 s at 38 MiB/s), so no number is both memory-safe and generous to a pausing consumer;
- bounding the queue and blocking the reader stalls every other call on the connection, HTTP/1.1 pipelining's head-of-line problem, the reason HTTP/2 has per-stream windows;
- credit bounds memory at credit × item size per call, stalls nothing else, sheds nothing;
- **credit cannot ride a best-effort carrier**, because a missed cancel wastes work and a missed credit frame stalls the stream forever. At HEAD the broker control lanes are exactly such a carrier: one shared lane per link read by every instance, `ordered_control: false` except TCP and Redis (`link.rs:309-314`), and a `cancel` that overtakes its request is dropped (DESIGN line 963, U17). D8's per-transport cost reading: tcp/udp in band, free; nats/redis/mqtt a per-call control address, free in their namespaces (Unverified in the memo, reasoned not probed); kafka/rabbitmq a durable object per call, so those two declare they cannot and fall back to a bounded queue with shed-and-cancel as a safety valve.

At HEAD the request direction has the same hole (`dispatch.rs:150-157`), which D8 anticipated: "A client streaming a request needs the server to bound it, which is the same mechanism in the other direction — so credit is built before the call shapes." The rebuild built the call shapes first. The probe is promotable as the verification: slow-consumer gap plateaus and the producer's rate drops below the control's.

**F231, value.** The four rule sets the audit collected: NATS treats `*` and `>` as wildcards; MQTT treats `+` and `#` as wildcards and forbids U+0000; Kafka topic names are `[a-zA-Z0-9._-]`, at most 249 characters, with `.` and `_` colliding in metric names; AMQP queue names are a `shortstr` (255 octets) with the `amq.` prefix reserved. At HEAD each broker's refusal of a bad name presumably surfaces from `listen` as a `bind` error, since the broker links have no panicking sites (item A5), which removes the worst old mode (a panic in the serve future after `bind` succeeded); whether a wildcard pattern subscribes and then never routes is not verified here. The reserved-name check belongs in core because the control lanes are ulo's: a handler on `ulo.rpc.control` would share a lane that every instance reads for `in`, `in_end` and `cancel`.

### B. Earlier entries the memo moved or the brief listed

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| F63 (count corrected by F223) | the load shed answers `overloaded`, not an `ErrorKind` | superseded | shed is `unavailable` + `RetryAfter` on every transport that refuses (DESIGN §2.8) | the gRPC audit's reading that `statuscodes.md` folds a shed load into `RESOURCE_EXHAUSTED`; see designer Q2 |
| F64 | one RPC error object spells two fields in two cases | superseded | one error body `e:{kind,message,details}` (DESIGN §5.2 grammar) | none |
| F65, with STANDARDS §5 (JSON-RPC 2.0) | the RPC error payload ignores a published standard | dropped | DESIGN §5.2 defines the frames "since no specification governs them"; JSON-RPC not adopted | the argument that decided it: JSON-RPC is silent on streaming replies and cancellation, so adopting it means extending it, and an extension carrying a standard's name borrows credibility; what maps cleanly is the correlation `id`, notification as absent `id`, transport-agnosticism (`RPC_AUDIT_HANDOFF.md`). The rebuild's `evt` without `id` and `req` with `id` match that mapping |
| F1 | an extraction failure renders `Internal` on RPC | built | case `errors::undecodable_payload` asserts `BadRequest` (`crates/ulo-rpc-conformance/src/cases/errors.rs:38-43`) | none |
| F86 | an RPC handler must return a `Result` | built | a handler returns `Reply`, `Data`, any `Serialize`, `()`, a stream, or a `Result` of those (DESIGN §5.1) | none |
| F46 (consequence added: F226) | RPC dispatches by string match inside one controller | built | one route per handler, keyed by pattern, duplicates refused (`server.rs:115-170`) | none |
| A5 (`DOCS_AUDIT.md` §17, counts nats 3, redis 2, rabbitmq 8, kafka 4, mqtt 0) | broker adapters panic in the serve future after `bind` succeeded | superseded | non-test code of the seven link files holds no `panic!`/`unwrap()` and four `expect`s, all on internal invariants (`ulo-rpc-kafka/src/link.rs:599, 1502, 1510`; `ulo-rpc-tcp-core/src/link.rs:579`); `listen`/`connect` return `Result` | none |
| F215 wording correction | `WsConnectionCallbacks` is not the odd one out; RPC's callbacks share its shape | dropped | both callback types are gone at HEAD | none |

### C. Decisions D1-D8

| ID | What was settled | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| D1 | (b) `RpcData` narrows to `Json \| Binary`, plus (d) `RpcCapabilities` read at bind: payload support, `max_frame`, delivery modes, pattern-name rules, streaming | built (pattern rules missing) | `PayloadKind::{Serde, Binary}`; `Capabilities { binary, max_frame, ordering, shapes, native_backpressure, delivery, miss_signal, holds_unserved, durable_replies, confirms_drain, ordered_control }` (`link.rs:272-315`), read in `Server::prepare` (`server.rs:29-32`); pattern-name rules absent (F231) | the "redis stops tagging" detail is moot: Redis carries the whole frame with `d` codec-encoded, as every link encodes it |
| D2 | adapter default with per-pattern override; `balanced` for messages, `broadcast` for events; redis refuses `balanced` | superseded, with loss | one `DeliveryMode` per link, no override, no redis refusal (F218 row) | F218 paragraph; designer Q1 |
| D3 | (a) cancel on `send` timeout now; (b) deadline on the wire filed with a trigger | built, both halves | `client.rs:743-760`; `deadline-ms` (DESIGN line 959). (b) was built without the trigger | the memo's clock-skew note, that gRPC sends a duration rather than an instant, is what `deadline-ms` does (remaining milliseconds) |
| D4 | tcp max frame, default 1 MiB, declared as a capability | built, number differs | `DEFAULT_MAX_FRAME` 4 MiB, "twice HTTP's default body limit" (`ulo-rpc-tcp-core/src/link.rs:25-27`); declared through `max_frame` on every link: udp 65,507 (`ulo-rpc-udp/src/link.rs:27, 198`), kafka 1,000,000, nats from `INFO`, mqtt from CONNACK | the 1 MiB argument: udp's ceiling is 65,507 bytes and tcp is what a caller picks when a payload does not fit a datagram, so the datagram size defeats the choice; 1 MiB is sixteen times that; actix 256 KiB and rocket 32 MiB bracket the range. Designer Q3 |
| D5 | brokers get the tcp/udp `JoinSet` + `drain_tasks`; NATS gains a shutdown signal | built | F221 row | none |
| D6 | CI for nats/redis/mqtt/rabbitmq on merge, `make` for kafka; tcp/udp implement `Broker` | built, exceeded | F233 row; Kafka also runs in CI | none |
| D7 | request streaming rides at the SPI and grammar level; call shapes in a separately approved Phase 3 | built, exceeded | `open`/`in`/`in_end` frames (DESIGN §5.2 grammar); `Inbound<T>` parameter, client-streaming and bidi handlers (DESIGN §5.1); UDP declares `UNARY_ONLY` (`link.rs:320-321`); cases `streams::{client_stream, client_stream_in_order, bidi_stream}` | D7's risk statement, that narrowing the payload type without the inbound direction in view is how a type gets re-widened a release later, did not materialise: `Inbound<T>` decodes the same `PayloadKind` |
| D8 | per-call credit, scheduled; kafka/rabbitmq declare they cannot and fall back to bounded queue with shed-and-cancel | open | F229 row | F229 paragraph |

### D. Memo-local observations not filed as F-entries

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| M1 (`RPC_AUDIT.md` §11 AMQP) | AMQP does not decide request-reply | built (convention followed) | `reply_to` + `correlation_id`, direct reply-to `amq.rabbitmq.reply-to` (`ulo-rpc-rabbitmq/src/link.rs:51-53`) | the spec citation: the AMQP 0-9-1 class definition (`rabbitmq.com/resources/specs/amqp0-9-1.xml`) marks `correlation-id` and `reply-to` "For application use, no formal behaviour", `reply-to` adding only "may hold the name of a private response queue, when used in request messages". RabbitMQ's tutorial convention is not a conformance target |
| M2 (§11 MQTT) | MQTT v5 request-reply is conformant | built | request sets `response_topic` (`ulo-rpc-mqtt/src/link.rs:1428-1430`); reply carries `correlation_data` and no response topic (`:1693-1699`); reply topic `ulo/rpc/reply/{id}` holds no wildcard (`:374`) | citations: §3.3.2.3.5 "The presence of a Response Topic identifies the Message as a Request"; §4.10.1 "the Responder copies this property into the Response Message … The Response Message does not include a Response Topic property"; `[MQTT-3.3.2-14]` forbids wildcards in a Response Topic. A conformance assertion on these three is a test idea none of the 18 case modules holds by name |
| M3 (§11) | repeated headers collapse: NATS kept the first value, MQTT and Kafka the last; MQTT order lost against `[MQTT-3.3.2-18]` | built | `CallHeaders` is an ordered `Vec`, repeats kept (`crates/ulo-rpc/src/transport.rs:232-248`), filled per value on NATS (`ulo-rpc-nats/src/link.rs:1271-1284`), MQTT (`ulo-rpc-mqtt/src/link.rs:1713-1721`), Kafka (`ulo-rpc-kafka/src/link.rs:1451-1465`); AMQP keeps the last value of a name, documented (DESIGN §5.3 RabbitMQ row) | `CallHeaders::get` answers the first value (`transport.rs:242-244`); a `get_all` is the remaining surface question, minor |
| M4 (§11 AMQP) | direct reply-to consumer is established before any request names it, the one ordering RabbitMQ requires | UNVERIFIED | `link.rs:127` says the connect opens "the connection, its channel and the direct reply-to consumer"; the order against the first publish not read | needs a read of the connect path |
| M5 (§12, §15) | measurements under `AUDIT3_DISPATCH.md` §4's rule: `forward_stream` tcp/udp 0 units, brokers 15-23, nats 47-62; dispatch blocks 43-77; receive loops 83-257; client loops 0-23 | superseded | the code measured is gone | the method and its verdict: core owns the sequence, the adapter owns the publish and the receive loop. "Same rule, same question, opposite answers on the two sides of one transport." The rebuild matches it: core owns the client and the dispatcher, each link owns its receive loop |
| M6 (§15) | proposed SPIs: `RpcAdapter { capabilities, register_handlers, into_lifecycle }` and `RpcCarrier { capabilities, connect, close, dispatch, inbound }` | superseded | one `Link` trait serves both sides: `capabilities`, `prepare`, `usable`, `max_inflight`, `listen`, `connect`, `drain`, `close`, `bound` (`link.rs` trait from line 35) | none |
| M7 (§14 D8) | an `Err` item marks a reply stream ended and suppresses the cancel notice | open (hazard, no defect today) | `Pending::drop` returns early when `settled` (`client.rs:752-757`); nothing at HEAD sheds | the rule for any shed fallback: send `cancel` before delivering the overrun error, or the producer runs on. The old code pinned the suppression in `an_err_item_is_terminal` |
| M8 (F219's open question) | should a `Timeout` be distinguishable from a miss? | built | `reason` `no_destination` / `pattern_unhandled`; `miss_signal` declares where it is possible (DESIGN line 965) | the memo's alternative (a broker-side pattern-registry probe at startup) was not needed |
| M9 (coupling) | F226 is the DI audit's hash-map ambiguity at a fourth site | built (RPC side) | `server.rs:164-168` | none |
| M10 (coupling, S10) | D1(d) is HTTP's `AdapterCapabilities` on a third transport: one `#[non_exhaustive]` type, one ADR | superseded | each transport declares its own: `ulo_rpc::Capabilities` (`link.rs:272`) | none |
| M11 (coupling) | read gRPC's `Inbound<T>` before inventing RPC's request-stream extractor | built | RPC's parameter is `Inbound<T>`, detected by type through an autoref probe (DESIGN §5.1) | none |
| M12 | a gRPC audit is owed | dropped | outside this subject; `source/grpc/` holds it | none |

### E. Build plan rows, sign-offs and ADRs

Phase 1 rows 1-8 merged on master (`3cc3651`) and on old code; their status here is the rebuild's.

| ID | What | Status | Evidence at HEAD | Value to extract |
| --- | --- | --- | --- | --- |
| PR1 / RPC-1 (row 8, #348) | refuse `Binary` on tcp/udp before I/O | built | F217 row | none |
| PR2 / RPC-2 (row 14, #352) | client-lifecycle docs | superseded | F225 row | the correction appended to F225 on 2026-09-25: the audit's grep missed `impl Provider for RpcClient` calling `connect` from a hook, a grep that excluded the file making the call |
| PR3 / RPC-3 (row 15, #353) | lane doc | superseded | F232 row | none |
| PR4 / RPC-4 (row 9, #349) | `Broker` for tcp and udp | built | `crates/ulo-rpc-{tcp,udp,tcp-smol}/tests/conformance.rs` | none |
| PR5 / RPC-5 (row 10, #350) | probe cases promoted as assertions | built | `delivery::two_instances`, `misses::unhandled_pattern`, `payloads::binary` | none |
| RPC-CI (row 11, #351, S11) | the suite in CI | built | F233 row | none |
| PR6 / RPC-6 (row 12, #369) | Kafka 5 s literal | superseded | F230 row | Halt 1 below |
| PR7 / RPC-7 (row 41, #382, unmerged) | refuse duplicate patterns at `create` | built | F226 row | none |
| PR8 / RPC-8 (row 13, #354) | stable Kafka reply topic; partition/replication knobs | open (residual) | F227 row | F227 paragraph |
| PR9 / RPC-9 (row 78) | capabilities on both SPIs | built (pattern rules missing) | D1 row | none |
| PR10 / RPC-10 (row 108) | `RpcData` narrowing; collapse the converters | superseded | F216 row | none |
| PR11 / RPC-11 (row 109) | client collapse; cancel on `send` timeout | built, differently | `RpcClient` in core over `Link::connect` (`client.rs`), cancel at `client.rs:743-760` | none |
| PR12 / RPC-12 (row 110) | server collapse: dispatch sequence, drain, bound, `basic_qos` | built | `dispatch.rs`, `server.rs`, F221 and F224 rows | none |
| PR13 / RPC-13 (row 111) | callbacks become a trait taking `Items<RpcData, RpcError>` | superseded | F234 row | none |
| PR14 / RPC-14 (row 112) | declared delivery mode with per-pattern override | superseded, with loss | F218 row | F218 paragraph |
| PR15 / RPC-15 (row 113) | tcp `max_frame` 1 MiB | built, number differs | D4 row | none |
| PR16 / RPC-16 (row 114) | pattern-name validation; reserved control names in core | open | F231 row | F231 paragraph |
| PR17 / RPC-17 (row 115) | one word for three statuses; udp single reply writes the error frame | superseded, built differently | F223 row | designer Q2 |
| PR18 / RPC-18 (row 116) | credit, core and tcp/udp | open | F229 row | F229 paragraph |
| PR19 / RPC-19 (row 117) | credit on brokers; kafka/rabbitmq fallback, cancel before error | open | F229 row, M7 | F229 paragraph |
| PR20 / RPC-20 (row 119) | inbound grammar; udp declares it cannot order inbound frames | built | D7 row | none |
| PR21 / RPC-21 (row 120) | handler-side request-stream extractor | built | `Inbound<T>` (DESIGN §5.1) | none |
| PR22 / RPC-22 (row 121) | client streaming | built | `send_stream` (DESIGN §5.2 line 959; SWITCH_INVENTORY row `rpc_streaming.rs`) | none |
| PR23 / RPC-23 (row 122) | bidi | built | `duplex` (same) | none |
| Row 132 / D3-ii-RPC | the cancel frame and a dropped connection fire a unary call's token, not only abort its task | built | `cancel` "fires ClientCancelled" (DESIGN §5.2 grammar); a failed reply send cancels with `Disconnected` (`dispatch.rs:631-638`); case `cancel::mid_stream` asserts the reason for a stream | a unary `cancel` scenario asserting `ClientCancelled` is not among the case names read; the `order::*` cases cover cancel ordering |
| S3 | words: `overloaded` → `TooManyRequests`, `unsupported` → `Unimplemented`, `error` → `Internal` (approved 2026-09-24) | superseded, built differently | F223 row | designer Q2 |
| S9 | Phase 3 is its own approval gate | dropped | the shapes exist; the gate concerned a build that never ran | none |
| S10 | one capability ADR covering HTTP, WS, RPC and GraphQL | superseded | M10 | none |
| S11 | D6 needs its own row | built | RPC-CI | none |
| Halt 1 (row 12) | Kafka server-side "remaining budget" names a number that does not exist before RPC-12; user chose client half now | superseded | the server now has a deadline from `deadline-ms` (DESIGN line 959) | the Halt's reasoning is the lesson: a "remaining budget" phrase needs a budget the server can read, and only a deadline on the wire supplies one |
| ADR-capability (PR9) | "a transport declares what it can carry" | superseded | DESIGN §5.2-§5.3 and `Capabilities`' docs carry it; no ADR file (`docs/adr/` ends at 0056) | none |
| ADR-payload (PR10) | what `RpcData` means on the wire | superseded | DESIGN §5.2 "Codecs" and "Binary payloads" | none |
| ADR-delivery (PR14) | delivery cardinality | superseded | DESIGN §5.2 "Delivery to two instances" | none |
| ADR-request-streaming (Phase 3) | both call shapes and the inbound grammar | superseded | DESIGN §5.1-§5.2 | none |

### F. Probes and artifacts

| ID | What it proves | Status | Holds on the new code? | Value to extract |
| --- | --- | --- | --- | --- |
| P1 `probe-cases.rs` | three broker cases through `conformance_suite!`: unrouted outcome, payload variant matrix, two instances one emit | built (promoted) | the old API (`UloFactory`, `#[patterns]`, `RpcData`) is gone; equivalents are `misses::unhandled_pattern`, `delivery::two_instances` | the self-test rule from the handoff: `probe_two_instances_one_emit` carries its control in the result (1 on rabbitmq/kafka, 2 on the rest through one counter), so a run where all five agree before a fix has broken the harness |
| P2a `rpc_wire_probe.rs` `probe_tcp`, `probe_udp` | the tcp/udp unrouted answer and variant matrix | superseded | no; `RpcData` gone, misses now `pattern_unhandled` | matrix in F216 paragraph |
| P2b `probe_abandoned_single_call_is_silent` | intended: the server hears nothing when a unary caller times out | dropped (defective) | n/a | the probe calls `probe.slow`, which no controller declares, and reads `FANOUT`, which only `probe.fanout` increments: the call is an unrouted miss and the assertion is true by construction. `RPC_AUDIT.md` §6 calls F222 "Read, with the tcp half probed"; the tcp half was in fact read, not probed. The finding still held by reading, and the rebuild fixed it |
| P2c `probe_tcp_unterminated_line` | a 64 MiB line with no `\n` is accepted whole | superseded | no; framing is a length prefix | test idea for HEAD: a raw socket writes a 4-byte prefix above `max_frame` and asserts the connection closes before any body byte is read; and a prefix at `max_frame` with a body trickled byte by byte. No server-side test of either was found by name |
| P2d `probe_duplicate_pattern` | two controllers on one pattern: `create` and `bind` succeed, the second answers | built | refused at `prepare` (`server.rs:164-168`) | none |
| P3 `rpc_backpressure_probe.rs` | the client reply queue is unbounded; producer rate independent of the consumer | UNVERIFIED | the chain it measured is rewritten; reading says the per-call queue is still unbounded (`client.rs:664-668`) | F229 paragraph; promotable as the credit verification, with its control |
| P4 `probe-output-brokers.txt` | the run behind §1-§3's tables | superseded | baseline of old code | the Kafka suite took 347 s in this file and "578 s for nine cases" in `RPC_AUDIT.md` (Confidence marking); two runs, both recorded as Kafka's cost, which D6 used to keep Kafka out of CI and the rebuild runs in CI anyway |
| P5 `probe-output-backpressure.txt` | the numbers behind D8 | superseded | old code | numbers in F229 paragraph |

## Worth discussing with the designer

1. **D2: should an `#[event]` be able to reach every instance on a competing broker, and should Redis request-reply be refused or flagged?** At HEAD every event on NATS, MQTT, AMQP and Kafka competes, and a Redis request runs on every instance with the duplicate reply dropped. Recommendation: a per-handler override for events, `broadcast` on the four competing brokers by the mechanisms in the F218 paragraph, keeping `Competing` as the default the rebuild chose; for Redis, a `warn` at `prepare` naming `FanOut` for every `#[message]` pattern, since a single instance cannot be told from many at bind.
2. **S3's signed words versus the rebuild's.** The user approved `TooManyRequests` for a shed load, `Unimplemented` for a stream on a link without streams, `Internal` for an oversize reply (`BUILD_SEQUENCE.md` S3, 2026-09-24). The rebuild answers `unavailable` + `RetryAfter`, `internal`, `internal`. Recommendation: confirm the rebuild's words explicitly, since a signed decision was changed. `unavailable` + `RetryAfter` is uniform across transports in §2.8 and defensible; the record should say S3 is superseded and why.
3. **D4's default: 1 MiB approved, 4 MiB built.** Recommendation: keep 4 MiB if "twice HTTP's default body limit" is the intended rule, and record that it replaces D4's number; it is one constant either way.
4. **When is credit built, and in what order relative to client streaming?** D8 scheduled credit before the request shapes; the rebuild built the shapes and reserved credit. Both directions now buffer without bound per call. Recommendation: build credit on TCP first, in band on the connection that carries `cancel` (UDP carries no streamed shape), and until then bound each per-call queue with shed-and-cancel as D8's safety valve, cancel sent before the error (M7). Note that the rebuild's shared, best-effort broker control lane is the carrier D8 said credit cannot use, so the broker half needs a per-call control address or a declared `credit: false`.
5. **Pattern-name validation (F231).** Recommendation: each link checks its broker's rules in `prepare` (`Link::prepare` already returns a `Result`), and core refuses a pattern equal to any link's control lane name, both naming the pattern and the rule.
6. **Kafka's default reply topic (F227).** Recommendation: default the reply topic and its group to a stable name derived as the server's group default is (root module path), keeping `reply_topic(name)` as the override.

## Not verifiable here

- **P3 / F229 at HEAD:** that a slow consumer grows the client's memory linearly needs the backpressure probe ported to the new API and run; reading `client.rs:664-668` and `dispatch.rs:150-157` says it will.
- **F230 server clause:** whether a Kafka reply publish can outlive the call's `deadline-ms` (writer queue, `QUEUE_TIMEOUT` 5 s, `message.timeout.ms` 30 s) needs a run against a broker refusing writes.
- **M4:** RabbitMQ's direct reply-to consumer ordering against the first publish needs a read of the client connect path.
- **F231's behaviour per broker:** what a wildcard pattern does on NATS and MQTT at HEAD (subscribe and never route, or refused) needs a broker run.
- **P2c's test:** the DESIGN claim that an oversized TCP prefix closes the connection before the body is read has no server-side test found by name in `ulo-rpc-tcp/tests`, `ulo-rpc-tcp-smol/tests` or `ulo-rpc-tcp-core/src`; a hostile-peer run would settle it.
