//! The database policy (integrations DESIGN §4): one copy of connecting, retrying, closing, the
//! health check and the migration lock, over the core's readiness API. A driver crate supplies a
//! [`Store`] and names [`StoreModule`] for it; this crate names no driver and no lock primitive.
//!
//! # Phases
//!
//! | Phase | What a store module does |
//! | --- | --- |
//! | `wire` | validates its options, records every problem, registers the URL and its password as secrets, contributes its check |
//! | `connect` | checks the runtime's name; creates the client through `spawn_with`; pings under `.ready`; runs migrations under the lock when asked |
//! | destroy | closes the client through `spawn_with` under the hook's bound |
//!
//! An app on a runtime that does not answer `"tokio"` fails `connect` with [`UnsupportedRuntime`]
//! inside `ulo::Permanent`, so `ulo::is_permanent` tells it from the retryable start failures: an
//! unreachable database, and a wait for another replica's migration lock that reached the bound.
//!
//! # Several instances
//!
//! `Module::keyed::<Q>()` requalifies a store module's exports as `T @ Q`: two keyed modules give
//! `Dep<PgPool, Primary>` and `Dep<PgPool, Analytics>` and contribute two checks. A module read
//! from a section names its check after the store and the section, `postgres@database`, and one
//! built with `from_options` after the store alone, `postgres`. A keyed store module appends its
//! qualifier written short, `postgres @ Primary`, so two keyed instances print apart whichever way
//! they are built. The qualifier read is the store module's own: a store imported by an
//! application module that is itself keyed is not keyed, and two such instances built with
//! `from_options` print alike, as do two qualifiers whose last path segments match
//! (`a::Primary` and `b::Primary`).

mod connect;
mod error;
mod health;
mod migrate;
mod module;
mod store;

pub use error::{LockTimedOut, MigrationFailed, UnsupportedRuntime};
pub use migrate::{MIGRATION_LOCK_KEY, MigrationLock, Migrations};
pub use module::StoreModule;
pub use store::{LazyStore, Store};
