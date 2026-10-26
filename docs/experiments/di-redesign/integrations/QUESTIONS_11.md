# Questions after the fifteenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the fifteenth response (J1 to J8, the `Timer` ruling) is built and checked by a second model. The check found three defects, closed. Each item below needs a ruling and carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F482, F484, F485; transports `DESIGN.md` §5.2, §5.3.

Paste alongside: transports `DESIGN.md` §5.2 ("A cut `close`."), §5.3.

## Where the build departed from a ruling

**K1. NATS: the keep task's flush-order comment does not say "neither changes what a peer reads".** J7 asked for that line at the flush. The builder and the checker both read the code and found it false in one case. If `close`'s first poll finds the channel full, some answers wait for room, and `close` is then cut, a flush made before `close` ends can be answered before those answers are queued. The task then drops the last `Client` and async-nats discards them. Built: the flush runs once `close` ends, and the comment says it follows every answer `close` queued, the waited ones included. Read, not probed; the early-flush break is unpinned. Confirm the wording? Recommendation: confirm, and either pin the break with a gated test or accept it as unpinned.

**K2. NATS has no `opened` closed check and no `Notified`.** J7's two items do not apply there. Its wait for room is a poll of async-nats's `publish`, which tries for room and registers for it in one call. No gate test was added. Recommendation: accept.

**K3. MQTT: the `enable()` the ruling asked for guards nothing.** tokio documents that a `Notified` is guaranteed `notify_waiters` wakeups from its creation, and the loop wakes waiters only with `notify_waiters`. It was added because the ruling asked. The gate test pins creation before the attempt. Recommendation: leave it with the comment saying so, or delete it.

## What a link cannot do

**K4. TCP: a peer that shuts its write half at once and never reads holds the writer's socket indefinitely.** The linger ends at the peer's FIN, but the writer detaches and keeps the socket until the peer reads or the process ends (probed: 128 of 128 answers read after 20 s on the app's clock; HEAD does the same). J5 says the linger ends at `CUT_GRACE`; it bounds the read side. Bounding the writer too would cut a slow reader that half-closed.
- (i) Leave it: a half-closed, never-reading peer is the peer's own stall.
- (ii) Bound the writer at `CUT_GRACE` from `close`'s end, so a slow reader that half-closed loses the answers it has not read.
- Recommendation: (ii), since §5.2 promises nothing hangs past the grace on the other links.

**K5. TCP client's own `close` has no linger.** A client `close` can reset a server that is mid-write. Not probed. Apply the linger to the client, as J4 did for MQTT? Recommendation: probe first; apply if it reproduces.

## Behaviour the build added

**K6. Timer fallback differs by link.** TCP and UDP refuse `listen` on an app with no `Timer`; MQTT and NATS fall back to the link runtime's clock, because their direct tests call `listen` without `prepare`. Through `ulo_rpc::Server` the refusal is unreachable. Make all four refuse? Recommendation: refuse on all four, and give the direct tests a `Timer`.

**K7. MQTT: a publish after the client's `close` is refused.** Once the client's loop polls past its DISCONNECT, rumqttc would write a later publish behind it, so `closed` became a mutex checked under the publish. No deadlock, nothing held across an await. A planted break that releases the lock between the check and the queue survives, as a race. Recommendation: accept.

**K8. smol read loops yield every 32 frames (F485).** The server's `serve` loop starved the executor when a peer flooded frames, which would have blocked `close` and so the linger. The client's `read_replies` has the same shape, read, not probed. Recommendation: probe and patch the client loop in the same shape.

## Test gaps nobody pins

**K9. CI does not run the `integration`-gated tests inside the NATS and MQTT `link.rs` files.** The broker job runs `--test conformance` only. The NATS grace tests and the app-timer test are among them. This predates the round. Run the lib tests with the feature in the broker job? Recommendation: yes.

**K10. Unpinned breaks that stay.** The TCP linger loop's yield (both runtimes), the serve drop-path yield on tokio, MQTT's closed check outside the lock, the early NATS flush (K1), and the J6 guard restored. Accept as unpinned? Recommendation: accept.

## For the record

- **Gate:** see the commit message; the broker tests for MQTT and NATS ran against Mosquitto 2.0.18 and `nats:2.10.14`.
- **Linux:** not exercised locally for any link. The TCP lingering tests use std sockets and threads and run on ubuntu in CI. Linux's larger loopback buffers may come close to holding the 8 MiB those tests queue, weakening their premise; a missing linger is still caught there, because the peer's write fails on the reset. The MQTT relay's macOS reset workaround may do nothing on Linux.
- **Upstream requests to file:** async-nats (observe the last sender's drop while writes are held, or an explicit close); rumqttc (write-side shutdown). Neither is filed yet.
- **Test method:** 30 runs each of the timing-dependent tests, idle and under twelve busy loops on ten cores, 0 failures after the checker's patches (one UDP test failed 1 in 30 before patch 03).
