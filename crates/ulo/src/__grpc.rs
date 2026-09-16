//! What a `#[grpc_methods]` expansion names and a caller never writes.
//!
//! The generated tonic trait impl runs the pipeline, catches a panicking handler and scopes a
//! streaming reply to its execution. Each of those is a call into the framework that has to be
//! spelled in the user's crate, so each is `pub` — and hidden, because writing one by hand means
//! writing what the macro writes.

pub use crate::grpc::runtime::{
    IntoScoped, ScopedGrpcStream, catch_handler_panic, empty_enhancers, run_grpc_pipeline,
};
