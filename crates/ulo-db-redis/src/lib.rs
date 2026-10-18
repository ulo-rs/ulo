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
//! The crate enables redis-rs's `tokio-comp` alone. An unrelated crate in the build can still
//! compile `smol-comp` in, and redis-rs then chooses a runtime through a process-global
//! preference: `prefer_tokio()` or `prefer_smol()` if some crate set one, otherwise tokio where a
//! tokio context exists. Absent a preference the choice is tokio here, because the connection is
//! made on the app's runtime after `ulo-db`'s check that it is tokio's.
//!
//! redis-rs 1.7.1 defines `prefer_tokio()` only when both features are compiled in, and offers no
//! read of the current preference, so this crate neither sets nor reads one. A dependency that calls
//! `prefer_smol()` is not detected, and redis-rs then spawns onto smol's global executor, not the
//! app's runtime. The crate keeps a process-wide guard so the step runs once and every Redis module
//! reads one recorded outcome.

mod options;
mod store;

pub use options::RedisOptions;
pub use store::Redis;

/// `StoreModule` for this crate's store.
pub type RedisModule = ulo_db::StoreModule<Redis>;
