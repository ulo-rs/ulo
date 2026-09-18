use crate::dispatch::Cardinality;

mod adapter;
mod client_transport;
mod context;
mod extractors;
mod lifecycle;
mod rpc_call_info;
mod rpc_client;
mod rpc_client_error;
mod rpc_controller;
mod rpc_controller_source;
mod rpc_controller_wrapper;
mod rpc_data;
mod rpc_error;
mod rpc_reply_stream;
pub mod wire;

pub use adapter::{RpcAdapter, RpcMessageCallbacks};
pub use client_transport::RpcClientTransport;
pub use context::RpcContext;
pub use extractors::PayloadError;
pub use lifecycle::RpcLifecycleHandle;
pub use rpc_call_info::RpcCallInfo;
pub use rpc_client::{RpcClient, RpcRequest};
pub use rpc_client_error::RpcClientError;
pub use rpc_controller::RpcController;
pub use rpc_controller_source::{RpcControllerSource, RpcEnhancers, RpcHandlerEnhancers};
pub(crate) use rpc_controller_wrapper::RpcControllerWrapper;
pub use rpc_data::RpcData;
pub use rpc_error::RpcError;
pub use rpc_reply_stream::{ReplySink, RpcReplyStream};

/// What an RPC handler answers with: nothing, one reply, or a stream of them.
///
/// `Cardinality` is the shape every transport with a count uses (ADR-0049); this names RPC's
/// instantiation of it. An item can fail mid-stream because an RPC call has a correlation and a
/// canonical error envelope to carry one.
pub type RpcHandlerOutput = Cardinality<RpcData, RpcError>;

/// What an RPC call answers with — the value the pipeline returns and the `R`
/// of [`Interceptor`](crate::enhancer::Interceptor) on this transport.
pub type RpcHandlerResult = Result<RpcHandlerOutput, RpcError>;
