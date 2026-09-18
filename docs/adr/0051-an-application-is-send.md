# 0051 — An application is Send

Status: accepted

Moves the container handle from `Rc<RefCell<_>>` to `Arc<RwLock<_>>` and bounds the three
bootstrap traits that kept it thread-local, so an application can be held, moved and awaited
anywhere a `Send` value can.

## Context

`UloApplication` is `!Send`. The consequence is documented where it bites: run under a
`current_thread` runtime inside `LocalSet::run_until`. The row in `CLAUDE.md` states it, and
`ulo-grpc`'s crate docs and README prescribe it in their bootstrap example.

One field causes it. `container: Rc<RefCell<Container>>` is held by `UloApplication`,
`UloApplicationContext`, `RouteMount` and the three controller resolvers. Nothing else in the
application is thread-local: the four adapter traits, `ServerLifecycle`, `Provider`, `Controller`,
`Gateway`, `Route`, `RpcController`, `Middleware` and every enhancer trait already carry
`Send + Sync`, and `ServerLifecycle::take_serve` already returns a `Send` future.

What that costs is composition. No bound on the serve path is involved, and how many requests run at
once is decided by the runtime flavor and the adapter's own per-connection spawning — neither of
which this changes. The cost is that the application cannot be *moved* into a task, so it must be
the outermost future or sit inside a `LocalSet`. Three shapes pay for that:

- **A server and a client in one process.** Reachable today at the price of scaffolding: seven
  examples, the five HTTP adapter integration test suites, the five database health suites, the core
  module-identity tests and the RPC conformance suite each carry a `LocalSet` for this reason alone.
- **Embedding.** An existing service cannot hold a `UloApplication` in a `Send` struct, share it
  across tasks, or place it under a supervisor.
- **Resolution off the main thread.** `UloApplicationContext` is the DI root with nothing served —
  what a CLI tool, a job or a worker builds ([ADR-0022](0022-an-execution-without-a-transport.md)).
  Those are the shapes most likely to resolve from a pool thread, and the handle cannot cross one.

The bound that is missing was already named. PR #117 separated two castes: **machinery** —
bootstrap-built and shared — carries `Send + Sync + 'static`; **flow data** — the per-execution
request, body, context and response — is bounded by what its own seam needs, and was reasoned about
separately. `ModuleMetadata`, `ProviderFactory` and `ControllerFactory` are machinery by that test.
They never took the bound because `Rc` meant nothing asked them for it.

## Decision

**An application is `Send`.** Every change is on the machinery caste:

1. `ModuleMetadata`, `ProviderFactory` and `ControllerFactory` gain `Send + Sync`.
2. `ModuleMetadata` drops `#[async_trait(?Send)]` for `#[async_trait]`, so a module's lifecycle
   hooks return `Send` futures the way a provider's already did.
3. The container handle becomes `Arc<RwLock<Container>>`, using the `parking_lot` lock the crate
   already depends on.
4. A module holds its metadata and its two factory maps as `Arc` handles rather than `Box`
   values, so each can be cloned out and the lock released before the await that uses it.

No flow-data type changes: contexts, bodies, streams and responses keep the bounds they have.

The lock is a read-write lock because the access pattern is asymmetric and already separated.
`Container` is mutated while the application is built and configured — scan, instance loading, and
the route drain in `use_http_adapter` — and is read-only from `bind` onward; `application.rs` and
`application_context.rs` take no write lock at all. Runtime resolution therefore takes read locks
that never contend with each other.

A lock guard is `!Send`, so a guard held across an `.await` is a compile error rather than a
silently thread-pinned future. That is the intended discipline: the resolution path already clones
the provider `Arc` out and drops its borrow before awaiting, and the sites that do not are reported
by the compiler.

## Consequences

An out-of-tree implementation of `ModuleMetadata`, `ProviderFactory` or `ControllerFactory` that is
not `Send + Sync` stops compiling, as does a module lifecycle hook whose body holds a non-`Send`
value across an await. Every implementation in this workspace, macro-generated and hand-written
alike, already satisfies both.

Two kinds of loop held a guard across an await and now take handles first: the lifecycle hooks that
iterate modules, and the DI loader, which awaits `ProviderFactory::build` and
`ControllerFactory::build` once per provider and per controller. The second is the one a test
reaches only by building the application *inside* the spawned task — building it first and spawning
the serve future passes while the loader still holds a guard, which is why
`an_application_is_send.rs` does both.

`LocalSet` leaves the conformance harness, the module-identity tests and the gRPC documentation
here. The other 116 files carrying it — 57 reaching for it through `tokio_localset_test` alone, 59
building one by hand, the seven examples among them — keep working unchanged and are not touched by
this change.

## Roads not taken

**`Arc<Mutex<Container>>`.** Simpler to write, and it funnels every runtime resolution through one
exclusive lock. The application would gain the ability to run across threads and immediately
serialize its DI lookups.

**Freezing to `Arc<Container>` after build.** No lock at all on the serve path, and the better end
state if resolution ever measures as hot. It is not reachable by construction today: the route
drain in `use_http_adapter` mutates the container after `create` returns, so the freeze point is
after configuration rather than at the end of the build, while `UloApplication` and
`UloApplicationContext` both take their handle before it. That is a state transition in two places
for a cost that a read lock does not yet impose. Available later; the decision here does not
foreclose it.

**Draining routes at build instead of bind, to move the freeze point earlier.** The drain is
load-bearing: it makes the mount the unique owner of each `Arc<RoutePipeline>`, so `Arc::get_mut`
succeeds and route middleware is applied. Cloning instead returns `None` from `get_mut` and drops
middleware with no error.
