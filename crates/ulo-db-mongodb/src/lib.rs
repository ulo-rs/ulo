// Tests: `tests/startup.rs` is hermetic — startup-failure reporting and
// connection-string redaction, contacting no server — and runs by default.
// `tests/health.rs` needs a live MongoDB and is gated:
//
//     cargo test -p ulo-db-mongodb --features integration

mod redact;

mod connection;
#[cfg(feature = "health")]
pub mod health;
mod module;

#[cfg(feature = "health")]
pub use health::MongoHealthIndicator;
pub use module::MongoModule;

pub use mongodb::{
    Collection, Database,
    bson::{Document, doc, oid::ObjectId},
    error::Error as MongoError,
    options::FindOptions,
};
