# Divergences: race 3, D1 (`ulo-db`)

Every place D1 departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and fifth
responses, fills a shape they leave open, or decides a point the plan leaves to the area. Each entry
gives what the design says, what was written, and why. All await the user's sign-off.

No cargo was run. Every claim about what compiles is read from the sources; the items a compile has
to show are listed under "Not verified".

Files written: `crates/ulo-db/src/{lib,store,module,connect,migrate,health,error}.rs`. The manifest
`crates/ulo-db/Cargo.toml` is unchanged from the skeleton: `ulo`, `ulo-config`, `ulo-health`,
`serde`, `tracing`, no driver. No other file was edited.

Replaced on `master`: the old core's `CheckedModule` and `StartupCheck`, on which each `master` store
built its connect, retry and health policy. Nothing of them is carried over; the branch's core no
longer has them, and the policy is written fresh over `.ready`, `spawn_with` and `Permanent`.

## Points the plan leaves to D1

PD1. **The default `.retries(..)` is `u32::MAX`, and the default `.backoff(..)` is 1 s.** The design
gives one total bound, `connect_within`, and the suite's scenario "an unreachable database fails
`connect` at its bound". A finite count would end the loop early against a database that refuses at
once (`Io(ConnectionRefused)` in 0.3 ms, PROBES_db.md §8.1), with `Errored` rather than at the bound.
With no count, the core's readiness loop (`made > ready.retries` never holds) and `ulo-db`'s loop
around `create` both run until `connect_within`. 1 s gives 30 attempts in the default bound. A
written count still ends the loop earlier, with the last attempt's error, and the docs on
`StoreModule::retries` say so.

PD2. **`K` is `3_490_428_366_386_280_370`, written as `MIGRATION_LOCK_KEY`.** S-skel wrote the
literal; D1 checked it: `shasum -a 256` of `ulo-db/migrations` begins `30707e1b2c66b7b2`, which read
as a big-endian signed `i64` is 3490428366386280370, and its remainder by `0x3d32ad9e` is 21608874,
as DESIGN §13.2 states.

PD3. **Two instances' checks are named apart by their section path: `S::NAME` alone for an empty path
(`from_options`, or the whole configuration), `S::NAME@path` otherwise (`postgres@database`).**
`ModuleDef` gives `register` no read of the `Keyed` qualifier (`keyed_by` and `ModuleNode::keyed`
are `pub(crate)` in the core), so the qualifier cannot name the check. Two keyed instances read from
two sections print apart; two built with `from_options` print alike, and a `checks` object keyed by
name would show one of them. The crate docs state it. A core read of the qualifier's name would close
the gap (request R1, optional).

## Divergences and choices

DV1. **`StoreModule::migrations` holds a `Migrate<S>`, not an `Arc<dyn Migrations<S>>`.**
`impl<S: Store> Module for StoreModule<S>` cannot call `MigrationLock::lock`, which needs
`S: MigrationLock`. `.migrate(..)`, where that bound holds, stores the migrations beside a function
pointer to `run_locked::<S>`, which `register` calls. The field is `pub(crate)`; no public signature
changes.

DV2. **`pub(crate)` signatures changed:** `StoreCheck::new(name, runtime, client)` (the skeleton took
the client alone), and `run_locked(runtime, client, migrations, bound)` (the skeleton had no
runtime). The check pings through `spawn_with` and the migrations run as a task, so both need the
app's runtime. `connect::{Attached, within}` and `health::check_name` are new private helpers.

DV3. **The health check's ping runs through `spawn_with`, as the readiness ping does.** DESIGN §4.2
says the check "runs `ping`" and names `spawn_with` for the readiness ping, the create, the
migrations and the close. One path for every ping keeps whatever a library spawns on the app's
runtime, and lets a check past its bound stop its ping (DV4).

DV4. **Every task `ulo-db` spawns is aborted when the future awaiting it is dropped.** The design is
silent. `spawn_with`'s handle detaches on drop, so a construction cut at `connect_within`, a readiness
attempt or check cut at its bound, or a hook cut at its bound would leave the attempt running unseen,
and a `create` that finished after the cut would build a client nobody closes. `connect::Attached`
aborts on drop. The close is aborted at the destroy hook's bound too; PROBES_db.md §6 shows an SQLx
pool stays closed when its `close` is cancelled, and the late holder closes its own connection.

DV5. **The migration bound is `connect_within`, over the lock's wait, the migrations and the release
together; the module's `on_init` hook is `.unbounded()`.** The design says "under the module's
bound" and the module has one, `connect_within`. The hook is unbounded so the failure arrives as
`MigrationFailed` from `ulo-db`'s own bound rather than as a hook `TimedOut` at `hook_timeout`'s
10 s. On expiry the task is aborted and the guard dropped, which releases each primitive (PROBES_db.md
§7.2). The `.migrate` docs say a long migration needs a longer `connect_within`.

