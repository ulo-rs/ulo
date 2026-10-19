# Divergences: race 3, step 5, a `close` claimed at the call, and the HTTP embedding and the WebSocket hub released at their drop (Q14, Q15)

The designer's Q14 ruling is built in the core. `AppHandle::close` is a `fn` returning
`impl Future<Output = Result<Shutdown, ShutdownError>> + Send + use<>`: the claim on the servers is
taken at the call and the trigger at the first poll, so a `close` called before the `App` value's
drop and polled after it runs the whole shutdown, its hooks and every server's `close` included, and
one dropped unpolled releases the servers as its claim drops. A `close` whose claim finds the
servers released by a drop answers `Err` carrying the new `ShutdownFailure::Dropped` and runs no
step. The Q15 ruling is built in `ulo-http`, `ulo-ws` and `ulo-ws-redis`: the embedding's drop takes
an installed host future out of its slot, the hub's delivery task holds a `Weak` and the hub's drop
aborts it, and the Redis subscription ends when its stream drops. Making the hub drop at all needed
a second fix the ruling did not name: the WebSocket hand-off's table held the app's `AppHandle`
from inside the app's graph, and `UpgradeHandler` gains a defaulted `release` that the HTTP
servers' drops call (D4, F2).

Files changed: `crates/ulo/src/lifecycle/shutdown.rs`, `src/app/{mod,handle,shared}.rs`,
`src/error/mod.rs`; `crates/ulo-http/src/{embed,server,upgrade}.rs`; `crates/ulo-ws/src/{rooms,handoff}.rs`;
`crates/ulo-ws-redis/src/lib.rs`; `docs/experiments/di-redesign/DESIGN.md` §9.5 step 6 and the
`ShutdownFailure` listing. Tests: four added to `crates/ulo-runtime-tokio/tests/app.rs`, and a new
`tests/dropped.rs` in `ulo-http-{axum,poem,salvo,rocket,actix}`, `ulo-ws` and `ulo-ws-redis`.
`Cargo.lock` is unchanged. No commit was made. The workspace's `FRAMEWORK_GAPS.md` has F433 and F434
closed and F426's last limit closed, each in a note marked "closed by the step-5 batch"; F428 is
unchanged.

## The signatures

```rust
// crates/ulo/src/app/handle.rs, was `pub async fn close(&self, signal: Signal) -> Result<Shutdown, ShutdownError>`
pub fn close(&self, signal: Signal) -> impl Future<Output = Result<Shutdown, ShutdownError>> + Send + use<>;

// crates/ulo/src/error/mod.rs, `#[non_exhaustive]`, additive
pub enum ShutdownFailure { Hook { .. }, Close { .. }, Dropped }

// crates/ulo-http/src/upgrade.rs, additive, defaulted
pub trait UpgradeHandler { /* .. */ fn release(&self) {} }

