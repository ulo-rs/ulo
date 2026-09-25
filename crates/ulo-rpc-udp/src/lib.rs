// Tests: `tests/conformance.rs` stamps the shared RPC conformance suite
// against this adapter, through a proxy the test owns so a disruption can be
// made; it needs no broker and runs under a plain `cargo test -p ulo-rpc-udp`.
// A case belongs in `ulo-rpc-conformance` when every transport owes it. What
// is this transport's alone — a panicking handler on the wire, drain,
// backpressure, the stream grammar frame by frame, the binary refusal — is
// proved in `integration-tests`.

mod udp_adapter;
mod udp_client_transport;

pub use udp_adapter::UdpAdapter;
pub use udp_client_transport::UdpClientTransport;
pub use ulo::rpc::{RpcAdapter, RpcClient, RpcClientTransport};
