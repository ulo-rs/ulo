// Tests: `tests/conformance.rs` implements `ulo_rpc_conformance::Broker`
// and stamps out the shared RPC case set — the same six cases every
// transport answers. It needs a live NATS from testcontainers, so it
// is gated behind the `integration` feature; without it the file compiles
// to nothing and cargo reports a clean run of none:
//
//     cargo test -p ulo-rpc-nats --features integration
//
// A case belongs in `ulo-rpc-conformance` when every transport owes it,
// and here only when it is specific to this one.

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
pub use ulo::rpc::{RpcAdapter, RpcClient, RpcClientTransport};
