# Questions after the twentieth response is built and checked (for the designer)

Written for: the designer. Every ruling in the twentieth response (O1 to O6) is built and checked by a second model. The check found one behaviour defect, old, which is the part of the new §3.9 rule that the build did not reach (P1), and six refinements, listed after it and not batched. The workspace runs 2243 tests with no failure. Detail: `FRAMEWORK_GAPS.md` F489 to F492; core `DESIGN.md` §3.9, §9.3, §10.2.

Paste alongside: core `DESIGN.md` §3.9 (the rule), §9.3 (the readiness clause) and §10.2 (`FailureReason`).

## The behaviour defect

**P1. The core's own bounds run on `sleep`, so a hook, a construction or a readiness check on a refusing clock reports `TimedOut` though no time passed (F491).** §3.9's rule ("a bound whose expiry is reported as a reason someone reads, a close code, an error a deployer sees or a hook's outcome, reports a refusal as a refusal") covers them. The build reached the sites outside the core and left the core's: probed, 5 s bounds fail at once as "timed out after 5s" for a hook, a constructor and a readiness check, and the shutdown cap and the transport close bound likewise. It also hides this round's `migrate_within` work: on a clock refusing every length the start reads "OnModuleInit hook ... timed out after 94s", retryable, instead of the `Permanent` `RuntimeShutDown` the module docs promise.

The fix is not mechanical. `timer::timeout` and `run::run` change; `Outcome` gains a refusal arm with a tie rule (an answer ready in the same poll wins, as `bounded` already does); seven `Outcome::TimedOut` to `FailureReason` sites change (`lifecycle/connect.rs` 74, 111, 139 and 159, `shutdown.rs` 354 and `bounded_close` 504, `app/shared.rs` 271). One test written last round, a refused backoff after a timed-out attempt, relies on today's path and would change. The shape is yours:
- (i) `FailureReason::Errored` holding a `Permanent` `RuntimeShutDown`, the type the other sites already use. No change to §10.2's enum; `is_permanent` already classifies it.
- (ii) A new `FailureReason` variant, `RuntimeShutDown`. A change to §10.2, and a second place that classifies a refusal.
- Recommendation: (i). One type already means "the runtime refused" at every other site, and a deployer matching on it finds the hook, the readiness check and the lock wait alike.

## Listed, not batched

Refinements, each with the reason it stays out of a round. None meets "a behaviour defect".

- **R1. WS has no counterpart of the RPC bound's "deadline had already passed" check.** A refusal arriving after an outstanding Pong was already due ends `RuntimeShutDown`, not `Lost`. A patch exists (records the due time, with a test) and conflicts with O2's "never as Lost", so it is your call whether `Lost` is right when the Pong was in fact late. The same check is missing at the other three converted bounds. Not batched: the outcome is already a close the client can read.
- **R2. A refusal with messages in flight closes at once and drops the answers still to come**, where a drain would wait for them. Neither this precedence nor the gateway-close precedence (a gateway's own close outranks a refusal) has a test. Not batched: a runtime that refuses cannot time the drain anyway.
- **R4. NATS answers one condition two ways.** A link whose own runtime died and which holds the roots load itself answers with the old `STOPPED` string (`Unavailable`, no reason); only a link joined to a load still alive on another runtime answers `runtime_shut_down`. Not batched: both end the call `Unavailable`.
- **R5. NATS `prepare` and `listen` raise `RuntimeShutDown` without `Permanent`,** against the type's own doc. `Permanent` means "a restart cannot help", and a runtime that shut down at startup is usually fixed by one. Not batched: the doc sentence needs to change, not the behaviour; say which you want.
- **Three confirmations from the build** (WS): a refusal closes at once and does not wait for in-flight messages; a refusal while a close is already under way only drops the clocks and the existing reason stands; the wait for the client's Close afterwards stays on `sleep`, so on a really dead runtime the connection ends right after it sends its Close frame. Say if any is wrong.

## For the record

- **Checks:** 2243 tests pass; every new test ran 30 of 30 idle and 30 of 30 under twelve busy loops (and 24); on an OrbStack Linux kernel all new tests pass; 28 planted breaks, 25 failed by name, the three missed were two precedence cases (R2) and one graphql-ws case now pinned by a test.
- **Beyond the brief:** ulo-db's `migrate_within` moved to the reporting form, because its expiry is an error a deployer reads. The check kept it. The shared `ulo-rpc` `connect_error` now maps a `RuntimeShutDown` connect error to `Unavailable` with `runtime_shut_down`; only NATS produces one.
- **Upstream requests:** async-nats and rumqttc are unfiled; the user files them from the texts in memory, and §5.2 links them in the same commit.
- **What follows:** once P1 is built, the example catalogue, then the docs-site pass, then the merge.
