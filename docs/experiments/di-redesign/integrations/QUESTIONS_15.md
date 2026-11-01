# Questions after the nineteenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the nineteenth response (N1 to N9) is built across two stages and checked by a second model. The check found nine defects: six closed by patch, three needing a ruling (O1 to O3). Each item below carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F487 to F490; core `DESIGN.md` §3.9 and §9.3; transports `DESIGN.md` §5.2, §8.

Paste alongside: core `DESIGN.md` §3.9 (the rule table), transports `DESIGN.md` §8 (the Tokio, Timer and Conformance bullets).

## Where the refinement stops

**O1. The bound-claims-time refinement is built at one site (F490).** Your N2 refinement ("a timeout claims that time passed: report the refusal") named the RPC client's `bounded`, and that is the one site built: a dead handed runtime answers `Unavailable` with the reason `runtime_shut_down`, and `Timeout` when the deadline had in fact passed before the refusal (the check's D4: a runtime can shut down after the deadline and before the timer task fires). Other bounds still run on `sleep` and report a refusal as an elapsed bound: graphql-ws's `connection_init` timeout (closes 4408 "init timeout" only when the app's timer and runtime are different runtimes and only the timer's has shut down); NATS `roots_timeout` (only when this link joined a process-wide load another link's runtime still runs); the execution, HTTP, gRPC and RPC per-call deadlines; WS `pong_timeout`; the hyper handshake timeout; `ulo-db` `connect_within`; the health check bound. Core §3.9 now says `bounded` is the one built site and the rest stay on `sleep`. Should the refinement reach the rest? Recommendation: reach the three that report a protocol-visible claim (graphql-ws 4408, NATS `roots_timeout`, `connect_within`, whose error text a deployer reads), and leave the per-call deadlines and the handshake timeout, where an early end already reads as "too late" and the caller learns nothing more from a reason.

**O2. A WebSocket connection whose runtime refuses a ping tick has no liveness check afterwards (F489).** The ticker stops and the connection serves on. With no pong outstanding nothing ever times it out; with one outstanding, `pong_timeout` is a plain `sleep` that a dead runtime ends at once, so the connection ends `Lost` for a missed pong when no time passed. The test sets `pong_timeout = Unbounded` and cannot see either. Options: (i) a refused tick ends the connection, as a refused backoff ends a loop (the connection cannot be kept alive by a clock that no longer runs); (ii) keep serving and accept no liveness check once the runtime is gone. Recommendation: (i). A connection whose runtime refuses has no way to tell a dead peer from a live one, and ending it as `Lost` is what `pong_timeout` would have said.

## Smaller

**O3. An empty `clocks` list in a harness with a stop is now a failure; an explicit stamp is the alternative.** The check's patch 01 makes the shut-down scenario fail when `Stoppable::clocks` is empty, so a runtime crate whose runtime is its only clock lists a second value of it. An explicit `Clocks::None { reason }` stamp would instead let a crate say why. Recommendation: keep the failure; a crate with a stop has a clock, and the stamp invites skipping it.

**O4. `ACCEPT_BACKOFF` is a public constant in two crates.** `ulo-rpc-tcp-core` and `ulo-server-hyper` export it, mirroring the already-public `CUT_GRACE`, so the pacing tests can read it. `#[doc(hidden)]` is the alternative. Recommendation: leave public; a deployer reading the drain can see it.

**O5. The four graces have a test pattern now, and one grace has a test.** HealthModule's lag is pinned by a `tracing` subscriber, a `ScriptedTimer` refusing the grace's length and an assertion on the refusal count and the log line (`crates/ulo-health/tests/lag_refused.rs`). gRPC's and WS's `CLOSE_GRACE` and the HTTP backend's `FLUSH` have none. Their behaviour equals the old `sleep`; only the `debug` line differs. Extend the pattern to the three? Recommendation: no, accept the line as unpinned.

**O6. Core logging.** §3.9 said "nothing else in the core logs", false before this round (a `warn` in `app/mod.rs`) and now also false for the readiness backoff's `debug`. The sentence now lists the two logs it names. Confirm that a refused readiness backoff logs. Recommendation: confirm.

## Behaviour the build added

- **A refused backoff in a readiness check or the `ulo-db` create loop can leave `TimedOut { limit: Attempt }` with retries left.** Core §9.3's reason rules do not list that case; the check now ends at once with the last attempt's reason. Recommendation: accept, and state it in §9.3 (done, in one clause).
- **`bounded` yields once on a refusal before ending the call**, so work that is ready in the same poll keeps its answer. Without the yield an unbiased `select!` discards a ready answer half the time. On a live runtime nothing changes: no yield, no extra poll.

## For the record

- **Checks:** 2225 tests pass in the checker's snapshot; every new test ran 30 of 30 idle and 30 of 30 under twelve busy loops on macOS; on Linux (OrbStack) the smol `lane_send_failure` copy flaked 3 in 40 until it waited through the test's own polling gate (60 of 60 and 60 of 60 after, the `Lost` plant still failing it). 25 planted breaks failed by test name; three were missed and are closed or accepted (O3, O5).
- **Upstream requests:** async-nats and rumqttc are unfiled; the user files them from the texts in memory.
