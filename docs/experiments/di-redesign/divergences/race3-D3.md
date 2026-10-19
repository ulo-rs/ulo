# Divergences: race 3, D3 (`ulo-db-conformance`)

Every place D3 departs from `integrations/DESIGN.md` §4.9, `BUILD_PLAN_3.md` or the fourth and fifth
responses, fills a shape they leave open, or needs something of another area. All await the user's
sign-off.

No cargo was run. Every API named was read from the race-2b tree at `5dd18436` and from the
skeletons of `ulo-db`, `ulo-health` and `ulo-conformance-support`; what only a compile or a run can
show is under "Not verified".

## Files written

| File | Lines | Holds |
| --- | --- | --- |
| `crates/ulo-db-conformance/Cargo.toml` | 29 | the skeleton's manifest plus `futures-util` (entry V1) |
| `crates/ulo-db-conformance/src/lib.rs` | 241 | the skeleton's items unchanged in signature; documentation only (entries V2-V5) |
| `src/cases/mod.rs` | 306 | the shared pieces: the root module, the app builders, the readiness poll, the error walk, the runtime waits |
| `src/cases/wire.rs` | 49 | the malformed-URL scenario |
| `src/cases/connect.rs` | 165 | the unreachable, bad-credentials and create-loop scenarios |
| `src/cases/lazy.rs` | 60 | the lazy scenario |
| `src/cases/health.rs` | 38 | the outage scenario |
| `src/cases/close.rs` | 109 | the drain scenario |
| `src/cases/stall.rs` | 88 | the one-thread scenario |
| `src/cases/runtime.rs` | 101 | the runtime-name scenario |
| `src/cases/keyed.rs` | 71 | the keyed scenario |
| `src/cases/migrate.rs` | 103 | the two migration scenarios |

No `todo!()` remains. No other file was edited.

## The scenarios and their planted breaks

| Stamped test | Case | Asserts | Planted break that fails it |
| --- | --- | --- | --- |
| `malformed_url_is_refused` | `wire::malformed_url_is_refused` | reachable options wire; malformed ones fail `wire` as `StartupError::Wiring` holding a `WiringError::ValueFailed`; `is_permanent`; no rendering (`Display`, `Debug`, `report`, each recorded failure) prints `PASSWORD` | a password echoed in the refusal |
| `unreachable_database_fails_at_the_bound` | `connect::unreachable_fails_at_the_bound` | under `connect_within(3 s)`, 1 000 retries 100 ms apart, `construct_timeout(3 s)`: a connect failure, `TimedOut`, not permanent, no earlier than 3 s and no later than 8 s; `Readiness` for a store whose `create` connects nothing, `Construct` or `Readiness` for one whose `create` connects | `Unreachable` treated as permanent |
| `bad_credentials_fail_at_once` | `connect::bad_credentials_are_permanent` | under a 20 s bound with 1 000 retries: permanent, `Errored`, under 10 s, one attempt where `Readiness` failed, `Construct` only for a store whose `create` connects, no `PASSWORD` in the report | a permanent error retried to the bound |
| `a_connecting_create_is_retried_and_stops_on_permanent` | `connect::create_loop` | fails at once on a store whose `CREATE_CONNECTS` is false; with the options cut and restored 1.5 s in, the connect succeeds no earlier than the restore; then bad credentials end the loop as above | a `create` run once outside the loop |
| `lazy_readiness_waits_for_the_first_ping` | `lazy::waits_for_the_first_ping` | fails at once unless `capabilities().lazy` and `!CREATE_CONNECTS`; with the options cut, a lazy app connects in under 5 s; readiness reads false and the store's check down, on the first reading and through the settle budget; restored, both turn up within the recovery budget | readiness set true before the first ping |
| `health_goes_down_in_an_outage_and_recovers` | `health::outage_and_recovery` | readiness up; cut, down with the store's check down within settle plus two 2 s check bounds; restored, up within recovery | a check that reports a cached `Up` |
| `pool_closes_after_the_drain` | `close::after_the_drain` | a standalone execution pings the module's client from before the shutdown until `settle` after the drain began; every ping succeeds, at least one answered during the drain; the shutdown reports no failure and abandons nothing | a pool closed before the drain |
| `connecting_does_not_stall_the_thread` | `stall::connecting_does_not_stall_the_thread` (`block_on_one_thread`) | three apps connect and close in turn while a ticker on the same thread records its longest gap; every tick on the scenario's thread; the gap under `Budget::stall` | a client created on the worker outside `spawn_with` |
| `unsupported_runtime_is_refused` | `runtime::unsupported_runtime_is_refused` | on the harness's tokio runtime renamed `"unknown"` (the default) and then `"smol"`, with reachable options and 1 000 retries: `ConnectError::Construct`, permanent, an `UnsupportedRuntime` with `required == "tokio"`, `given` the name, a non-empty `module`, under 10 s | the name check removed |
| `two_keyed_instances_stay_separate` | `keyed::two_instances` | `Dep<Client, First>` and `Dep<Client, Second>` resolve to two `Arc`s that are not `ptr_eq`, each pings; the unqualified client is `LookupError::NotFound`; readiness up over exactly two checks named from `Store::NAME` | two qualifiers resolving to one instance |
| `migrations_run_only_on_request` | `migrate::only_on_request` | fails at once unless `capabilities().migrations`; an app without `.migrate(..)` connects first; a migrating app has run nothing after `wire` and exactly one application after `connect`; a second migrating app leaves the count at one | migrations run without `.migrate(..)` |
| `migrations_run_once_across_replicas` | `migrate::once_across_replicas` | as above for the capability; two migrating apps wired, then connected together; both connect, and the migrations were applied once; a failed replica names any `MigrationFailed` it carries | the migration lock removed |

