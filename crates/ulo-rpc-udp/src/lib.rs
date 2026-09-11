// Tests: this crate has no `tests/` of its own. Nothing it does is
// observable without an application dispatching through it, so its
// behaviour is proved in `integration-tests`:
// `rpc_udp.rs` and `rpc_udp_stream.rs`.

mod udp_adapter;
mod udp_client_transport;

pub use udp_adapter::UdpAdapter;
pub use udp_client_transport::UdpClientTransport;
pub use ulo::{RpcAdapter, RpcClient, RpcClientTransport};
