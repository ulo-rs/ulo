//! The MongoDB store for `ulo-db` (integrations DESIGN §4.6): `mongodb::Client`, bound bare.
//!
//! ```ignore
//! m.import(MongoModule::from_config(&config, "mongo"));
//! ```
//!
//! Whether `create` connects is decided by the URI. `Client::with_options` connects nothing, but
//! `ClientOptions::parse` resolves a `mongodb+srv` URL's DNS inside `create`:
//!
//! | URI | `create` | Start | `.lazy()` |
//! | --- | --- | --- | --- |
//! | `mongodb://` | resolves nothing | `.ready(ping)` retries under `connect_within` | offered |
//! | `mongodb+srv://` | resolves DNS, retried by `ulo-db`'s loop under `connect_within` | one confirming ping after the loop | refused at registration, naming the scheme |
//!
//! The store has no `.migrate(..)`: MongoDB has no migrator.
//!
//! MongoDB has no unknown-database error, since it creates a database on first write. A database
//! named in the URL with no `authSource` becomes the authentication source and fails as
//! `Authentication`.
//!
//! # Authenticated deployments
//!
//! The driver derives a SCRAM key on the thread that polls the connect. The stall scenario
//! (`connecting_does_not_stall_the_thread`, a one-thread runtime held to 50 ms) runs against a
//! server without authentication, since a debug build spends 55 to 80 ms on that derivation. Run
//! against a server with a root user, authenticating, in a release build, the longest gap between
//! two ticks of a 5 ms ticker was 7.8 to 8.0 ms over three runs, within the 50 ms bound. That is the
//! driver's cost, not the store's.
//!
//! # TLS coverage
//!
//! No test exercises a TLS connection to MongoDB. The suite runs against a container without TLS.

mod options;
mod store;

pub use options::MongoOptions;
pub use store::Mongo;

/// `StoreModule` for this crate's store.
pub type MongoModule = ulo_db::StoreModule<Mongo>;
