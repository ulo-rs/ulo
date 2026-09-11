// Tests: `tests/conformance.rs` implements `ulo_rpc_conformance::Broker`
// and stamps out the shared RPC case set — the same six cases every
// transport answers. It needs a live Redis from testcontainers, so it
// is gated behind the `integration` feature; without it the file compiles
// to nothing and cargo reports a clean run of none:
//
//     cargo test -p ulo-rpc-redis --features integration
//
// A case belongs in `ulo-rpc-conformance` when every transport owes it,
// and here only when it is specific to this one.

//! Redis Pub/Sub transport for the Ulo RPC gateway.
//!
//! Redis Pub/Sub has no native request-reply: a publisher cannot address a
//! reply back to the caller the way NATS does with a reply-to inbox. This
//! transport emulates it with a correlation-keyed reply channel carried inside
//! a JSON request envelope. The envelope is also where per-call `metadata`
//! rides, since Redis Pub/Sub frames carry no headers of their own.
//!
//! - [`RedisAdapter`] — server side; subscribes one channel per registered
//!   pattern and publishes replies to the channel named in the request.
//! - [`RedisClientTransport`] — client side; runs a single background
//!   reply-router subscribed to all its in-flight reply channels at once.

mod redis_adapter;
mod redis_client_transport;
mod wire;

pub use redis_adapter::RedisAdapter;
pub use redis_client_transport::RedisClientTransport;
pub use ulo::{RpcAdapter, RpcClient, RpcClientTransport};
