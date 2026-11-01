# Questions after the twelfth response is built and checked (for the designer)

Written for: the designer. Every ruling in the twelfth response (G1 to G16) is built, checked by a second model and on `experiment/di-redesign` at `3ca125ad`. The workspace runs 2136 tests with no failure. The checks found no behaviour defect in the stream and deadline work and closed 11 gaps in tests and wording. The builds themselves found five defects, all closed: a gRPC reply stream that ignored the drain (F476), an RPC stream that ran past its deadline (F475), an HTTP body that polled the drain before the body, a UDP link that discarded a queued reply when it dropped (F478), and MQTT losing the reply on a one-thread runtime (F479, one half). Each item below needs a ruling. Each carries a recommendation. Detail: `CORE_TEST_PLAN.md` (the twelfth-response status sections), `FRAMEWORK_GAPS.md` F475 to F481.

Paste alongside: core `DESIGN.md` §3.9; transports `DESIGN.md` §2.6, §5.2.

## A rule that did not settle

**H1. F479: on a worker pool, MQTT cannot both keep E5 and deliver a held call's answer under a spent cap.** The new `drain::spent_cap` scenario passes on UDP, TCP, NATS, Redis, RabbitMQ and Kafka. On MQTT the one-thread copy now passes: the event loop is spawned through `Turns`, and `shut` gives it a turn once the DISCONNECT is queued, so the reply and the DISCONNECT are written in `close`'s first poll before the guard aborts. The worker-pool copy fails 9 runs of 10. On another worker the held call's `unavailable` is queued about 0.2 ms after `shut` queued the DISCONNECT (G3's `give` skips a poll that is under way), and MQTT allows no packet after DISCONNECT. Probed against Mosquitto 2.0.18. No change inside the link both keeps the DISCONNECT in `close` and writes that reply.
- (i) Let `give` wait for a poll under way. This reverses G3 and reopens the inline-close deadlock.
- (ii) A cut `close` skips the DISCONNECT and keeps the loop until the link drops, as UDP's writer does (F478). The connection outlives the cut until the link drops, against E5's "nothing hangs past the cut", and the broker sees an abnormal disconnect. The window shrinks; the race stays.
- (iii) Accept it. The worker-pool stamp is marked `not_applicable` on MQTT with this reason, which is the state on the branch now.
- Recommendation: (iii), with the cause stated in transports §5.2. The one-thread stamp pins the first-poll write, and a held call's answer on a worker pool under a spent cap is already best-effort on a link whose protocol forbids a packet after DISCONNECT.

## Where a rule reached further than its ruling

**H2. The pipeline-first rule now covers deadlines.** The ruling named cancellation. Without the extension an always-ready RPC stream never saw its deadline (F475), and a gRPC `CallBody` polled its deadline before the body. A deadline is a `CancelReason`, so §2.6 now says "a cancellation or a deadline". Confirm.

**H3. A unary gRPC reply whose data frame is ready in the deadline's poll is now sent, then followed by `DEADLINE_EXCEEDED`.** Before, the reply was trailers-only `DEADLINE_EXCEEDED`. The check noted that §2.6's "what the pipeline has ready wins" could also be read as sending OK. Which reading is the rule? Recommendation: the pipeline's answer wins, so a ready unary reply is an `OK`, and `CallBody::expired` ends the call without a trailing deadline status. The change is small and no test depends on the current form.

## Findings from reading the build

**H4. `AbortOnDrop::release_unserved` is dead on the path that reaches it (F480).** `drain()`, in the same first poll, always releases the listeners first, so removing only the guard's release stays green. The rebind test pins the outcome through the whole path. Delete the guard's release, or keep it as a second line of defence with a comment saying which path reaches it?

**H5. gRPC's HTTP/2 builder used hyper's own timer (F481).** Nothing slept on it, so no behaviour changed; it now takes the app's `Timer`, and the `hyper-util` dependency is gone from `ulo-grpc`. Recorded because §3.9 says a wait goes through the app's timer, and the HTTP/2 builder was the one place that did not.

**H6. A rustc late lint does not run after an error.** The client-less `ulo-otel-otlp` case asserts one `compile_error!` and nothing after it, but rustc runs no late lint once an error is emitted, so an ungated module that would warn is invisible to the test. A secondary error is caught, a secondary warning is not. State in the test's comment that it guards errors only, or add a `cargo check` of the same crate with a lint denied?

## Interfaces and shapes the build chose

**H7. The G14 case is a nested `cargo check` with its own snapshot, not a trybuild case.** trybuild cannot build a crate that is not a dependency with its features off, and a dependency that fails to build by design cannot be a dev-dependency. The test runs `cargo check -p ulo-otel-otlp --no-default-features`, strips cargo's progress lines, and compares the rest to `ui_features/otlp_without_a_client.stderr`. It runs only on rustc 1.98.1, like the other snapshots; on a cold target directory it compiles the OpenTelemetry dependency tree in check mode. Accept the form?

**H8. The spent-cap scenario is stamped twice.** One copy runs on the harness runtime, the other on `block_on_one_thread`. On a worker pool another worker runs the cancelled call before the cap drops `close`, so a missing first-poll turn stays green on TCP and UDP there; only the one-thread copy pins it. The suite's "never message text" rule now names the shutdown phrase as the one exception, because the client's own `Unavailable` for a closed connection has the same kind and no reason. Confirm the exception and the two stamps.

**H9. The all-four-transports assertion stays in `cross_transport.rs`.** G10 asked to move the RPC scenario into `ulo-rpc-conformance`; the HTTP, WebSocket, RPC and gRPC renderings and the close order are asserted together there, so only the RPC half moved. Confirm.

## For the record

- **G2 answered:** HTTP/2 flow control holds a gRPC `close`: a client with 64 KiB windows that reads none of 400 000 numbers held it 0.70 s against a 200 ms window; a reply that fits the window ended in 1.5 ms. The probe is in the four-server concurrent-close test beside the WebSocket pair.
- **G11 answered:** under `shutdown_timeout(ZERO)` an open health `Watch("")` reads `NOT_SERVING`, 20 of 20 on one thread and on four workers. The E3 first-poll step is the cause (F477), and the older B4 note is corrected.
- **Not exercised this round:** the spent-cap stamps on the brokers other than MQTT ran 5 of 5 once each, in a scratch copy; MQTT on Linux; Mosquitto versions other than 2.0.18; the TCP CBOR and TLS stamps under load; the MySQL client-certificate pin.
- **Process tally:** the checker planted breaks against every new test; 11 gaps in tests and wording were closed by new tests or rewording, and one reproduction (F479) was fixed in part.
