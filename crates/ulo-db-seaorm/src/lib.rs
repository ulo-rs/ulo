// Tests: `tests/startup.rs` is hermetic — startup-failure reporting and
// connection-string redaction, contacting no server — and runs by default.
// `tests/health.rs` needs a live Postgres and is gated:
//
//     cargo test -p ulo-db-seaorm --features integration

mod redact;

mod connection;
#[cfg(feature = "health")]
pub mod health;
mod module;

pub use module::SeaOrmModule;

#[cfg(feature = "health")]
pub use health::SeaOrmHealthIndicator;
pub use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set};
