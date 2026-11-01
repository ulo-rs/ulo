# Divergences: race 3, area H1 (`ulo-health`, `ulo-health-http`)

Every place H1 departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and fifth
responses, or fills a shape they leave open. Each entry gives what the design says, what was
written, and why. All await the user's sign-off.

No cargo was run. Every claim about what compiles is read from the sources and the registry; the
items a compile would have to show are listed under "Not verified".

Files written: `crates/ulo-health/Cargo.toml`, `src/lib.rs`, `src/check.rs`, `src/run.rs`,
`src/readiness.rs`, `src/module.rs`; `crates/ulo-health-http/Cargo.toml`, `src/lib.rs`,
`src/routes.rs`, `src/body.rs`. No public signature of the skeleton changed.

## Points the plan leaves to H1

P1. **What `Readiness` reads to know the app is `Running`: its own state, not the app's phase.**
The core binds no `AppHandle` for injection, so `AppHandle::phase()` is out of the health module's
reach. `Readiness` answers from its checks from the moment the binding is built until its
before-shutdown hook records `Stopping` (`Readiness::stop`, an `AtomicBool`). Two consequences:

- Between the core advancing to `Stopping` and health's hook running, while the hooks ordered
  ahead of it run (a lazily loaded module's, and a singleton built after the health binding),
  readiness still answers from the checks. DESIGN §3.2 states the order as "before every `Check`'s
  hooks", which this meets.
- `Readiness` does not observe `listen`. Before `listen` it answers from its checks; that no probe
  sees this holds by construction, as DESIGN §3.2 says. The coordinator's unit test for "readiness
  false before `listen`" can assert the construction (an embedding answering 503 while `Unbound`),
  not `Readiness::readiness()` called before `listen`, which answers up with every check up.

P2. **How the gRPC bridge is told of a change: `Readiness::subscribe() -> Changes`, whose
`changed()` resolves at `Stopping`.** One notice per subscriber per bump of the generation; a
subscription taken after `Stopping` resolves at once, its `seen` starting at 0. A check going down
is not a change it reports, since no result is kept between runs; the bridge sees it by re-running
`readiness()` on its interval (H2). The wakers sit in a registry on `State`, one entry per
subscriber, replaced on re-poll and removed by `Changes`' `Drop`, so nothing grows with polls.
`stop` bumps the generation before taking the wakers and a subscriber reads the generation after
registering, so no notice is lost to a concurrent poll. Runtime-free: no channel crate.

P3. **P10, the `.lag` guidance, is in the documentation** of `HealthModule::lag` and the crate
root: zero beside a `preStop` sleep, a few seconds without one, zero by default, and a lag past
`shutdown_timeout` cut by the cap and recorded `TimedOut { limit: ShutdownCap }`.

## Divergences

V1. **The before-shutdown hook is a closure hook on the `Readiness` binding, not an
`impl BeforeApplicationShutdown`.** DESIGN §3.2 names the trait and the bound `After(d + 1 s)`. The
trait's `TIMEOUT` is an associated const and cannot carry a lag read from the module's value, so the
hook is `.before_shutdown(..)` on the binding handle followed by `.timeout(lag + 1 s)`. It runs
under `HookKind::BeforeApplicationShutdown` at the binding's place in reverse connect order, which is
the place the trait hook would take. `lag + 1 s` saturates rather than overflowing.

V2. **`HealthModule` is global (`m.global()`).** The design is silent. `HealthHttp`'s routes, the
gRPC bridge and the database suite all read `Dep<Readiness>`, and `HealthHttp` cannot import a
`HealthModule` of its own without knowing the application's `.lag`; a second import with another lag
is another module. Global export makes one `HealthModule`, imported anywhere, visible everywhere. Two
`HealthModule`s with different lags in one app then export one key globally twice, which wiring is
expected to refuse (not verified).

V3. **A zero bound is refused as the `Readiness` binding's construction error.** DESIGN §10 says "a
zero `Check` bound, `connect`, the health module reading `Many<dyn Check>`". The factory returns
`Err(ZeroBound)`, which `connect` reports as `ConnectError::Construct { reason: Errored(..) }`,
redacted by the core. Only the first check declaring a zero bound is named; `ZeroBound` carries one
name, as the skeleton gives it.

V4. **After `Stopping`, `readiness()` answers a report holding one down result named `lifecycle`
with the reason "the app is stopping".** The skeleton's doc asks for "a report that is down without
running them"; `Report::is_up` is true for an empty report, so a down report needs a down entry.
Under `.detail(true)` it appears in `checks` as `"lifecycle": [{"status": "fail", "output": "the app
is stopping"}]`. A run that began before the flip and ends after it answers the same report, so no
probe reads ready once the hook has run.

V5. **A panic in a check, or in the call that builds its future, is down for that run.** The design
is silent. The reason is "the check panicked"; the payload is left out, being the check's own text,
and a `warn` names the check. Without this one panicking check would fail every probe reading the
shared run. A check past its bound is down with "the check did not answer within {bound:?}".

V6. **The shared run is per probe, and a run every caller abandoned is dropped, not resumed.**
DESIGN §3.1 says "a probe arriving while a run is in flight shares that run". Readiness and liveness
run different checks, so each has its own slot; the skeleton's one `in_flight` slot is replaced
(the `Runner` is `pub(crate)`, its fields read by no other area). Each slot counts the probes
awaiting its run, and the last to leave, finished or dropped, clears it. A `Shared` future left in
the slot after its callers dropped would otherwise be resumed by the next probe, whose checks'
bounds, measured from the abandoned start, would already have passed and read down. A finished
run's report is never handed to a probe that arrived after it finished.

V7. **`startup()` is always up and `liveness()` ignores `Stopping`.** DESIGN §3.2: startup "true
once `listen` has finished, which holds whenever a probe is answered", liveness "true whenever the
process answers, plus any check declared `Probe::Liveness`". Neither is turned false at `Stopping`;
kubelet stops both probes at termination (PROBES_obs.md §6). `startup()` runs no check.

V8. **A down reason is redacted where it is shown, in `ulo-health-http`, and stored as the check
returned it.** S-skel's R2 asked how `ulo-health` reaches the graph's redaction; S-core answered
that `AppHandle::redact` is public and left the route to H1. `HttpCx::app()` hands the route an
`AppHandle`, so `body::render` takes it and redacts each reason. The report is shared by every probe
of its run, so the reason cannot be moved into `redact`: its `source()` chain is flattened in
`body.rs` by the core's rule (joined by a colon, a repeat skipped, at most 32 links) and the text
redacted. `CheckResult::health()` read anywhere else holds the unredacted reason; the gRPC bridge
writes statuses alone, and a reader that prints a reason has to redact it the same way. No core
change is requested. `body::render` gained the `AppHandle` parameter (`pub(crate)`).

V9. **`HealthHttp` mounts each probe as a controller of its own.** S-skel's H2 chose a module with
configurable paths. `Controller::mount` is a function with no access to the module's value, so the
three routes are `ProbeRoute<Live>`, `ProbeRoute<Ready>` and `ProbeRoute<Started>`, each answering
`GET /` and placed at its path by `ControllerHandle::at`, the runtime prefix the GraphQL endpoint
uses. `HEAD` is answered from `GET` by the router. A path is normalized to one leading `/` and no
trailing one. The detail flag is bound in the module as a private value its routes read.

V10. **The body's `checks` object maps a check's name to an array of `{status, output}`,** `output`
present for a down check alone, which is the draft's shape for `checks`. Two checks of one name are
both listed in that name's array; naming two keyed stores apart is D1's. The response carries
`Content-Type: application/health+json` and no other header.

V11. **`ulo-health-http` depends on `http`**, the workspace entry, for `CONTENT_TYPE` and
`HeaderValue::from_static`; `ulo-http` re-exports `HeaderValue` but not the header names. No new
workspace dependency.

V12. **The health module requires a `Timer`.** It reads `Dep<dyn Timer>` for every check's bound and
the lag, and its hook's `After(..)` bound needs one; an app with neither a `Timer` nor a `Runtime`
fails `wire()` on the missing `dyn Timer`. The design runs health's bounds on the core's `Timer`
(§1) and does not say what happens without one; the documentation states the requirement.

## What was replaced on `master`, and not carried over

`TerminusModule`, `HealthCheckService`, `HealthIndicator`, `HealthEntry`, `HealthCheckResult`, and
the `disk`, `memory` and `http` indicators behind features. S-skel removed their files; H1 wrote
none of them again. The design has no indicators (BUILD_PLAN_3 "Not in race 3"); a check is what an
application or a store contributes.

## Requests to other owners

None.

## Not verified

- That `join_all` over futures borrowing the boxed run's own `Arc`s, and the `catch_unwind` around
  each check, satisfy the `Send` bound of `BoxFuture<'static, Arc<Report>>` without a
  higher-ranked-lifetime error.
- That `async fn construct` on `ProbeRoute<P>` and the boxed `answer::<P>` future are `Send`, read
  from the same pattern in `ulo-graphql-http`'s controller.
- That two global exports of `Readiness` are refused at `wire()` (V2).
- Any runtime behaviour: the §3.5 scenarios 1 to 4, the shared run, the lag under the cap.
