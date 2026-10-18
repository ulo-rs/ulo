//! The MongoDB store for `ulo-db` (integrations DESIGN §4.6): `mongodb::Client`, bound bare.
//!
//! ```ignore
//! m.import(MongoModule::from_config(&config, "mongo"));
//! ```
//!
//! `CREATE_CONNECTS` is `true` for the whole store, a `const`, though `Client::with_options`
//! connects nothing: `ClientOptions::parse` resolves a `mongodb+srv` URL's DNS in `create`, so a
//! lookup failure is retried by `ulo-db`'s loop under `connect_within`, and a plain `mongodb://` URL
//! cannot fail transiently there, so the loop's first attempt succeeds at no cost. The store
//! therefore has no `.lazy()`, and no `.migrate(..)`: MongoDB has no migrator.
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
