//! Support for the code `ulo-ws-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::any::TypeId;
use std::cell::Cell;
use std::marker::PhantomData;

use futures_core::Stream;
use futures_util::StreamExt;
use serde::Serialize;
use ulo::{BoxError, Mount, Transport};
use ulo_transport::{CallError, ErrorKind, IntoReply};

pub use ulo_transport as transport;

use crate::gateway::{AfterInit, ConnectHandler, Gateway, OnConnect, OnDisconnect};
use crate::handler::GatewayHooks;
use crate::transport::{Reply, Ws, WsCx};

/// `(&&HookProbe::<Self>::new()).__ulo_on_connect(hooks)` and its two siblings:
/// `hooks.on_connect()` where the gateway type implements the hook's trait, `hooks` unchanged
/// otherwise. Every probe method here is named `__ulo_*`, so it does not meet a user trait's
/// method of a common name at the call (`ulo::__private`).
pub struct HookProbe<G>(PhantomData<fn() -> G>);

impl<G> HookProbe<G> {
    pub fn new() -> Self {
        HookProbe(PhantomData)
    }
}

impl<G> Default for HookProbe<G> {
    fn default() -> Self {
        HookProbe::new()
    }
}

pub trait ViaOnConnect<G> {
    fn __ulo_on_connect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G>;
}

impl<G: OnConnect> ViaOnConnect<G> for &HookProbe<G> {
    fn __ulo_on_connect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G> {
        hooks.on_connect()
    }
}

pub trait NoOnConnect<G> {
    fn __ulo_on_connect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G>;
}

impl<G> NoOnConnect<G> for HookProbe<G> {
    fn __ulo_on_connect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G> {
        hooks
    }
}

pub trait ViaOnDisconnect<G> {
    fn __ulo_on_disconnect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G>;
}

impl<G: OnDisconnect> ViaOnDisconnect<G> for &HookProbe<G> {
    fn __ulo_on_disconnect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G> {
        hooks.on_disconnect()
    }
}

pub trait NoOnDisconnect<G> {
    fn __ulo_on_disconnect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G>;
}

