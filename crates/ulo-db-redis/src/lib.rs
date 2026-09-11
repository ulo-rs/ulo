// Tests: `tests/startup.rs` is hermetic — startup-failure reporting and
// connection-string redaction, contacting no server — and runs by default.
// `tests/health.rs` needs a live Redis and is gated:
//
//     cargo test -p ulo-db-redis --features integration

mod redact;

mod connection;
#[cfg(feature = "health")]
pub mod health;
mod module;

pub use module::RedisModule;

#[cfg(feature = "health")]
pub use health::RedisHealthIndicator;
pub use redis::{AsyncCommands, RedisError, RedisResult, aio::ConnectionManager};
