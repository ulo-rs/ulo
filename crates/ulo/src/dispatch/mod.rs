//! What can be called, and what runs around it.
//!
//! Between the DI graph and the sockets that serve it: `di` says what exists, `dispatch` says what
//! a caller can reach and what happens on the way in and out, and the adapters carry the bytes.
//!
//! A transport is a type here ([`transport::Transport`]), which is what lets one registry, one
//! resolver and one pipeline serve four of them. What stays per transport is the part with no
//! shared shape — the leaf that calls the handler, and the render that turns an answer into bytes.

mod controller;
mod execution_result;
pub(crate) mod registry;
pub(crate) mod resolve;
pub(crate) mod source;
pub(crate) mod transport;

pub use self::controller::{Controller, ControllerFactory, Targets};
pub use self::execution_result::ExecutionResult;

pub(crate) use self::source::DispatchSource;
