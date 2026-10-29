# Questions after the twenty-third round is built and checked (for the designer)

Written for: the designer. The twenty-third round is built and checked by a second model: the `Bind` mapping test, the TCP link on a runtime without an IO driver, and Kafka's pacing loop. The check found no defect in the diff. It found three older behaviour defects beside it, one of the same class as the TCP panic you ruled on (P1, P2) and one on a healthy runtime (P3), and one question on §3.9 (P4). Detail: `FRAMEWORK_GAPS.md` F496 to F498; core `DESIGN.md` §3.2, §3.9; transports `DESIGN.md` §5.3, decision 30.

Paste alongside: core `DESIGN.md` §3.2 and §3.9, transports `DESIGN.md` §5.3 and decision 30.

## The behaviour defects

**P1. UDP, NATS and Redis panic on a runtime built without an IO driver (F496).** The TCP link no longer does: `prepare` probes the runtime and reports a `Permanent` `Configure` naming `enable_io`, and a TCP client is refused at `RpcClient::new` with `UnusableLink`. Probed on the check's snapshot, a runtime without IO, an unreachable address:
- UDP, NATS and Redis panic in `listen` and in every client call. That breaks §3.2, as the TCP panic did.
- MQTT answers a non-permanent `Bind` in 8 ms with no pointer to `enable_io`, and a client call `Unavailable` in 3 ms.
- Kafka is unaffected. Its tokio dependency has no `net` feature, and its results are the same with IO enabled.

The check recommends one shared method, `Tokio::io_enabled`, in `ulo-runtime-tokio`, called from each link's `prepare` (a permanent `Configure`) and `usable` (`UnusableLink`) for UDP, NATS, Redis, RabbitMQ and MQTT. It argues against putting the check inside `Tokio::run`: `run` is called per operation, it answers `RuntimeShutDown`, which is the wrong error, it fires at bind or call time and not at configure time, and RabbitMQ's panic is on lapin's own thread, which `run` never sees. Ruled for TCP only so far. Extend it to the five links through one shared method? Recommendation: yes. The alternative is five copies of the probe TCP now holds.

**P2. RabbitMQ on a runtime without IO never returns from `listen` (F496).** More than 100 s, with lapin's I/O thread panicking. A panic ends startup with a message naming the cause; this holds startup forever and reports nothing. The client side answers `Timeout` after 5 s, which reads as retryable. P1's method covers it, since `prepare` refuses before any connect. Confirm that P1's shape covers this too. Recommendation: confirm.

**P3. The RabbitMQ server's `listen` never returns against a broker that refuses connections, on a healthy runtime (F497).** More than 100 s. The cause is `enable_auto_recover()` at `ulo-rpc-rabbitmq/src/link.rs:196`; with it removed in a probe copy, the same case answers `Bind` "Connection refused" in 5 ms. That contradicts decision 30 and the refuse-or-none rule for `bind`. It predates this round and is a different cause from P1 and P2. Options: (i) drop auto-recover for the server's first connect and enable it once the connection is up; (ii) bound the first connect by the `connect` timeout the other links take. Recommendation: (i). Recovery serves a connection that existed, and a first connect that fails should fail the start, as the other brokers' do.

## One question

**P4. Kafka's refused backoff returns the last attempt's error, not `RuntimeShutDown` (F498).** `coordinator_retried` sleeps only while `now() + backoff` is before its deadline, so a refusal always arrives before the deadline, and the loop has no expiry outcome. `Expiry::passed()` would be a dead check there, so your F494 premise ("the last site where the (g) rule is not applied") does not hold, and no code changed. Core §3.9 now names the loops that ask and says Kafka's does not. Decision 30 documents the last-error ending. Kafka's `is_permanent` is false for a refused backoff, which sits beside §3.9's "one type … at every site." Is that the intended reading, with the timer refused being the app's and not the link's runtime? Or should a refused backoff there end as `Permanent` `RuntimeShutDown` like the rest? Recommendation: leave it. Both end the call the same way, the retries stop at once, and a deployer reading the last attempt's error learns what Kafka said. Changing it also changes the test `a_refused_backoff_ends_the_retries`.

## Listed, not batched

Refinements from the check, none a behaviour defect:
- **R4.** When a loopback socket cannot be bound, the TCP probe answers "enabled", which falls back to the old panic. A `UnixStream::pair()` probe on Unix, or a `catch_unwind` around `adopt` as a backstop, removes it.
- **R5.** The refused path prints a panic line through the panic hook, and a user's hook would record a panic. The healthy path prints nothing. The workspace sets no `panic = "abort"` profile. Both go in `io_enabled`'s doc.
- **R6.** The refusal-table row omits the `RpcClientModule` init-hook path, which reports `ConnectError::Hook`. Applied.

## For the record

- **Checks:** the builder's `ulo` and `ulo-rpc-tcp` suites pass; the 6 `no_io` tests and the new `startup_errors` test passed 30 of 30 idle and 30 of 30 under twelve busy loops. 42 planted breaks; 37 caught by the builder's tests, 40 with the check's three added tests. The two missed are one branch that cannot be tested without a seam (a fallback when a loopback socket cannot be bound) and one in Kafka's loop that predates the round.
- **No file-descriptor leak:** 10 before and 10 after 1,000 healthy and 1,000 refused probes. A probe costs 7.6 to 12.8 µs with IO and 34 to 39 µs refused.
- **Docs corrected by the check:** the §5.3 sentence on the other links (Kafka is unaffected, RabbitMQ's client times out, MQTT fails at once), the `Link::usable` trait doc (the smol TCP link has no tokio runtime and no such check), and the refusal-table row.
- **Catalogue:** Part A starts once this round is committed. It compiles against the tree this round leaves; P1 to P3 touch links Part A does not use.
