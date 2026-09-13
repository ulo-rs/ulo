// Tests: `tests/conformance.rs` implements `ulo_rpc_conformance::Broker`
// and stamps out the shared RPC case set — the same six cases every
// transport answers. It needs a live MQTT broker from testcontainers, so it
// is gated behind the `integration` feature; without it the file compiles
// to nothing and cargo reports a clean run of none:
//
//     cargo test -p ulo-rpc-mqtt --features integration
//
// A case belongs in `ulo-rpc-conformance` when every transport owes it,
// and here only when it is specific to this one.

//! MQTT v5 transport for the Ulo RPC gateway.
//!
//! MQTT v5 carries request-response natively: a PUBLISH can name a
//! `response_topic` and `correlation_data`, and per-call metadata rides in the
//! v5 `user_properties`. So there is no envelope — the body is raw `RpcData`
//! bytes and the addressing lives in the PUBLISH properties.
//!
//! Both halves drive a rumqttc event loop: the loop must be polled
//! continuously for queued publishes to transmit and for incoming messages
//! (requests on the server, replies on the client) to arrive.
//!
//! - [`MqttAdapter`] — server side; subscribes one topic per registered
//!   pattern and publishes replies to the request's `response_topic`.
//! - [`MqttClientTransport`] — client side; subscribes a private reply topic
//!   and routes replies back by `correlation_data`.

mod mqtt_adapter;
mod mqtt_client_transport;
mod wire;

pub use mqtt_adapter::MqttAdapter;
pub use mqtt_client_transport::MqttClientTransport;
pub use ulo::rpc::{RpcAdapter, RpcClient, RpcClientTransport};