impl<G> NoOnDisconnect<G> for HookProbe<G> {
    fn __ulo_on_disconnect(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G> {
        hooks
    }
}

pub trait ViaAfterInit<G> {
    fn __ulo_after_init(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G>;
}

impl<G: AfterInit> ViaAfterInit<G> for &HookProbe<G> {
    fn __ulo_after_init(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G> {
        hooks.after_init()
    }
}

pub trait NoAfterInit<G> {
    fn __ulo_after_init(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G>;
}

impl<G> NoAfterInit<G> for HookProbe<G> {
    fn __ulo_after_init(&self, hooks: GatewayHooks<G>) -> GatewayHooks<G> {
        hooks
    }
}

/// The `Mount::once` key of a hand-written gateway's connect handler, apart from the gateway
/// type's own key, which a `#[message]` mount function spends on `mount_gateway`.
pub struct HandWritten<G>(PhantomData<fn() -> G>);

/// What `Gateway::mount` runs once per controller type.
pub fn mount_hand_written<G: Gateway>(m: &mut Mount<'_>) {
    crate::gateway::mount::<G>(m, ConnectHandler::hand_written::<G>());
}

/// The paths `#[message]` gives `ulo-handler-codegen` for its generated call: `Param` and
/// `controller` as `ulo-transport` writes them, and the WebSocket reply probe in place of
/// `ulo-transport`'s, which reaches only `IntoReply` types.
pub mod reply {
    pub mod __private {
        pub use crate::__private::{IntoReplyProbe, ViaBoxError, ViaCallError, ViaValue};
        pub use ulo_transport::__private::{Param, ViaCall, ViaContainer, controller};
    }
}

/// The value-side marker of an [`Answer`] that is an `IntoReply<Ws>` type: `Frame`, `Reply`.
pub enum ViaReply {}
/// The value-side marker of an [`Answer`] that is a stream of `Result<T: Serialize, E>`.
pub enum ViaStream {}
/// The value-side marker of an [`Answer`] that is a `T: Serialize`, `()` included.
pub enum ViaSerde {}

/// What a message handler may answer with, besides the `Result` around it (transports DESIGN
/// §4.1): `()`, any `T: Serialize`, a `Frame` or `Reply`, and a stream of `Result<T, E>` with
/// `T: Serialize` and `E: Into<CallError>`.
///
/// `IntoReply<Ws>` cannot carry the serializable values and the streams itself: a blanket
/// `impl<T: Serialize> IntoReply<Ws> for T` is refused by the orphan rule, `T` being an uncovered
/// type parameter ahead of the local `Ws` (E0210), and the stream and serde blankets would
/// overlap besides (E0119). The marker `M` keeps the three blankets apart as three traits, and the
/// reply probe infers it from the one whose where-clauses the concrete value meets, as `Param<T, M>`
/// infers `ViaCall` or `ViaContainer`. A type that is both a stream and serializable is ambiguous
/// at the handler, a compile error there.
pub trait Answer<M>: Send + 'static {
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError>;
}

impl<V: IntoReply<Ws>> Answer<ViaReply> for V {
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError> {
        self.into_reply(cx).map_err(|err| BoxError::from(CallError::from(err)))
    }
}

/// Each item encoded by the gateway's codec; an `Err` item becomes the late path's error, and an
/// item that does not encode an internal one.
impl<S, T, E> Answer<ViaStream> for S
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: Serialize + Send + 'static,
    E: Into<CallError> + Send + 'static,
{
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError> {
        let codec = cx.conn().inner.gateway.settings().codec;
        let frames = self.map(move |item| match item {
            Ok(value) => codec.encode(&value).map_err(|err| BoxError::from(encode_failure(err))),
            Err(err) => {
                let err: CallError = err.into();
                Err(BoxError::from(err))
            }
        });
        Ok(Reply::Many(Box::pin(frames)))
    }
}

/// `()` is no answer, `Reply::None`; any other value is encoded by the gateway's codec.
impl<T: Serialize + Send + 'static> Answer<ViaSerde> for T {
    fn answer(self, cx: &WsCx) -> Result<Reply, BoxError> {
        if TypeId::of::<T>() == TypeId::of::<()>() {
            return Ok(Reply::None);
        }
        let codec = cx.conn().inner.gateway.settings().codec;
        codec.encode(&self).map(Reply::One).map_err(|err| BoxError::from(encode_failure(err)))
    }
}

fn encode_failure(err: BoxError) -> CallError {
    CallError::new(ErrorKind::Internal, "internal error").with_source(err)
}

/// The WebSocket reply probe, the shape of `ulo-transport`'s with [`Answer`] on the value side:
/// `(&&&IntoReplyProbe::<Ws, _>::new(out)).__ulo_into_reply(&cx)`. Three arms, each one
/// reference deeper than the priority reads: `Result<V, E: Into<CallError>>`,
/// `Result<V, E: Into<BoxError>>`, then any `V`. Each arm's trait carries the value marker `M`, inferred from `V: Answer<M>`.
pub struct IntoReplyProbe<T, V> {
    value: Cell<Option<V>>,
    _t: PhantomData<fn() -> T>,
}

impl<T, V> IntoReplyProbe<T, V> {
    pub fn new(value: V) -> Self {
        IntoReplyProbe { value: Cell::new(Some(value)), _t: PhantomData }
    }

    /// The value, once; a second call is answered as an internal error rather than a panic.
    fn take(&self) -> Result<V, BoxError> {
        self.value.take().ok_or_else(|| BoxError::from(CallError::new(ErrorKind::Internal, "internal error")))
    }
}

pub trait ViaCallError<M> {
    fn __ulo_into_reply(&self, cx: &<Ws as Transport>::Cx) -> Result<Reply, BoxError>;
}

pub trait ViaBoxError<M> {
    fn __ulo_into_reply(&self, cx: &<Ws as Transport>::Cx) -> Result<Reply, BoxError>;
}

pub trait ViaValue<M> {
    fn __ulo_into_reply(&self, cx: &<Ws as Transport>::Cx) -> Result<Reply, BoxError>;
}

impl<V: Answer<M>, E: Into<CallError>, M> ViaCallError<M> for &&IntoReplyProbe<Ws, Result<V, E>> {
    fn __ulo_into_reply(&self, cx: &WsCx) -> Result<Reply, BoxError> {
        match self.take()? {
            Ok(value) => value.answer(cx),
            Err(err) => {
                let err: CallError = err.into();
                Err(BoxError::from(err))
            }
        }
    }
}

impl<V: Answer<M>, E: Into<BoxError>, M> ViaBoxError<M> for &IntoReplyProbe<Ws, Result<V, E>> {
    fn __ulo_into_reply(&self, cx: &WsCx) -> Result<Reply, BoxError> {
        match self.take()? {
            Ok(value) => value.answer(cx),
            Err(err) => Err(err.into()),
        }
    }
}

impl<V: Answer<M>, M> ViaValue<M> for IntoReplyProbe<Ws, V> {
    fn __ulo_into_reply(&self, cx: &WsCx) -> Result<Reply, BoxError> {
        self.take()?.answer(cx)
    }
}