// crate-private
impl<A: Embed> Drop for Embedded<A>;     // ulo-http: releases an installed host future, then each upgrade handler
impl<B: Backend> Drop for Server<B>;     // ulo-http: releases each upgrade handler
impl Drop for Hub;                       // ulo-ws: aborts the delivery task
struct Closing { shared: Arc<AppShared>, dropped: bool }   // ulo: was `Closing<'a>(&'a AppShared)`
```

`handle.close(sig).await` compiles as written everywhere in the workspace: `cargo check --workspace
--all-targets` passes with no change at any call site.

## Decisions

### D1. The claim at the call, the trigger at the first poll

`shutdown::close` (`crates/ulo/src/lifecycle/shutdown.rs:150`) takes `Closing::claim` before it
builds the `async move` block, which triggers and runs the sequence. `Closing` now owns an
`Arc<AppShared>`, so the future holds what the shutdown needs whatever happens to the handle or the
`App` value. `serve` and `App<Connected>::close` go through the same function; both hold the `App`
value's `Owner` while the claim is held, so nothing changes for them, and in `serve` the claim is
still declared after the moved `Owner` and drops first.

The claim records at the call whether the servers were already released by a drop: the `App` value
dropped, no other claim held, and the close step not ended. `Release` gains `servers_closed`, set
under the same lock by the close step before its release (`:251`). Whenever the `App` value has
dropped and the count of claims is zero, a release has happened, by the drop or by the last claim's
drop; `servers_closed` tells that release from the close step's.

**The ruling's soundness rule holds:** an unpolled `close` dropped after the app's drop decrements
the count in `Closing`'s drop and, the last, calls `release_servers` once, outside the lock, as
batch 45's drop does.

### D2. Case (a) runs its hooks; the fallback was not needed

The ruling asked whether the shutdown can run its hooks once the `Owner` has dropped. It can.
`Owner`'s drop only sets `app_dropped` and, with no claim held, releases the servers; it frees
nothing else. The singleton store, the graph and the phase live in `AppShared`, which the claim's
`Arc` keeps. With a claim held the drop releases nothing, so the sequence finds every server.
`a_close_called_before_the_app_s_drop_and_polled_after_it_closes_every_server` reads the
before-shutdown hook's log entry, the phase `Closed`, each server's `close` in reverse bind order,
then the servers' drops and the store's with the last handle. The close answers `Ok`, the outcome
the sequence produced. The fallback the ruling named (servers closed, `Err(Dropped)`) is not built.

### D3. `ShutdownFailure::Dropped`: when, and what it says

A `close` answers it when its claim finds the servers released by a drop (D1). That covers batch
42's case, a `close` called after an app dropped with no `close` under way, and an app whose `serve`
or a handle's `close` was abandoned before the close step and then dropped. The ruling's parenthesis
says batch 42's `close` "today answers Ok with no hook run"; batch 42's log (`p03-*-0.txt`) records
that `close` running the before-shutdown hook and answering `Ok`, closing no transport. It now
answers `Err(Dropped)` and runs no hook.

The answer runs no step: no trigger is recorded, the phase stays where it was, and the outcome is not
stored in the shutdown's cell, so each later `close` answers `Dropped` again. The report's signal is
the winning one when a shutdown had been triggered, the call's own otherwise, and it counts no
execution. `Display`: "the app was dropped before its shutdown closed its servers: they were
released without their `close`, and no shutdown hook runs after the drop". The ruling's wording,
"its hooks did not run", is false for an abandoned shutdown whose before-shutdown hooks ran before
the drop, so the text states what holds in every case (F3).

A `close` after the drop of an app whose close step ended answers that shutdown's outcome, as batch
42's `a_closed_app_s_drop_and_a_later_close_release_nothing_again` requires, unchanged.

### D4. F433: the embedding's drop

`Embedded<A>`'s `Drop` (`crates/ulo-http/src/embed.rs:730`) swaps the host slot to `Released` and
drops what it held after the slot's lock is released. Installed and never taken, that is the host's
future, which owns the host's listener. `serve`'s future borrows the embedding, so a `Taken` slot at
the drop means `serve` already ended or was dropped. A `Handle::host` after the drop is refused as
after `close`, which keeps a later installation from forming the same cycle. One fix covers the
five adapters: each installs through `Handle::host`. No weak reference is needed: the `Stopping` and
`Service` inside the host future are the other side of the cycle, and the owner's drop removes the
future that holds them.

Batch 46's checker read rocket and actix as installing "the same way". rocket binds its listener at
`launch`, so an ignited rocket installed and never served holds no socket. actix-server 2.9.8 binds
at `HttpServer::bind` and starts its accept thread in `ServerInner::run_sync`, inside the `Server`
future (`actix-server-2.9.8/src/server.rs:184-198`, read), so an unpolled one holds the sockets and
no thread; `run`, which installs before it returns its future, is the test's path on actix.

### D5. F434: the hub, and the hand-off cycle reading found

The hub's delivery task holds a `Weak<Hub>`, upgraded to subscribe and for each delivery and
dropped between them, and the hub's `Drop` aborts the task (`crates/ulo-ws/src/rooms.rs:110`,
`:160`). `Hub::stop` is unchanged and still what `close` calls. The subscribe call stays inside the
task, so where an adapter's `subscribe` runs is unchanged.

With that alone the test failed: after the app's and the last handle's drop the singleton, the
adapter and the subscription were all alive (`probes/p01-ws-flags-0.txt`). The app served its
gateway through the hand-off on `ulo-http-hyper`'s port. `WsModule` registers the hand-off in
`Upgrades` metadata, which the app's graph holds, and the table its `prepare` builds holds the app's
`AppHandle` (`crates/ulo-ws/src/table.rs`, `Inner::app`), a cycle no release of the servers
reaches. It holds for a closed app too: `close` leaves the table in place. The same app over a
standalone `ulo_ws_hyper::Server`, which prepares no hand-off, freed everything (the Redis test).

The fix follows the ruling's shape, the owner's drop releasing: `UpgradeHandler::release`
(`crates/ulo-http/src/upgrade.rs:65`), defaulted to nothing, which `Server<B>`'s new `Drop`
(`crates/ulo-http/src/server.rs:383`) and `Embedded<A>`'s call on every handler they prepared; the
hand-off's takes its table out and drops it (`crates/ulo-ws/src/handoff.rs:118`). This adds a
method to a public trait (F2).

`ulo-ws-redis`'s subscription task ran `while !sender.is_closed()` around a `listen` that waits on
the next Pub/Sub message, so a dropped stream ended it only when a message arrived. The loop now
runs inside `futures_util::future::select` against `sender.closed()`
(`crates/ulo-ws-redis/src/lib.rs:144`): the stream's drop ends the task wherever it waits, and the
Pub/Sub connection closes with it. redis-rs 1.7.1's `PubSubStream` aborts its driver task on drop
(`redis-1.7.1/src/aio/pubsub.rs:384-406`, read), which the test confirms at the server's side.

"Connections ended" is read as the adapter's connections: the Pub/Sub one and the publisher's. A
WebSocket client's upgraded connection is a detached task holding the hub through its `Accept`, and
outlives the server until the peer hangs up (F435, open). The tests open no client connection.

### D6. No shared owner type

The ruling asks that each crate not invent its own mechanism. Each fix is the owner's `Drop`
releasing what the other side would hold, the shape of the core's `Owner` and the TCP links'
`Owned`; no new type was added. The hub's `Drop` aborting a `TaskHandle` is the same three lines as
`ulo-rpc-tcp-core`'s `Owned` (F4).

## The tests

| Test | What it holds |
| --- | --- |
| `ulo-runtime-tokio` `app.rs` `a_close_called_before_the_app_s_drop_and_polled_after_it_closes_every_server` | (a): nothing touched at the drop, then the hook, `Phase::Closed`, `close b`, `close a`, both drops, the store last |
| `a_close_called_before_the_app_s_drop_and_dropped_unpolled_releases_the_servers` | (b): nothing touched at the drop; the unpolled future's drop drops both servers once, no `close` and no hook; the store with the last handle |
| `a_close_called_after_the_app_s_drop_answers_that_the_app_was_dropped` | (c): `Err` with failures `[Dropped]` and its text, no hook, no `close`; a second `close` the same |
| `a_close_spawned_before_the_app_s_drop_closes_every_server_in_every_race` | (d): 500 races on two workers, `tokio::spawn(handle.close(..))` then the drop with no spin; every server closed in each; requires at least one race whose phase still reads `Running` after the drop |
| `ulo-http-{axum,poem,salvo,rocket,actix}` `dropped.rs` | F433: the host's listener refusing after the app's drop (not rocket, D4), no task left, a later `host` refused, the singletons with the embedding's last handle |
| `ulo-ws` `dropped.rs` | F434 on the hand-off: the subscription's stream dropped, the adapter dropped, the singletons, no task left |
| `ulo-ws-redis` `dropped.rs` | F434 on Redis: a stand-in server on a local port answering `SUBSCRIBE`, `PSUBSCRIBE`, `PUBLISH` and `+OK` to the rest, counting open connections; the Pub/Sub and publisher connections both closed after the drops |

The race test's phase read undercounts: the task can poll between the drop and the read. Three runs
read `Running` after the drop in 500 of 500 races each.

### Breaks

Specs in the session scratchpad's `step5/specs/`, output in `step5/broken/` and `step5/probes/`,
each with `summary.txt` recording every restore's `cmp`. The script copies each file aside before
its edit, copies it back after the runs, `touch`es it and `cmp`s it: every restore reported
`identical`.

| Break | Result |
| --- | --- |
| b01. the claim taken at the first poll | (a) and (b) fail, `a server closed or dropped at the app's drop: ["drop a", "drop b"]`; (d) fails at race 0 answering `Dropped` |
| b02. the claim counted at the call, its guard built at the first poll (`ManuallyDrop`) | (b) fails, `` `drop a` was not logged once after the unpolled close's drop: [] `` |
| b03. `Ok` in place of `Dropped` | (c) fails, `a close after the app's drop answered `Ok`` |
| b04. the embedding's drop releases nothing | all five adapters' `dropped.rs` fail: the listener still accepting after 5 s on axum, poem, salvo and actix, the singletons outliving the last handle on rocket |
| b05. the delivery task holds an `Arc<Hub>` | both WS tests fail: the subscription not dropped; the Redis connections not closed |
| b06. the hub's drop does not abort the task | the Redis test fails; the `ulo-ws` test passes (F5) |
| b07. the hand-off's `release` keeps its table | the `ulo-ws` test fails, the subscription not dropped |
| b08. the subscription not racing its stream's drop | the Redis test fails, the connections not closed |
| p01. the `ulo-ws` test with D5's first fix alone, flags printed after the drops | `singleton false adapter false subscription false tasks 1 before 0` |