Design question Q1: a replica whose wait for another replica's lock reaches the bound fails with
`MigrationFailed` inside `Permanent`, as the skeleton's docs on `run_locked` and `MigrationFailed`
state, yet a restart after the other replica finishes would succeed. Whether the lock's wait timing
out should be retryable (no `Permanent`) is the designer's.

DV6. **A zero `connect_within` is refused through `m.try_value::<ZeroConnectBound, _>(Err(..))`.**
DESIGN §10 says "the module's registration". `try_value` is the registration-time refusal the core
offers; the error type is its own key, nothing depends on it, and `wire()` lists it beside the
options' problems.

DV7. **`validate`'s problems are rewritten under the section's path and recorded as a `Problems`
error, not a `ConfigError`.** `Problems::at` is the only public constructor, so each path is joined
as `section.path` and the origin is not carried; `Store::validate` builds its problems with `at`,
which sets no origin, so nothing is lost today. `ConfigError` has no public constructor, so a
`validate` refusal is reached through `Redacted::downcast_ref::<Problems>()`, and a failed section
read through `downcast_ref::<ConfigError>()`. A section read's failure skips `validate`.

The tree at the time of writing holds a `Problems::prefixed(self, prefix)` in C's `problems.rs`,
absent from the frozen skeleton, which keeps each problem's origin and joins an index path (`[0]`)
without a dot. D1 does not call an item the spine did not freeze; its private `under` joins paths by
the same rule, and switching to `prefixed` once C's entry is signed is a one-line change in
`module.rs`.

DV8. **A `create` whose task panics or is dropped by the runtime ends the loop at once**, as a
panicking readiness attempt ends the core's check, with a `TaskFailed` error and no `Permanent`. The
design is silent.

DV9. **`from_options` registers only what `Store::secrets` answers.** A section read registers every
`Secret` leaf; options built in code pass through no deserializer, and `StoreModule` names no field.
The `from_options` docs tell the importing module to register the URL with `m.secret(..)`, which
redacts it across the graph. `Store::secrets`' contract is unchanged ("beyond the `Secret` leaves
the section read already registered"), since D2 builds against it in this stage.

DV10. **An app with no runtime at all fails `wire()` on a missing `Dep<dyn Runtime>`, not `connect`
with `UnsupportedRuntime`.** DESIGN §10 refuses "a runtime other than tokio, one declaring no name
included" at `connect`; that row holds for any runtime given. An app given none has nothing to
answer a name, and `.timeout(..)` on the binding needs a `Timer` anyway.

DV11. **`lazy()` carries `const { assert!(!S::CREATE_CONNECTS) }`**, which S-skel's D2 entry offered:
a store that implements `LazyStore` with `CREATE_CONNECTS` true fails to compile where `lazy` is
called, never at runtime.

DV12. **`UnsupportedRuntime::module` and `MigrationFailed::module` are `S::NAME`.** The fields are
`&'static str` and the type name of `StoreModule<S>` is not a constant string; the `ConnectError`
around each names the binding's key and module.

DV13. **The retry log line carries the store and the attempt, not the error.** A `tracing` line is not
redacted by the graph, and a library's error can print the URL; the error reaches the core, which
redacts it, when the loop ends.

DV14. **`.lazy()` with `.migrate(..)` still waits for the database at start**, since the migrations
connect in `on_init`. The design does not combine the two; nothing refuses it.

DV15. **The module's identity is the options alone, as the skeleton wrote it.** Two imports with equal
options and different builder settings are one module, registered with the settings of the first the
graph reaches; the `StoreModule` docs say so.

## Requests

R1. **S-core, `crates/ulo/src/module/def.rs` (optional):** a read of the `Keyed` qualifier's name at
registration, for instance `ModuleDef::qualifier(&self) -> Option<&'static str>`, so a keyed store's
check can be named from its qualifier where its section path does not tell it apart (PD3). Without
it, PD3 stands.

## Not verified

- `const { assert!(..) }` naming `S::CREATE_CONNECTS` inside `impl<S: LazyStore>`, evaluated after
  monomorphisation (inline `const` with generics is stable from 1.79).
- `Factory` inference for the fn items `ready_ping::<S>` and `close::<S>` and for the annotated
  closures, the pattern the core's own `|cfg: Dep<DbConfig>| async move { .. }` uses.
- The coercion closure `|check| check` producing `Arc<dyn Check>` from `Arc<StoreCheck<S>>`, as the
  core's `|a| a` examples do.
- That the futures of the generic `async fn`s `connect::create` and `migrate::run_locked` are `Send`
  under the trait's bounds, which `try_singleton` and `BoxFuture` require.
- `impl Future<..> + use<F>` on `connect::within` (precise capturing, stable from 1.82).
- That `&dyn Runtime` defaults to `dyn Runtime + 'static` through `Timer`'s and `Spawn`'s `'static`
  supertrait bounds, which `Spawn::name(runtime)` and `Timer::sleep(runtime, ..)` rely on; the
  skeleton's `require_tokio` already took `&dyn Runtime`.
- `type RunLocked<S>` naming `<S as Store>::Client` with no bound on `S`.
