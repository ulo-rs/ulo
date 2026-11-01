# Questions after the eighteenth response is built and checked (for the designer)

Written for: the designer. Every ruling in the eighteenth response (M1 to M10) is built and checked by a second model. The check found eight defects, seven closed by patch and one (N1) needing a ruling. Each item below needs a ruling and carries a recommendation. Detail: `FRAMEWORK_GAPS.md` F486 to F488; core `DESIGN.md` §3.9; transports `DESIGN.md` §5.2, §5.3, §8.

Paste alongside: core `DESIGN.md` §3.9, transports `DESIGN.md` §5.3 and the Tokio bullet in §8.

## Where the contract needed a choice

**N1. The unit `ulo_runtime_tokio::Timer` still panics after its runtime shuts down (F487).** `Tokio::try_sleep` refuses as a value. The clock-only unit struct, which is not a `Runtime`, creates a `tokio::time::sleep` and panics at its poll once the driver is gone. That contradicts the `Timer` trait doc ("never panics"). Probed. Options: (i) delegate it to `Tokio::current()`, a task per sleep and a panic outside a runtime; (ii) narrow the trait doc: the unit struct is the ambient clock and carries no refusal. Recommendation: (ii), since the struct has no runtime to refuse for and (i) changes where it panics, not whether.

**N2. `sleep` on a shut-down runtime returns at once, so a loop that paces itself with it spins (F488).** Probed: a loop sleeping 1 s per turn on a dead `Tokio` ran 93,441 turns in 100 ms. The RPC client's `bounded` answers every call `Timeout` at once on a dead handed runtime, and Kafka's `coordinator_retried` loses its backoff (read). No in-tree loop spins unbounded. The ruling says `sleep` ends at once and never hangs. Keep that, or have `sleep` pend forever on a dead runtime, which a pacing loop survives and a bounded wait does not? Recommendation: keep; a call that never returns is the harder failure to see, and `try_sleep` is there for code that must tell the two apart.

**N3. Which `sleep(CUT_GRACE)` sites on a handed timer use `try_sleep`.** Patches moved MQTT's `linger` (a client timed on its handed runtime, M1's exact case), NATS's `keep_until_closed` and UDP's `drain_queued` to `try_sleep`, each with its own `debug` line for a refusal. Listed and not moved: Kafka's coordinator backoff (a backoff, not a grace), gRPC's and WS's `CLOSE_GRACE`, and the TCP accept loop's backoff. Move gRPC's and WS's? Recommendation: yes, they are graces on a handed timer like the three moved; leave the two backoffs.

## Behaviour the build added

**N4. Cost of `Tokio::try_sleep`: a task per polled sleep.** Probed in release: creating, polling once and dropping a sleep that is armed and then beaten costs 636 ns where raw tokio costs 51 ns (4 workers), about 1.2 µs per request on a multi-thread runtime across hyper's header timer, the per-request deadline and the RPC bounds. Aborted sleep tasks pile up until a worker runs them: 200,000 alive on a current-thread runtime, drained in 45 ms. A sentinel task per `Tokio` value was rejected because three `dropped.rs` tests assert `num_alive_tasks`. Accept? Recommendation: accept; the alternative, catching the poll-time panic, depends on a message string.

**N5. Wrappers that forward `sleep` and `spawn` but not the new methods drop the refusal without a sign.** `ulo-db-conformance`'s `Unnamed` and `Smol`, the tests' `Handed` and `Holding`. None is on a path that needs it today. Make `try_sleep` and `try_spawn` required methods, so a wrapper that forgets them fails to compile? Recommendation: yes for the non-test wrappers; the defaults stay for test doubles only if a `#[cfg(test)]` marker can say so, otherwise drop the defaults everywhere and let the 34 test impls say `Ok`.

**N6. The `Lost` versus `Dropped` line is placed (M6b).** Patch 07 puts the comment at the send site. The case is internal by the ruling's test, but peer-visible: as `Lost`, a peer still writing would read a reset. Confirm the wording. Recommendation: confirm.

## Test gaps

**N7. The smol harness now declares the shut-down scenario not applicable.** The runtime suite stamps seventeen scenarios; tokio runs all, smol runs sixteen and reports one ignored with its reason. A `None` from `stoppable` without that declaration fails. Confirm. Recommendation: confirm.

**N8. `try_spawn` reports a task a shutdown cut part-way as refused.** Read, not probed: the docs' "dropped unrun" is not strictly true there. Accept with the doc corrected, or probe? Recommendation: correct the doc.

**N9. The NATS `keep_until_closed` change was not run.** Its tests need a broker; the broker job runs them on CI.

## For the record

- **Checks:** 2209 tests pass before the patches; the new and moved tests ran 60 of 60 (30 idle, 30 under twelve busy loops) and passed on an OrbStack Linux kernel; about twenty planted breaks each fail by test name. The server cut-grace test missed an early release of its hold, now pinned.
- **CI:** the run for `0ab2a886` and `446d1d8f` is green; `71404b45` has no run of its own.
- **Upstream requests:** async-nats and rumqttc are unfiled; the user files them, from the texts in memory.
