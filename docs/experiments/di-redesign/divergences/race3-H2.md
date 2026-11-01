# Divergences: race 3, H2 (`ulo-health-grpc`)

Files written: `crates/ulo-health-grpc/src/{lib,bridge,interval}.rs`. `Cargo.toml` is unchanged: it
already names `ulo`, `ulo-grpc`, `ulo-health`, `tonic-health` and `tracing`, and nothing more is
used; `ulo-grpc` gains no dependency on `ulo-health`. No cargo was run. Built against the finished
`crates/ulo-grpc/src/health.rs` and `server.rs` (S-core) and the skeleton signatures of
`crates/ulo-health/src` (H1's bodies were all `todo!()` when read). Replaces nothing on `master`.

## Points the area resolves

1. **The bridge imports `GrpcHealth::module()` itself.** `register` calls
   `m.import(GrpcHealth::module())`. `DynamicModule::new::<GrpcHealth, ()>((), ..)` has one identity
   wherever it is built, and equal identities are one module, so an application that imports it as
   well gets the same module and the same reporter. Without the import a missing `GrpcHealth` would
   still fail `wire()` (the bridge reads `Dep<GrpcHealth>`), so the import saves a step rather than
   closing a silent gap. The skeleton's `lib.rs` example and `HealthGrpc`'s doc showed the
   application importing it; both now say the bridge does (S-skel's H3 left the point to H2).
2. **A zero interval is refused at `wire()`, not `connect`.** S-skel's H3 and the skeleton doc on
   `HealthGrpc::interval` said `connect`. The setting is a field of the module, read in `register`,
   so registration is the first phase that sees it (integrations DESIGN §0 principle 2), and §10's
   row places the refusal "where the bridge's setting is read". `register` records
   `try_value::<Bridge, _>(Err(ZeroInterval))`, the shape `RpcClientModule` uses for its zero
   timeouts, and registers nothing else of the bridge. The error type stays `ZeroInterval`; its
   `Display` now follows `ulo_transport::prepare::zero_bound`'s hint wording (the setter, what zero
   would do, `Bound::Unbounded` as the way to turn it off). The interval's doc says `wire()`.

## Decisions within the design

3. **`""` turns `NOT_SERVING` in a before-shutdown hook of the bridge's own**, which aborts the
   re-evaluation task, awaits its end, then writes `NOT_SERVING` through `GrpcHealth`. The design
   says `""` is "pushed at lifecycle transitions" and turns `NOT_SERVING` at `Stopping` (§3.4,
   decision 11). A write made by the task on `Readiness`'s change notice runs whenever the task is
   scheduled, with no order against the drain's GOAWAY; the hook runs in shutdown step 1, which
   ends before step 2 starts the drain. Awaiting the aborted task first keeps an evaluation begun
   before `Stopping` from writing `SERVING` after the hook's write. `GrpcHealth::write` is safe to
   abort: tonic-health 0.14.6's `set_service_status` suspends only acquiring its lock, before the
   send, and `write` inserts into the copy with no await after the send, so an abort leaves the
   copy and the reporter agreeing (read, `tonic-health-0.14.6/src/server.rs:74-92`).
   Consequence: `Readiness::subscribe` and `Changes` are not used by the bridge. The hook does not
   depend on running before or after the health module's own before-shutdown hook.
4. **The first evaluation runs on the task, not inside the init hook.** The init hook calls
   `delegate_overall()` and spawns the task through `Dep<dyn Runtime>`; the task's first pass
   evaluates readiness and writes `""` at once. Awaiting the evaluation in the hook would fail
   `connect` on the hook's bound (`hook_timeout`, 10 s) for a check declared `Bound::Unbounded`
   that never answers, a refusal §10 does not list. Until the first write `""` holds `SERVING`, the
   reporter's start and what the server reports without the bridge; nothing serves before
   `listen()`. `delegate_overall()` still runs inside the init hook, during `connect`, before
   `listen()` and every server write.
5. **`Bound::Unbounded` evaluates once at start and then only at `Stopping`.** "Statuses change at
   lifecycle transitions alone": the start and `Stopping` are the two the bridge sees. A check that
   goes down later is not reflected under `Unbounded`, as the design states.
6. **The task ends at `Stopping` on its own as well**: a pass that read `is_stopping()` true before
   evaluating writes the down status and returns, and under `Unbounded` a `Stopping` that arrives
   mid-pass triggers one more pass. That covers a before-shutdown hook the shutdown cap skipped.
7. **The task is aborted when the bridge's state drops.** An app whose `connect` fails after the
   init hook, or that is dropped without a shutdown, runs no before-shutdown hook; `Bridge`'s
   `Drop` aborts the task. The task holds `Readiness`, `GrpcHealth` and the runtime, never the
   `Bridge`, so the drop is reachable.
8. **The state is a private singleton, `Bridge`, and the hooks are module hooks.** The singleton's
   factory reads `Dep<Readiness>`, `Dep<GrpcHealth>` and `Dep<dyn Runtime>`; the init and
   before-shutdown hooks are `m.on_init` / `m.before_shutdown` closures reading `Dep<Bridge>`, which
   avoids a binding hook reading its own binding. Nothing is exported.
9. **A panicked task is logged at `warn` when the before-shutdown hook awaits it**, with the
   redacted message `TaskEnd::Panicked` carries. A panic ends the re-evaluation and nothing restarts
   it.

## Requests

- **H1, `crates/ulo-health/src/module.rs`: `HealthModule` makes `Readiness` visible to a sibling
  module, through `m.global()`.** The application imports `HealthModule` and `HealthGrpc` side by
  side, and a module sees only its imports' exports and globals. The bridge cannot import
  `HealthModule` itself: its identity is `of_value(self)`, `lag` included, so the bridge's
  `HealthModule::new()` beside the application's `HealthModule::new().lag(d)` would be a second
  module with a second `Readiness`. Without `global()` the bridge's `Dep<Readiness>` fails
  `wire()` as missing. `ulo-health-http`'s routes need the same.

## Assumptions about H1's bodies (signatures as the skeleton freezes them)

- `Readiness::readiness()` answers the readiness checks' result from `connect` on, and down
  without running them from `Stopping`, as its doc says. The task's first pass runs during
  `connect`; were `readiness()` down until `listen()` finished, `""` would read `NOT_SERVING` until
  the next tick, and under `Unbounded` until shutdown.
- The futures of `Readiness::readiness()` hold nothing `!Send` across an await. The task is spawned
  as a `BoxFuture<'static, ()>`, which requires `Send`; a `std::sync::MutexGuard` held across an
  await inside `readiness()` fails this crate's build.
- `Readiness` is `Send + Sync + 'static` (the skeleton's fields are), which `Bridge` needs as a
  singleton.

## Not verified

- That a trait object's supertrait methods resolve without the supertrait imported. Avoided:
  `ulo::Spawn::spawn(&*runtime, ..)` and `ulo::Timer::sleep(&*runtime, ..)` are called by path.
- Every runtime behaviour, scenario 5 of §3.5 included (`Watch` receives `NOT_SERVING` on `""` at
  `Stopping` and nothing while the value is unchanged); the coordinator's tests after the compile.
