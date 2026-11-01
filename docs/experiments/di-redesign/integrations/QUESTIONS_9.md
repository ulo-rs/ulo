# Questions after the thirteenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the thirteenth response (H1 to H9) is built, checked by a second model and on `experiment/di-redesign` at `009e3861`. The workspace runs 2143 tests; the one that failed in the full run is a timing test the batch does not touch (`clients.rs`, a 1 ms margin missed by 0.7 ms on a loaded machine) and passed 5 of 5 on rerun. The check found one behaviour defect, closed: the H1 lock dropped a reply frame that found the link's channel full, which cut a streamed reply short on MQTT (at 65,536 items) and on NATS (at 2,048 on one thread). Each item below needs a ruling. Each carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F479, F481 to F483; transports `DESIGN.md` §5.3.

Paste alongside: transports `DESIGN.md` §2.6, §5.2, §5.3.

## Where a ruling collided with a behaviour

**I1. A reply that finds the channel full waits for room; it is not lost.** H1's wording read as "a frame the full channel refuses ends its call". Built that way, a stream whose items are always ready fills the channel before the connection empties it, and the caller read a `Timeout` after 65,536 items (MQTT) or 2,048 (NATS, one thread). HEAD delivered every item. Built now: the first `try_` publish runs under the lock; on a full channel the send releases the lock and awaits the publish inside its future. The call stays unanswered meanwhile, so a `close` in that window still answers it `unavailable`. On NATS the first try runs inside tokio's cooperative budget, because without it a one-thread run lost 28,128 of 100,000 items. Confirm. Recommendation: confirm, and amend H1's sentence.

## Where a link cannot keep E5

**I2. NATS: a task that holds a `Client` past a cut `close`.** async-nats's connection task exits once no sender is left, without writing commands it has already taken: publish then drop the last `Client` lost 40 of 40, and 0 of 300 with a clone held until `Closed`/`Disconnected`. The lock alone left 2 failures in 100 stamp runs; lock plus the task, 0 in 100 and 0 in 40 under CPU load. The task queues the drain if `close` was cut, and is not spawned when the connection is already down. After a cut close the socket closed 15 to 17 ms later (3 of 3, also with the broker paused); the roughly three-minute bound applies only when the send buffer is full. This is UDP's writer again (F478) and runs against E5's "nothing hangs past the cut". Recommendation: accept, with the bound stated in §5.2.

**I3. MQTT: the held call's answer can be lost after a cut `close` (F479).** Without load the worker-pool stamp passed 28 of 30 and the one-thread stamp 30 of 30. Under CPU load (twelve busy loops on ten cores) the worker-pool stamp passed 11 of 30 and the one-thread stamp 9 of 30; HEAD passed 8 and 7. Every failure is the caller's `Timeout`. In an instrumented failing run the link wrote the PUBLISH and the DISCONNECT in `close`'s first poll. A scratch variant that kept the loop alive until the broker closed the connection (bounded at 2 s) passed 8 of 8; reading what had already arrived before the drop passed 2 of 8. Cause not established; the guess is that dropping the socket right after the write loses the frames before the broker reads them. The worker-pool stamp is `not_applicable` again.
- (i) Accept: keep the stamp `not_applicable`, state the cause in §5.2.
- (ii) Keep the MQTT loop past a cut `close` until the broker closes, bounded at 2 s, as I2 does for NATS. Passes the stamp, and runs against E5.
- Recommendation: (ii) if I2 is accepted, since the same ruling then applies on both links; otherwise (i).

## Behaviour the build added

**I4. Two publishes on MQTT bypass the lock.** `refuse` and `opened` publish without taking it, so a refusal queued after the DISCONNECT is never written. This predates the batch. Recommendation: route them through the lock.

**I5. NATS: a request lane's `hold` racing `close` on another worker could hold a call after the replies close.** Its caller would then time out. Read, not probed. Recommendation: probe it with a test that holds a call while `close` runs on another worker.

## Test gaps nobody pins

**I6.** An unmarked gRPC body that yields a second data frame after the deadline is unpinned. Six NATS planted breaks survived both link tests (unsorted close answers, no bookkeeping on a full channel, the keep task dropping its `Client` right after the drain, `borrow_and_update` removed, `close` dropping its sender instead of signalling it, a wrong frame id in `close`'s `err`, which is harmless because NATS routes by subject). The keep task's `Client` drop survives because the connection task, woken first, writes before the task runs. Pin them, or accept?

**I7. `clients.rs::a_new_connection_gives_the_server_the_connect_beyond_the_caller_s_deadline_and_no_more` has a 1 ms margin.** It missed by 0.7 ms in one full-workspace run (round 0: the server's deadline +259.084 ms against a connect of 252.586 ms and 4.856 ms in transit). Widen the margin, or leave?

## For the record

- **H3 as ruled:** a ready unary gRPC reply in the deadline's poll is `OK` with no trailing deadline status. Pinned in both orders; the timing-based OK-order test passed 20 of 20 with and without load.
- **H4 as ruled:** the guard's `release_unserved` is deleted; a dropped unserved `Serve` releases its listeners through its own drop, pinned by a new test.
- **H8:** `ulo_transport::SHUTDOWN_MESSAGE` is exported and used by the transports and the conformance scenario. Literal copies of the phrase remain in several test files.
- **Other links:** the spent-cap worker-pool stamp passed 20 of 20 on Redis, RabbitMQ and Kafka.
- **Not exercised:** MQTT on Linux, Mosquitto versions other than 2.0.18, NATS servers other than the container image CI uses.
