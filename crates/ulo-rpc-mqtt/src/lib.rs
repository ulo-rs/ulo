// Tests: `tests/` needs a live MQTT broker, started by testcontainers, and
// is gated behind the `integration` feature. Without it the files compile
// to nothing and `cargo test -p ulo-rpc-mqtt` reports a clean run of none:
//
//     cargo test -p ulo-rpc-mqtt --features integration

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
pub use ulo::{RpcAdapter, RpcClient, RpcClientTransport};
