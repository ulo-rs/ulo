// Tests: `tests/` needs a live Redis, started by testcontainers, and is
// gated behind the `integration` feature:
//
//     cargo test -p ulo-ws-redis --features integration
//
// Without it the file compiles to nothing, and a plain
// `cargo test -p ulo-ws-redis` runs only the wire-format pin in `message.rs`.

mod message;
mod module;
mod provider;
mod service;

pub use module::RedisBroadcastModule;
pub use service::{RedisBroadcastService, RedisBroadcastTarget};
