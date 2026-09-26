//! What a `#[grpc_methods]` expansion names and a caller never writes.
//!
//! The generated tonic trait impl runs the pipeline, catches a panicking handler and scopes a
//! streaming reply to its execution. Each of those is a call into the framework that has to be
//! spelled in the user's crate, so each is `pub` — and hidden, because writing one by hand means
//! writing what the macro writes.

pub use crate::grpc::runtime::{
    IntoScoped, ScopedGrpcStream, catch_handler_panic, empty_enhancers, run_grpc_pipeline,
};

/// Empties a call's request slot when the call's future ends, by returning or by being dropped.
///
/// The installed request is the tonic request, whose extensions hold this context: while it sits
/// in the slot the execution holds itself. An extractor taking it breaks that; a guard that
/// refuses, an interceptor that answers, a handler taking no request, or a call tonic drops first
/// does not, and without this the execution would outlive the call. A stream or detached work
/// that reads the request after the call's future ends gets `Taken`.
pub struct ReleaseRequest(pub crate::grpc::GrpcContext);

impl Drop for ReleaseRequest {
    fn drop(&mut self) {
        let _ = self.0.take_request();
    }
}
