# Questions after the fourteenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the fourteenth response (I1 to I7, H8) is built, checked by a second model and on `experiment/di-redesign` at `84e37162`. The workspace runs 2150 tests with no failure. The check found one behaviour defect, closed (MQTT acknowledged requests after its own DISCONNECT) and one open gap in a link the ruling named (the TCP server's close, F484). Each item below needs a ruling. Each carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F479, F482 to F484; transports `DESIGN.md` §5.2, §5.3.

Paste alongside: transports `DESIGN.md` §5.2 ("A cut `close`."), §5.3.

## Where the build departed from a ruling

**J1. `close` waits for room for its own answers.** I1's test (a stream blocked on a full channel while `close` runs, one terminal frame, a clean disconnect) could pass only once `close`'s own `unavailable` answers were allowed to wait: a frame already waiting always wins the room against `close`'s `try_`. Built: `close` queues what its first poll could not, in order, once there is room; no other sender can get in, because all find `closed` set. A `close` cut before there is room loses those answers, and on MQTT its DISCONNECT too. The ruling condition holds (no frame reaches the channel after `close`; exactly one terminal frame; the broker reads a DISCONNECT with no PUBLISH after it). Confirm. Recommendation: confirm; the alternative is a reserved slot for `close`'s answers, which costs capacity on every link to cover a cut.

**J2. I2's pin ("the socket closes by the grace") cannot hold on async-nats 0.46.** The connection task does not poll its command channel while its write buffer is over 65,535 bytes, so the last `Client` dropping goes unseen while writes are held. Through a relay that stopped reading, the keep task let go at the grace and the socket was still open 400 s after the cut. The pin built asserts the task lets go at the grace (and on a lost connection); a second test asserts the socket outlives the task, which documents the library's limit and fails if async-nats changes. §5.2/§5.3 state it. Accept? Recommendation: accept.

## What a link cannot do

**J3. MQTT: a PINGREQ can follow the DISCONNECT inside the 2 s window.** MQTT 5 §3.14.4 allows nothing after a DISCONNECT. rumqttc 0.25.1 has no write-side shutdown (`EventLoop::network` is private), so a keep-alive firing inside the lingering window writes one (probed: a `close` 9.2 s after connect drew a PINGREQ behind the DISCONNECT). The PUBACKs are closed by setting `manual_acks` after the DISCONNECT. Setting `await_pingresp = true` stops the PINGREQ, but the keep-alive then ends the loop and drops the socket with the broker's packets unread: the reset the linger exists to avoid.
- (i) Accept the PINGREQ: it needs the keep-alive to expire inside a 2 s window, and the broker reads a protocol-breaking packet after a clean DISCONNECT.
- (ii) `await_pingresp = true`: no packet after the DISCONNECT; the reset returns when the keep-alive fires inside the window.
- Recommendation: (i), stated in §5.2, and an upstream request for a write-side shutdown on rumqttc.

**J4. The MQTT client side has the I3 shape and showed no failure.** The client's event loop ends at its DISCONNECT and drops the socket at once. `client_close_writes_what_was_queued_before_it` passed 60 of 60, with and without load. A client-side `close` is the caller's own, so the reset costs the caller its own queued frames. Apply the linger to the client, or leave? Recommendation: leave, and record it as untested under a producer that outpaces the broker.

## F484: the TCP server's close

**J5. The TCP RPC server's `close` resets a peer that is still writing.** I3 asked for a check of the TCP link for the drop-after-write shape; the build did not do it and the check did. Probed on macOS: a peer idle at `close` reads a clean FIN (3 of 3); a peer still writing during `close` reads `ConnectionReset` (3 of 3). §5.3 says the peer reads a FIN. Options: (i) the lingering close for TCP (shut the write half, read and discard to EOF under `CUT_GRACE`), as for MQTT; (ii) amend §5.3 to say the peer reads a FIN, or a reset if it is still writing. Recommendation: (i), because the TCP link is the reference RPC transport and a reset discards answers the peer has not read. Probed on macOS only; Linux is not exercised.

## Smaller

**J6. The close-order guard (patch 05, not applied).** `close` stops trying further answers after one is refused. It affects only order across calls, which nothing promises. The check offers deleting it on both links with the wording "from the first refusal on". Delete? Recommendation: delete.

**J7. Unpinned breaks that stay.** `opened`'s `closed` check is unreachable (`on_publish` holds the `deliveries` guard across it and `close` takes `deliveries` before it sets `closed`); the ruling asked for the check and no test can kill it. Also unpinned: creating the `Notified` after the retry rather than before (a real window, survives every test); the event loop's turn given after `close`'s waiting remainder rather than before; the NATS keep task flushing before `close` ends. Delete the unreachable check, or keep it as a guard? Recommendation: keep `opened`'s check with a comment saying which lock makes it unreachable; accept the other three as unpinned.

**J8. The cut-close NATS test is not deterministic.** F482 claimed `a_cut_close_writes_what_was_queued_once_the_socket_takes_writes_again` fails when the keep task lets go at once; with that break planted it failed 1 of 4 runs. The grace test's timing catches it every time. Amend F482 and leave the test, or make it deterministic with a held socket? Recommendation: amend F482.

## For the record

- **Gate and rates:** 1,320 timing runs, with and without twelve busy loops on ten cores, 0 failures: the MQTT and NATS spent-cap stamps on both runtimes, the I1 blocked-stream tests, the MQTT grace test, the NATS waited-terminal and cut-close tests, gRPC `Burst`, the connect-time test, the UDP drain test. The MQTT worker-pool stamp is required again (30 of 30 under load). The stamp counts alone do not separate the built link from HEAD (HEAD failed the worker-pool stamp 2 of 120 in one session, 19 of 30 in another); the relay grace test is the deterministic pin.
- **I3 cause:** a broker-side capture in a container sharing Mosquitto's network namespace. In the one failing run the broker received the PUBLISH and DISCONNECT, wrote UNSUBACKs the link never read, and the connection ended with a FIN and a reset 14 µs apart; passing runs show the broker's PUBACK and FIN first. The link now keeps polling past the DISCONNECT, bounded by `CUT_GRACE` after `close` ends.
- **I7:** the connect-time bound reads the clock on both sides of the forward and asserts the gap lies between the two readings, less one `grpc-timeout` unit. No scheduling margin remains; 60 of 60.
- **UDP:** the drain after a drop is bounded by a local `CUT_GRACE` through `tokio::time` (the writer has no `Timer` handle; MQTT and NATS do the same). Its test runs on a paused clock and asserts the bound.
- **Not exercised:** Linux for the TCP probe; Mosquitto versions other than 2.0.18; an MQTT producer that outpaces the broker on the client side; async-nats other than 0.46.
