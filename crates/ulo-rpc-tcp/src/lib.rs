// Tests: this crate has no `tests/` of its own. Nothing it does is
// observable without an application dispatching through it, so its
// behaviour is proved in `integration-tests`:
// `rpc_tcp.rs` and `rpc_tcp_stream.rs`.

mod tcp_adapter;
mod tcp_client_transport;

pub use tcp_adapter::TcpAdapter;
pub use tcp_client_transport::TcpClientTransport;
pub use ulo::rpc::{RpcAdapter, RpcClient, RpcClientTransport};
