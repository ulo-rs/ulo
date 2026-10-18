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
//! inside `ulo::Permanent`, so `ulo::is_permanent` tells it from an unreachable database, which is
//! the one retryable start failure.
//!
//! # Several instances
//!
//! `Module::keyed::<Q>()` requalifies a store module's exports as `T @ Q`: two keyed modules give
//! `Dep<PgPool, Primary>` and `Dep<PgPool, Analytics>` and contribute two checks.

mod connect;
mod error;
mod health;
mod migrate;
mod module;
mod store;

pub use error::{MigrationFailed, UnsupportedRuntime};
pub use migrate::{MIGRATION_LOCK_KEY, MigrationLock, Migrations};
pub use module::StoreModule;
pub use store::{LazyStore, Store};
