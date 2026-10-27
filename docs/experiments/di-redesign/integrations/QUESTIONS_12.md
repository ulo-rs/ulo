# Questions after the sixteenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the sixteenth response (K1 to K10) is built and checked by a second model. The check found five defects, closed: a flaky K5 test, a K9 CI step a cfg typo could not fail, a server-connection race against K4, a missing K10 note and a stale DESIGN sentence. The workspace runs 2179 tests with no failure. Each item below needs a ruling and carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F485, F486; transports `DESIGN.md` §5.2, §5.3.

Paste alongside: transports `DESIGN.md` §5.2 ("A cut `close`."), §5.3.

## Breaks no test catches

**L1. TCP: the "completed or cut" half of K4 has no test.** With the grace removed after a cut `close`, the whole TCP suite passes. Likewise, counting the grace from `close`'s start rather than its end, and `close` not waiting for the client writer to shut its half, survive. Pin them with half-close tests that cut `close`, or accept? Recommendation: pin the cut-close grace and the start-versus-end count; accept the writer wait.

**L2. The linger's discard-loop yield is unpinned on both runtimes.** K10 and F485 said the server flood test pins the yield; it pins only the frame-read yield. Pin it with a peer that floods bytes the linger discards, or accept? Recommendation: pin it, since a starved smol executor blocks `close` itself.

**L3. The smol listener's read budget can return `Pending` without a wakeup.** Removing the wake survives every test; read, it has no lost wakeup. Pin or accept? Recommendation: accept.

**L4. The WS yield is pinned weakly.** Removing it fails its flood test 4 of 10. Strengthen the test, or accept? Recommendation: accept; F486 records it.

**L5. smol starvation shows only as a hang in the lingering tests.** Those tests have no watchdog, so a break reads as a 600 s kill. Add a watchdog to each? Recommendation: yes, a bound on the test's own wait.

## Open and not exercised

**L6. F486's two unprobed paths:** the WS `refuse` loop on smol, and a client dropped without `close`. Probe both, or leave filed? Recommendation: probe them.

**L7. Linux.** Never run locally for any link. The checker probed an OrbStack kernel only: loopback buffers there are 2.6 MB send and 131 KB receive, so a 16× queue still exceeds them. The smol starvation tests and the MQTT relay's reset workaround have not run on Linux. CI's ubuntu job runs the TCP lingering tests; its result for this commit is pending.

**L8. The 250 ms timer test.** `a_call_s_timer_ends_with_its_bound_after_a_blocked_connect` failed twice for the builder at 201 to 204 ms and 0 of 240 for the checker, on HEAD and the build alike. Cause not established. Leave, or loosen the bound? Recommendation: leave until it fails in CI.

## For the record

- **Upstream requests:** async-nats and rumqttc are still unfiled; the user files them. DESIGN §5.2 links neither yet.
- **Kafka:** `NoTimer` compiles with `--features integration --tests`; not run against a broker locally.
- **Test method:** 30 runs each of the timing tests, idle and under twelve busy loops. The K5 test failed 6 of 100 on tcp before the checker's patch and 0 of 100 after.
