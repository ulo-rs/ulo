//! The vocabulary a macro expansion names and a caller never writes.
//!
//! `#[use_guards]` and its siblings resolve to an entry the framework stores per handler, and the
//! expansion has to name that entry's type in the user's crate. None of these is a decision a
//! caller makes: the entries are how a resolved guard, interceptor or per-call target is carried
//! between `create` and dispatch, and [`DispatchSource`] is where a dispatch target's instance
//! comes from.
//!
//! [`Transport`] and its four markers sit here too. The one place outside this crate that names
//! them is the code `#[injectable]` writes for an enhancer carrying execution-scoped
//! dependencies.
//!
//! They are `pub` because the expansion lands outside this crate, and hidden because naming one by
//! hand means writing what a macro writes.

pub use crate::dispatch::source::{DispatchSource, execution_scoped_dependencies};
pub use crate::dispatch::transport::{
    Grpc, GrpcGuardEntry, GrpcInterceptorEntry, GuardEntry, GuardFactory, Http, HttpGuardEntry,
    HttpInterceptorEntry, InterceptorEntry, InterceptorFactory, Rpc, RpcGuardEntry,
    RpcInterceptorEntry, Transport, Ws, WsGuardEntry, WsInterceptorEntry,
};
