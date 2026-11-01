# Questions after the seventeenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the seventeenth response (L1 to L8) is built and checked by a second model, and is on `experiment/di-redesign` at `71404b45`. The workspace runs 2199 tests with no failure. The check found nine defects: seven closed by patch, one a design question (M1), one a duplication (M5). Each item below needs a ruling and carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F485, F486; transports `DESIGN.md` §5.2, §5.3.

Paste alongside: transports `DESIGN.md` §5.2 ("A cut `close`."), §5.3.

## Where the build departed from a ruling

**M1. A client dropped with a dead runtime loses the linger and panics.** L6's rule is that after a client is dropped without `close`, nothing it owned outlives the drop plus `CUT_GRACE`. The probe found a real reset and the fix is built: the link's drop advances the client epoch, and a dropped reply lane hands its reader to the linger with the grace counted from the drop. One case remains. A client handed a runtime that has shut down, while the link's own runtime is alive, calls `timer.sleep(CUT_GRACE)` on the dead runtime when its reply lane drops. tokio panics in the connection task, the linger is lost and the server reads a broken pipe. Probed on tokio. Options: (i) accept: a client built on a runtime it then shuts down has no clock to bound a linger with; (ii) when the handed runtime cannot spawn or time, drop the connection at once and say so in §5.3. Recommendation: (ii), since a panic in a connection task is the worse of the two.

**M2. The end-versus-start pin also pins the client writer wait.** L1 said to leave the writer wait unpinned because the shut lands inside the grace either way. Both parts of that are false. On a TCP client the writer wait is the only thing that holds `close` open, so the end-versus-start test cannot be built without it. And the peer does read differently: with the writer held past `CUT_GRACE` and the wait removed, the server read 6 requests and then a reset; with the wait it read every request and the FIN. The comment at `Core::close` now says so. Confirm the pin stays? Recommendation: confirm.

## Behaviour the build added

**M3. The WebSocket `refuse` loop yields.** The probe found it starving a smol executor under a Pong flood: a 17,980-byte gap between ticker turns, for up to `pong_timeout` (30 s by default). It yields every `TURNS_PER_YIELD` turns now, and `yield_smol.rs` pins it. Predates the round. Confirm. Recommendation: confirm.

**M4. Four starvation-test properties the checker tightened.** Gap limits are one budget's worth, where the builder's were 2× to 4× (a yield constant of 64 passed all four cases before). The WS case resolves only to 64, since its reads are 60 bytes: a constant of 48 still passes there. The flood thresholds are 8 MiB, because a loopback connection with nothing reading holds 2.69 MB on Linux and 1.68 MB on macOS. The harness carries five self-tests, each of five harness mutations failing at least one. Recommendation: accept the WS resolution as stated.

## Test gaps nobody pins

**M5. Four test files define their own scripted clock.** `ulo-conformance-support::ScriptedTimer` lacks `pending_at`, which they use. Move them onto the shared clock with that method? Recommendation: yes.

**M6. Unpinned breaks that stay.** Mapping a failed send to the reply lane as `Lost` rather than `Dropped` (a multi-thread race, 5 of 150 failures per runtime with the break, 0 of 150 without); the server's grace counted from `close`'s start (the cut-close test cuts after one poll, where start and end coincide). Accept, or add a held-call server-side test as the client has? Recommendation: add the server-side one, since K4's "whole connection" claim is about the server.

**M7. F485's client loop.** On a one-thread executor the client's reply reader cannot starve, because its 64-slot reply queue makes it wait for the consumer. The multi-worker case is untested. Probe it? Recommendation: accept; a multi-worker executor yields between workers.

## Open and not exercised

**M8. DESIGN has no text on the read budget or the smol yields.** §3.7 and the `fw-listener-smol` row name neither the listener's read budget nor the WS and TCP yields. Add a paragraph to §3.7 stating the rule (a loop that never awaits a pending read yields every 32 turns on smol, tokio's budget doing it there) and the pins? Recommendation: yes, one paragraph.

**M9. The timer test.** `a_call_s_timer_ends_with_its_bound_after_a_blocked_connect` could not have failed early as described: it asserts every outcome is `Timeout` and each call within 250 ms, so a 201 to 204 ms failure is a call ending with another kind. The test's std clock and the runtime's differ by +0.000 ms. 0 of 180 failures. On failure it now prints both clocks, the connect's start and return, and the full error. Bound unchanged. Leave until it fails? Recommendation: leave.

**M10. Linux.** CI's result for the TCP lingering tests on `fdcd5a5a` and `5ed48ecf` is green. The seventeenth round's new tests ran on an OrbStack kernel, 5 of 5; CI's ubuntu job is pending for `71404b45`, and the MQTT relay workaround's ubuntu result for K9 is green.

## For the record

- **Upstream requests:** async-nats and rumqttc are unfiled; the user files them. The designer offered to draft the issue texts; yes, with the probe evidence.
- **Kafka:** the `NoTimer` test passes locally and runs in the broker job through `require_tests.py`.
- **Test method:** the new starvation and timing tests ran 30 times idle and 30 under twelve busy loops on ten cores, 0 failures; the checker's planted breaks, about 60, each failed by the starved loop's name and none hung.
