//! What an integration crate implements to plug into the framework.
//!
//! A provider, a controller and a route are things the macros write for an app author and a
//! hand-written integration writes for itself — `ulo-config`, `ulo-db-prisma` and the GraphQL
//! crates all implement these directly. The DI vocabulary an app *uses* is `di`; this is what it
//! takes to *be* one of the things `di` hands out.

mod controller;
pub(crate) mod dispatch_source;
mod execution_result;
pub(crate) mod provider;

pub use self::controller::{Controller, ControllerFactory, Dispatch};
pub use self::execution_result::ExecutionResult;

pub use self::provider::{Injectable, Provider, ProviderFactory, ProviderRole};
pub use crate::adapter::{AdapterContext, BindTarget};
pub use crate::error::AdapterResult;

// The enhancer plumbing keeps an in-crate path; its public one is `__enhancer`, which is where a
// macro expansion names it.
pub(crate) use self::dispatch_source::DispatchSource;
pub(crate) use self::provider::{
    GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry, HttpErrorHandlerArc, HttpGuardEntry,
    HttpInterceptorEntry, RpcErrorHandlerArc, RpcGuardEntry, RpcInterceptorEntry,
    WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry,
};
