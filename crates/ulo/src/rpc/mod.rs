use crate::dispatch::Items;

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
/// `Items` is the shape every transport with a count uses (ADR-0049); this names RPC's
/// instantiation of it. An item can fail mid-stream because an RPC call has a correlation and a
/// canonical error envelope to carry one.
pub type RpcHandlerOutput = Items<RpcData, RpcError>;

/// What an RPC call answers with — the value the pipeline returns and the `R`
/// of [`Interceptor`](crate::enhancer::Interceptor) on this transport.
pub type RpcHandlerResult = Result<RpcHandlerOutput, RpcError>;

/// What an RPC handler may answer with.
///
/// Three kinds of value reach the wire as `RpcData`: an [`Items`](crate::dispatch::Items), one
/// `RpcData`, and anything serde can serialize. The third cannot be an ordinary impl, because
/// `RpcData` and `Result` are both serializable and a blanket over `Serialize` overlaps either.
/// The recognised types are chosen by method resolution instead, through the autoref pair in
/// [`fallback`](crate::rpc::fallback). The macro calls that pair; a handler names none of it.
mod into_output {
    use super::{RpcData, RpcHandlerOutput};
    use crate::dispatch::{Answer, IntoOutput, Items, Rpc};

    impl IntoOutput<Rpc> for RpcHandlerOutput {
        fn into_output(self) -> Answer<Rpc> {
            Ok(self)
        }
    }

    impl IntoOutput<Rpc> for RpcData {
        fn into_output(self) -> Answer<Rpc> {
            Ok(Items::One(self))
        }
    }

    impl IntoOutput<Rpc> for () {
        fn into_output(self) -> Answer<Rpc> {
            Ok(Items::Empty)
        }
    }
}

/// How the macro picks between [`IntoOutput<Rpc>`](crate::dispatch::IntoOutput) and serde.
///
/// `RpcData` is `Serialize` and so is `Result`, so the two cannot be told apart by trait bounds:
/// a blanket `impl<S: Serialize> IntoOutput<Rpc> for S` conflicts with the impl for either. Method
/// resolution can tell them apart, because it tries the shallower reference first. The macro emits
/// `(&&Answers(value)).ulo_answer()`, which reaches [`Answered`](fallback::Answered) when the value implements
/// `IntoOutput<Rpc>` and [`Serialized`](fallback::Serialized) only when it does not.
///
/// A handler's type decides what its value means, at this seam.
pub mod fallback {
    use super::{RpcData, RpcError};
    use crate::dispatch::{Answer, IntoOutput, Items, Rpc};

    /// Carries a handler's value to the two impls below.
    pub struct Answers<T>(pub std::cell::Cell<Option<T>>);

    impl<T> Answers<T> {
        pub fn new(value: T) -> Self {
            Self(std::cell::Cell::new(Some(value)))
        }

        fn take(&self) -> T {
            self.0
                .take()
                .expect("a handler's value is taken once, by the one call the macro emits")
        }
    }

    /// Reached first: the value says what it is.
    pub trait Answered {
        fn ulo_answer(self) -> Answer<Rpc>;
    }

    impl<T: IntoOutput<Rpc>> Answered for &&Answers<T> {
        fn ulo_answer(self) -> Answer<Rpc> {
            Answers::take(self).into_output()
        }
    }

    /// Reached when the value implements no `IntoOutput<Rpc>`: serde decides.
    pub trait Serialized {
        fn ulo_answer(self) -> Answer<Rpc>;
    }

    impl<T: serde::Serialize> Serialized for &Answers<T> {
        fn ulo_answer(self) -> Answer<Rpc> {
            match RpcData::from_serialize(&Answers::take(self)) {
                Ok(data) => Ok(Items::One(data)),
                Err(e) => Err(RpcError::Internal(e.to_string())),
            }
        }
    }
}
