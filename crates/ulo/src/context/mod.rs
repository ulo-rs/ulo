//! The context one execution runs in.
//!
//! Each transport (HTTP, RPC, WebSocket, gRPC) has its own concrete context type with
//! transport-specific fields, and [`StandaloneContext`] is the one for an execution with no
//! transport behind it. They all implement [`HandlerContext`], the universal interface that lets a
//! single enhancer (guard / interceptor / error handler) be written for one transport, all four, or
//! a chosen subset.

mod cancellation;
mod extensions;
mod handler_context;
mod metadata;
pub(crate) mod shared;
mod standalone;

pub use self::cancellation::CancellationToken;
pub use self::extensions::Extensions;
pub use self::handler_context::HandlerContext;
pub use self::metadata::Metadata;
pub use self::standalone::StandaloneContext;

#[cfg(test)]
mod handle_bounds_tests {
    use super::*;
    use crate::grpc::GrpcContext;
    use crate::http::HttpContext;
    use crate::rpc::RpcContext;
    use crate::ws::WsContext;

    fn assert_handle<T: Send + Sync + Clone + 'static>() {}

    /// Every enhancer signature takes `&C` across an await, and `&T` is `Send`
    /// only where `T` is `Sync`. A context losing `Sync` breaks the whole
    /// enhancer surface, and the error would surface far from the cause.
    #[test]
    fn every_context_is_a_send_sync_clone_handle() {
        assert_handle::<HttpContext>();
        assert_handle::<RpcContext>();
        assert_handle::<WsContext>();
        assert_handle::<GrpcContext>();
    }
}
