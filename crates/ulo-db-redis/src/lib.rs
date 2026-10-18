//! The Redis store for `ulo-db` (integrations DESIGN §4.5, §4.6): redis-rs's `ConnectionManager`,
//! bound bare.
//!
//! ```ignore
//! m.import(RedisModule::from_config(&config, "redis"));
//! ```
//!
//! `ConnectionManager::new` connects, and retries for about 9.5 s by default, a wrong password
//! included. The store sets `CREATE_CONNECTS` and `ConnectionManagerConfig::set_number_of_retries(0)`,
//! so the manager connects once and `ulo-db`'s loop around `create` is the one retry loop. The store
//! therefore has no `.lazy()`, and no `.migrate(..)`.
//!
//! # The runtime preference
//!
//! With both of redis-rs's runtime features compiled in, which an unrelated crate in the build can
//! cause, redis-rs chooses a runtime through a process-global `OnceLock`. This crate calls
//! `prefer_tokio()` once per process, under a `OnceLock` of its own, from the first pool factory to
//! run at `connect`, after the runtime's name check, and every later Redis module reads the
//! recorded outcome. The preference is process-wide: it decides only which runtime redis-rs uses when
//! both features are compiled in, and an app already verified to be on tokio needs tokio. A
//! preference another crate set first is read where redis-rs allows, tokio continuing and any other
//! refused as `Permanent`; where it cannot be read, a `warn` names the conflict and the start
//! continues, and a non-tokio preference then fails the first query.

mod options;
mod store;

pub use options::RedisOptions;
pub use store::Redis;

/// `StoreModule` for this crate's store.
pub type RedisModule = ulo_db::StoreModule<Redis>;