## Points D3 resolves

P1. **The drain scenario holds its query in a standalone execution, with no transport.** The app
stops at `Connected` and binds nothing. `Execution::open` on the root module opens the execution;
a task owns it and the module's client and calls `Store::ping` on the client every 20 ms, from before
the shutdown until the settle budget has passed since `Execution::is_draining` first read true,
then drops the execution. `App::close` runs the sequence of §9.5 with no transports: the drain waits
on `AppShared::live`, which a standalone execution enters as a transport's does, and the store's
`on_destroy` close runs after it. `Store::ping` is the only query a generic scenario can issue; a
run of pings across the drain is the generic form of "a query in flight at the drain".

P2. **The replica scenario runs two `App` values in one process.** Each has its own root, graph and
client, built from the host's one `options()`, with `Host::migrating` called once per app over one
`MigrationProbe`. Both are wired before either connects, and the two `connect` futures run under
`futures_util::future::join`, so their creations, readiness pings and init-hook migrations overlap
from the first poll. The server sees two clients, as it would see two processes. `join` polls both
on one task, which suffices for I/O-bound connects and needs no `Send` bound on `App::connect`'s
future.

P3. **No app binds a transport.** An app at `Connected` reads `Phase::Running` (`AppHandle::phase`),
which is what readiness needs, and `App<Connected>::close` runs the full shutdown. The health
scenarios read `Readiness` through `App::get` rather than over `ulo-health-http`, which keeps the
suite's dependencies to `ulo-health`.

P4. **A store's checks are those whose name contains `Store::NAME`.** The keyed scenario counts them
(exactly two) and the readiness poll requires every one to agree. D1 decides how two keyed checks are
named apart; containing `S::NAME` is all the suite reads (request R1).

P5. **The unknown runtime is the harness's own, renamed.** Two wrappers in `runtime.rs` delegate
`Timer` and `Spawn::spawn` to the harness's runtime; `Unnamed` keeps `Spawn::name`'s default
`"unknown"`, and `Smol` answers `"smol"`. The runtime underneath is tokio, so without the name check
the store connects and the scenario fails.

P6. **The bounds.** `connect_within(3 s)` for the unreachable scenario, with `construct_timeout(3 s)` on
the app so the one confirming ping of a store whose `create` connects (MongoDB on a plain URL)
times out at the same bound; a 20 s bound for the scenarios whose failure has to arrive "at once",
which they must do in under 10 s; 1 000 retries 100 ms apart wherever the bound, not the count, has
to end a loop; a 1.5 s outage in the create-loop scenario under a bound of 1.5 s plus the recovery
budget.

## Divergences

V1. **`futures-util` added to the manifest.** The skeleton's manifest has no future combinators;
`select` (the timeout) and `join` (the replicas) come from the workspace's existing `futures-util`
entry, as `ulo-rpc-conformance` takes them. Runtime-free; no new workspace dependency.

V2. **`Host::bad_credentials` now requires `PASSWORD` as the refused password.** The skeleton's
doc said only "credentials it refuses". The bad-credentials and create-loop scenarios assert the
refusal does not print `PASSWORD`, which DESIGN §4.6 states of every probed error ("no text
contained the password") and which §4.9's table does not list. A host whose server's real password
is `hunter2` has to choose another. No signature changes.

V3. **`Host::cut` and `Host::restore` are stated more precisely.** `cut` closes the connections open
through `options()` and fails every new one until `restore`, which `Relay::cut_for` with a long outage
does; `restore` lets connections through from when it returns. The health scenario relies on open
connections closing; the create-loop scenario on the restore moment. No signature changes.

V4. **`MigrationProbe::record` and `Host::migrating` are stated as applications, not runs.** The
skeleton said the body records "each time it runs". With the lock, both replicas call
`Migrations::run`, and the second finds nothing pending: the count the replica scenario asserts is
one per application of the pending migrations, so a run that finds nothing pending records nothing.
For SQLx's `Migrator` that means a host counts what was applied (the `_sqlx_migrations` rows before
and after, say), not calls of `run`. No signature changes.

V5. **Twelve stamped tests.** The plan says "the ten scenarios of §4.9"; §4.9's table has eleven rows,
and the skeleton's macro stamps twelve, splitting the migrations row in two. The skeleton's twelve
are kept. `lib.rs` documents which scenarios each kind of store declares not applicable:
`a_connecting_create_is_retried_and_stops_on_permanent` on a store whose `CREATE_CONNECTS` is false
(every SQL store), `lazy_readiness_waits_for_the_first_ping` on one whose `CREATE_CONNECTS` is true
(MongoDB, Redis), and the two migration scenarios on one with no migrator (MongoDB, Redis). The
create-loop case reads `CREATE_CONNECTS` rather than a capability, since the constant is the store's
own statement of it.

