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

mod options;
mod store;

pub use options::MongoOptions;
pub use store::Mongo;

/// `StoreModule` for this crate's store.
pub type MongoModule = ulo_db::StoreModule<Mongo>;
