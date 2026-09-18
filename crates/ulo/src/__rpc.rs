//! Bridge between a `#[controller]` struct and its `#[patterns]` impl.
//!
//! `#[patterns]` emits `handle_message` on the struct and a `RpcControllerSource` companion
//! carrying the patterns and the enhancer tokens; all three delegate to `Self::__ulo_rpc_*` at the
//! concrete type, through the inherent `__ulo_rpc_*` fns that out-rank the defaults below.
//! RPC has no connection hooks, so all three are derived from the impl scan — `#[patterns]` is
//! pure aggregation.
//!
//! What a controller *declares* — its patterns and its enhancer tokens — takes no receiver, because
//! the framework has to read it at startup to register the controller, and an execution-scoped
//! controller has no instance until a call arrives. Only `handle_message` needs one.
//!
//! Both forms carry the same constraint: the call site must name the concrete type. Reached through
//! a generic, `T::__ulo_rpc_patterns()` resolves to the default below and answers empty rather than
//! failing — see ADR 0001.

#![doc(hidden)]

use async_trait::async_trait;

use crate::context::Metadata;
use crate::dispatch::ExecutionResult;
use crate::rpc::RpcContext;
use crate::rpc::{RpcEnhancers, RpcError, RpcHandlerOutput};

/// Blanket "no patterns" defaults, implemented for every type. `#[patterns]` shadows these with
/// inherent fns of the same name, which win at the concrete-type call site in the generated
/// `RpcController` impl.
#[async_trait]
pub trait RpcHandlersBridge {
    fn __ulo_rpc_patterns() -> Vec<String>
    where
        Self: Sized,
    {
        Vec::new()
    }

    async fn __ulo_rpc_handle_message(
        &self,
        ctx: &RpcContext,
    ) -> ExecutionResult<RpcHandlerOutput, RpcError> {
        ExecutionResult::Err(RpcError::PatternNotFound(format!(
            "Unknown pattern: {}",
            ctx.pattern()
        )))
    }

    fn __ulo_rpc_enhancers() -> RpcEnhancers
    where
        Self: Sized,
    {
        RpcEnhancers::default()
    }

    fn __ulo_rpc_metadata() -> Metadata
    where
        Self: Sized,
    {
        Metadata::new()
    }

    fn __ulo_rpc_handler_metadata() -> Vec<(String, Metadata)>
    where
        Self: Sized,
    {
        Vec::new()
    }
}

impl<T: ?Sized + Sync> RpcHandlersBridge for T {}

/// How `#[patterns]` picks between [`IntoOutput<Rpc>`](crate::dispatch::IntoOutput) and serde.
///
/// `RpcData` is `Serialize` and so is `Result`, so the two cannot be told apart by trait bounds: a
/// blanket `impl<S: Serialize> IntoOutput<Rpc> for S` conflicts with the impl for either. Method
/// resolution can tell them apart, because it tries the shallower reference first. The macro emits
/// `(&&Answers::new(value)).ulo_answer()`, which reaches [`Answered`](answer::Answered) when the value implements
/// `IntoOutput<Rpc>` and [`Serialized`](answer::Serialized) only when it does not.
///
/// A handler's type decides what its value means, at this seam, and a handler names none of this.
pub mod answer {
    use crate::dispatch::{Answer, IntoOutput, Items, Rpc};
    use crate::rpc::{RpcData, RpcError};

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