V6. **Planted breaks that some stores cannot show.** Each scenario fails against its break on the
stores where the break changes behaviour; on these it cannot:

- `pool_closes_after_the_drain` on a store whose `close` does nothing a later query sees (Redis's
  `ConnectionManager` has no close; S2d's `close` drops it): a close before the drain leaves the
  pings succeeding.
- `migrations_run_once_across_replicas` on `ulo-db-sqlx`: SQLx's `Migrator::run` takes its own
  `pg_advisory_lock` and `GET_LOCK` (DESIGN §4.4), so with `ulo-db`'s lock removed the second
  replica still waits. SQLite's `lock()` is a no-op, so the SQLite host shows the break. SeaORM and
  Diesel take none and show it.
- `a_connecting_create_is_retried_and_stops_on_permanent` on MongoDB with a plain `mongodb://` URL:
  `create` connects nothing there, and the confirming ping waits out the outage under server
  selection. A host serving `mongodb+srv://` would show it; a container cannot serve SRV records
  without a DNS server of its own.
- `migrations_run_only_on_request` detects a run the store makes without being asked only for
  migrations the store can reach on its own, through the next app's count: had the first app
  applied them, the requested app would find nothing pending and the count would stay at zero. A
  store cannot run the host's migrations without being handed them.

The coordinator's "shown failing against its planted break" step reads this list for which store
to show each break on.

## Requests

R1. **D1, `crates/ulo-db/src/health.rs`: every check `StoreCheck` names contains `S::NAME`.** The
suite reads a store's checks by that substring; a name that drops it fails the keyed, lazy and
health scenarios with "no checks" for the store. D1's body names a check `S::NAME`, or
`S::NAME@<section path>` (`check_name`), which holds. An observation for D1, not a request: two keyed
instances built with `StoreModule::from_options`, whose section path is empty, are two checks with one
name, `S::NAME`, since the name reads the path and not the qualifier. The keyed scenario counts
them and passes; a probe body listing checks by name (`ulo-health-http`'s `.detail(true)`) shows
the name twice.

Read and holding, no request: H1's `Readiness` answers from its checks from the moment its binding
is built until the before-shutdown hook records `Stopping` (`crates/ulo-health/src/readiness.rs`),
so an app at `Connected` with no transport reads readiness from its checks; and `HealthModule` is
global and exports `Readiness` (`crates/ulo-health/src/module.rs`), which the suite reads with
`App::get::<Readiness>()` from the root.

## Not verified

- That the suite compiles: no cargo was run. Read for: `#[non_exhaustive]` enums matched with a
  wildcard (`ConnectError`, `ConstructError`, `LookupError`, `WiringError`, `StartupError` through
  `let else`); explicit generic arguments beside `impl Trait` arguments (`timeout::<H, _>`, stable
  since 1.63, as the RPC suite writes them); the `Send + 'static` futures handed to `spawn_with` in
  `close.rs` and `stall.rs`, whose captures are an `Execution` (`Send + Sync` by its docs), the
  store's `Client` (`Send + Sync`), `Arc`s and the runtime's `BoxFuture` sleeps; the outlives bound
  `<H::Store as Store>::Options: 'static` that `StoreModule<S>: Module` needs, which holds because
  every component of the projection is `'static` (`Host: 'static`, `Store: 'static`).
- That the timing bounds hold on CI's containers: the 3 s connect bound against an unreachable host
  whose connect is refused at once or black-holed; the 2 s check bound against an SQLx pool's
  `acquire_timeout` during the outage; the 20 ms ping gap in the drain scenario.
- That a `Relay` cut makes an SQLx or MongoDB ping fail rather than hang past the check's bound
  (either reads down); and that redis-rs's `ConnectionManager`, created with
  `set_number_of_retries(0)`, reports a relay's closed connection as a refusal `create` retries.

- Checked against a run (2026-10-19), V6's SQLx entry does not hold: with the lock removed on SQLx Postgres the scenario fails at `applied the migrations 2 times` with both replicas connected and one row in `_sqlx_migrations`, because `Counted` reads `before` outside any lock and each replica counts the one application; the red comes from the probe, which cannot tell a migration applied twice from one counted twice. SeaORM and Diesel on Postgres fail it on a real double application (a duplicate `pg_type_typname_nsp_index` key).
- Checked against a run (2026-10-19), `unreachable_database_fails_at_the_bound` cannot fail against `Unreachable` treated as permanent on SQLx, whose 5 s acquire timeout outlasts the 3 s bound so no attempt returns (an `acquire_timeout_secs: 1` host turns it red), nor on MongoDB, whose 30 s server selection outlasts it; with `serverSelectionTimeoutMS=1000` the unbroken MongoDB scenario fails at 1 s after one attempt, since `create` connects nothing on a plain URL and the confirming ping runs `.retries(0)`.
