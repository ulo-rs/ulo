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
//! The driver derives a SCRAM key on the thread that polls the connect, and the store leaves it
//! there. The stall scenario (`connecting_does_not_stall_the_thread`, a one-thread runtime held to
//! 50 ms) runs against a server without authentication, since the driver's debug build spends about
//! 80 ms on that derivation. Measured against a Mongo 5.0.6 server with a root user and `--auth`,
//! over ten runs of the scenario with a 5 ms ticker, the longest gap between two ticks was:
//!
//! | Build | Min | Median | Max |
//! | --- | --- | --- | --- |
//! | release | 6.4 ms | 7.1 ms | 9.3 ms |
//! | debug | 75.7 ms | 78.0 ms | 87.2 ms |
//!
//! A release build stays far inside the bound, so the connect stays on the polling thread. A debug
//! build exceeds it: an application that runs a one-thread runtime in debug with authentication
//! sees a stall of about 80 ms per connect, the driver's cost and not the store's.
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
