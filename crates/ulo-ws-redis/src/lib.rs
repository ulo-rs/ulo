// Tests: `tests/` needs a live Redis, started by testcontainers, and is
// gated behind the `integration` feature. Without it the files compile to
// nothing and `cargo test -p ulo-ws-redis` reports a clean run of none:
//
//     cargo test -p ulo-ws-redis --features integration

mod message;
mod module;
mod provider;
mod service;

pub use module::RedisBroadcastModule;
pub use service::{RedisBroadcastService, RedisBroadcastTarget};
