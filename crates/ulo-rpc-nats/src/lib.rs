// Tests: `tests/` needs a live NATS, started by testcontainers, and is
// gated behind the `integration` feature. Without it the files compile to
// nothing and `cargo test -p ulo-rpc-nats` reports a clean run of none:
//
//     cargo test -p ulo-rpc-nats --features integration

mod nats_adapter;
mod nats_client_transport;
mod servers;

/// The subject carrying stream-cancel notices (ADR-0032). Every server
/// instance subscribes without a queue group, so each sees every notice and
/// only the instance holding the call acts on it.
pub(crate) const CANCEL_SUBJECT: &str = "ulo.rpc.cancel";

pub use nats_adapter::NatsAdapter;
pub use nats_client_transport::NatsClientTransport;
pub use servers::IntoNatsServers;
pub use ulo::{RpcAdapter, RpcClient, RpcClientTransport};
