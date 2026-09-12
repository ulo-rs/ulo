//! Everything that is gRPC and nothing that is not: the status a call answers with, the context
//! one call runs in, the adapter trait an integration crate implements, what a service declares,
//! and the extractor only a gRPC handler can ask for.
//!
//! What a gRPC handler shares with the other transports — `Guard`, `Interceptor`, `FromContext`,
//! `Payload`, `ExecutionResult` — is in the crate's core, because it means the same thing there.

mod adapter;
mod context;
pub mod extract;
mod lifecycle;
pub(crate) mod runtime;
mod service_source;
mod status;

pub use adapter::{GrpcAdapter, GrpcMethodPath};
pub use context::GrpcContext;
pub use lifecycle::GrpcLifecycleHandle;
pub use runtime::{GrpcFailure, RequestCarrier, RequestError};
pub use service_source::{
    GrpcEnhancers, GrpcHandlerEnhancers, GrpcServiceSource, ResolvedGrpcEnhancers,
};
pub use status::{GrpcCode, GrpcHandlerResult, GrpcStatus, error_kind, grpc_code};
