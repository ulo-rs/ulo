//! The Redis store for `ulo-db` (integrations DESIGN §4.5, §4.6): redis-rs's `ConnectionManager`,
//! bound bare.
//!
//! ```ignore
//! m.import(RedisModule::from_config(&config, "redis"));
//! ```
//!
//! The manager is built with `ConnectionManager::new_lazy_with_config`, which connects nothing; the
//! first command makes the connection. `create` therefore cannot fail on an unreachable server, the
//! store offers `.lazy()`, and under `.ready` the first ping is the first connection, retried by
//! `.ready` under `connect_within`. A wrong password is `Permanent` and ends the start at its first
//! attempt. The store has no `.migrate(..)`.
//!
//! `set_number_of_retries` is 0 (PROBES_db.md sections 9 and 9.1), so `.ready` is the one retry
//! policy. The setting governs every reconnect as well as the first connection, but a brief outage
//! costs one failed command at every setting and recovery takes the same time, so the library's
//! retries add only a longer wait before a failure: a wrong password fails in 2 ms at 0, 1.2 s at 3
//! and 9.5 s at the default of 6.
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
//! read of the current preference. This crate's `smol-comp` feature compiles redis-rs's in beside
//! `tokio-comp`; with it on, the crate calls `prefer_tokio()` once per process and logs a refusal,
//! another crate's preference having been set first, at `warn`. With it off, nothing is called or
//! logged, and a dependency that calls `prefer_smol()` is not detected: redis-rs then spawns onto
//! smol's global executor, not the app's runtime.
//!
//! # TLS coverage
//!
//! No test exercises `rediss://`, with or without the `tls` feature. The suite runs against a
//! container without TLS.

mod options;
mod store;

pub use options::RedisOptions;
pub use store::Redis;

/// `StoreModule` for this crate's store.
pub type RedisModule = ulo_db::StoreModule<Redis>;