## Left for the DESIGN folds

- The transports DESIGN, X21 (§3.5 and the X-table row): `UpgradeHandler::{prepare, bound, drain,
  close}` gains `release`, called by `Server<B>`'s and `Embedded<A>`'s drops.
- The transports DESIGN, §3.8's embedding shutdown: an embedding dropped without `close` drops an
  installed host future; a `Handle::host` after that drop is refused.
- The transports DESIGN, §4.3: the hub's delivery task ends at the hub's drop, and the Redis
  subscription at its stream's drop.
- The core DESIGN §9.5 step 6 and the `ShutdownFailure` listing are updated in this step.

## Flags for the designer

### F1. `close`'s future borrows nothing (`use<>`)

The ruling's signature is `fn close(&self, ..) -> impl Future + Send`. In edition 2024 that captures
the borrow of `self`, and `tokio::spawn(handle.close(sig))` would not compile; a caller spawning a
`close` would write `let h = handle.clone(); spawn(async move { h.close(sig).await })`, which takes
the claim at the task's first poll and reopens the window. `+ use<>` promises the future owns what
it needs, which the claim's `Arc` already does, and is what the race test spawns.

### F2. `UpgradeHandler::release` is a public trait addition

Defaulted, so no implementor breaks. The alternative that touches no SPI is the table holding the
app weakly, which needs a weak `AppHandle` the core does not offer. Two HTTP servers preparing the
same hand-off: the later `prepare` replaces the table, and the first server's drop then releases the
table the second still uses. No shipped configuration binds two HTTP servers over one `WsModule`;
not probed.

