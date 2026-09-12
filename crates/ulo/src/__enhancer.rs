//! The vocabulary a macro expansion names and a caller never writes.
//!
//! `#[use_guards]` and its siblings resolve to an entry the framework stores per handler, and the
//! expansion has to name that entry's type in the user's crate. None of these is a decision a
//! caller makes: the entries are how a resolved guard, interceptor or per-call target is carried
//! between `create` and dispatch, and [`DispatchSource`] is where a dispatch target's instance
//! comes from.
//!
//! They are `pub` because the expansion lands outside this crate, and hidden because naming one by
//! hand means writing what a macro writes.

pub use crate::traits::dispatch_source::{DispatchSource, request_scoped_dependencies};
pub use crate::traits::provider::{
    DynGrpcGuardFactory, DynGrpcInterceptorFactory, DynHttpGuardFactory, DynHttpInterceptorFactory,
    DynRpcGuardFactory, DynRpcInterceptorFactory, DynWsGuardFactory, DynWsInterceptorFactory,
    GrpcGuardEntry, GrpcInterceptorEntry, HttpGuardEntry, HttpInterceptorEntry, RpcGuardEntry,
    RpcInterceptorEntry, WsGuardEntry, WsInterceptorEntry,
};
