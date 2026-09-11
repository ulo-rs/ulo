// Tests: `tests/conformance.rs` implements `ulo_rpc_conformance::Broker`
// and stamps out the shared RPC case set — the same six cases every
// transport answers. It needs a live RabbitMQ from testcontainers, so it
// is gated behind the `integration` feature; without it the file compiles
// to nothing and cargo reports a clean run of none:
//
//     cargo test -p ulo-rpc-rabbitmq --features integration
//
// A case belongs in `ulo-rpc-conformance` when every transport owes it,
// and here only when it is specific to this one.

//! RabbitMQ (AMQP 0-9-1) transport for the Ulo RPC gateway.
//!
//! Unlike Redis Pub/Sub, AMQP carries request-response natively: a message
//! has a `reply_to` and `correlation_id`, and per-call metadata rides in the
//! AMQP headers table. So there is no envelope to invent — the payload is raw
//! `RpcData` bytes (as in NATS) and the addressing lives in the message
//! properties.
//!
//! - [`RabbitMqAdapter`] — server side; declares one queue per registered
//!   pattern (routed by the default exchange), consumes, and publishes replies
//!   to the delivery's `reply_to` with the matching `correlation_id`.
//! - [`RabbitMqClientTransport`] — client side; uses RabbitMQ direct reply-to
//!   (`amq.rabbitmq.reply-to`), so request-response needs no real reply queue.

mod rabbitmq_adapter;
mod rabbitmq_client_transport;
mod wire;

pub use rabbitmq_adapter::RabbitMqAdapter;
pub use rabbitmq_client_transport::RabbitMqClientTransport;
pub use ulo::{RpcAdapter, RpcClient, RpcClientTransport};