### F3. `Dropped`'s text, and what it leaves unchanged

The text departs from the ruling's wording (D3). A `Dropped` close also leaves `AppHandle::phase()`
reading `Running` and `draining()` unfired, as a `close` after the drop already did, and nothing
fires `draining()` for a background loop holding a handle past an app dropped unclosed.

### F4. One owned-task type

The hub's `Drop` and `ulo-rpc-tcp-core`'s and `ulo-rpc-udp`'s `Owned` each abort a task handle on
drop. A `ulo_transport` type for it would give the next server one to reach for.

### F5. The in-memory adapter does not need the hub's abort

b06: with the `Weak` alone, the in-memory adapter's stream ends once the hub, the `Rooms` singleton
and the adapter binding drop, its sender with them, and the task returns. The abort is held by the
Redis test, whose sender lives in the adapter's own subscription task.

## Verification

Full output in the session scratchpad's `step5/verify/`; one cargo command at a time, `--locked`,
the OpenSSL flags set. No docker command was run and no broker suite.

- `cargo check --workspace --all-targets --locked`: exit 0, `ulo`'s 17 known warnings and no other.
- Tests, `cargo test -p <crate> --locked`, each exit 0: `ulo` 1 passed and 5 ignored (doctests);
  `ulo-runtime-tokio` 51 and 1, `app` 29 of them; `ulo-http` 20 and 9; `ulo-http-axum` 70 and 1;
  `ulo-ws` 118 and 4; `ulo-ws-redis` 3 and 1, its `integration` binary compiled without its feature
  and running none; the RPC links, `ulo-rpc` 54 and 4 (`closed_app` among them), `ulo-rpc-tcp` 172
  and 10, `ulo-rpc-udp` 58 and 7. The servers the two new `Drop`s reach: `ulo-http-poem` 70 and 1,
  `ulo-http-salvo` 71 and 2, `ulo-http-rocket` 66 and 5, `ulo-http-actix` 36 and 35, `ulo-http-hyper`
  89 and 9, `ulo-ws-hyper` 108 and 1.
- The race test three times on its own: 500 races each, every server closed, the drop ahead of the
  trigger in 500.
- `python3 scripts/runtime_levels.py --locked`: 64 members, 70 entries, 42 checked, 0 failures,
  0 errors, exit 0.
- `python3 scripts/unsafe_allowed.py check`: "self-test: 6 planted lines refused, 2 valid lines
  accepted", 573 files scanned, 2 listed, 0 findings, exit 0.
- `cargo +1.98.1 clippy -p ulo -p ulo-runtime-tokio -p ulo-http -p ulo-http-{axum,poem,salvo,rocket,actix}
  -p ulo-ws -p ulo-ws-redis --all-targets --no-deps --locked`: exit 0, 103 locations, none on a
  line this step added (the intersection with `git diff -U0` and the new files, read by a script).
- `RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --lib -p ulo -p ulo-http -p ulo-ws -p ulo-ws-redis
  --locked`: exit 0, after the embedding's `Drop` doc stopped linking the private `Shared`; that edit
  changed a doc comment only, after the test runs.
- The prose: batch 45's scan for the banned register and template braces, asserted first against a
  planted line (it flagged `we` and `just`), over every added line under `crates` and `docs`, the new
  test files, this log and the ledger's added lines: no finding. The scan checks vocabulary only.
