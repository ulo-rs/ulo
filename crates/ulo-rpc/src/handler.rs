use std::future::Future;
use std::sync::Arc;

use ulo::{BoxError, BoxFuture};

use crate::frame::PayloadKind;
use crate::transport::{Reply, RpcCx};

/// A handler's call: extraction, the controller, the handler and the reply, run by `dispatch`
/// after every guard admits.
pub(crate) type HandlerFn = Arc<dyn Fn(RpcCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// Whether a handler answers calls or takes events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Request-reply, in any of the four shapes.
    Message,
    /// Fire-and-forget.
    Event,
}

/// One RPC handler: the value a controller's [`mount`](ulo::Controller::mount) passes to
/// [`HandlerSpec::new`](ulo::HandlerSpec::new), which the server reads back when it prepares.
/// `#[ulo_rpc::message]` and `#[ulo_rpc::event]` build this value, and a controller written
/// without them builds it by hand.
///
/// The pattern is the spec's [`route`](ulo::HandlerSpec::route) and the call shape the spec's
/// [`shape`](ulo::HandlerSpec::shape), `Unary` unless set, which a handler reading an
/// [`Inbound<T>`](crate::Inbound) or answering [`RpcCx::reply_stream`] declares to match. The
/// request frames a call is accepted in follow the declared shape, and so does the reply: a
/// `Reply::Many` from a handler declared `Unary` or `ClientStreaming` is refused where it is
/// written; one payload answering a streaming shape is not, since an interceptor or an error
/// handler may answer a streamed call that way. The caller gets `Internal` with the reason `shape_mismatch` and the
/// server logs one `warn` naming the handler. The server refuses, when it prepares, a handler with
/// no pattern or an empty one, a pattern holding whitespace or a control character, an event of
/// any shape but `Unary`, and a shape or a binary payload the link does not carry.
///
/// The call runs once per call, after every guard admits, and inside the interceptors. It reads
/// its parameters through [`FromCall`](crate::FromCall), its controller through
/// `cx.exec().get::<C>()`, and answers a [`Reply`]: [`RpcCx::reply`] encodes one value with the
/// link's codec, [`RpcCx::reply_stream`] a stream of them, and `Reply::None` answers nothing. An
/// error that classifies itself is converted with [`CallError::from`](crate::CallError) first, as
/// the attributes do, since a `BoxError` carries no kind of its own.
///
/// ```
/// use ulo::scope::Auto;
/// use ulo::{BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Mount, Resolver};
/// use ulo_rpc::{FromCall, Payload, Reply, Rpc, RpcCx, RpcHandler};
///
/// struct Adder;
///
/// impl Construct for Adder {
///     type Scope = Auto;
///
///     fn dependencies(_: &mut Dependencies) {}
///
///     async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
///         Ok(Adder)
///     }
/// }
///
/// async fn add(cx: RpcCx) -> Result<Reply, BoxError> {
///     let Payload((a, b)) = <Payload<(i64, i64)> as FromCall<Rpc>>::from_call(&cx).await?;
///     Ok(cx.reply(&(a + b))?)
/// }
///
/// impl Controller for Adder {
///     fn mount(m: &mut Mount<'_>) {
///         m.handler(HandlerSpec::<Rpc, _>::new("add", RpcHandler::message(add)).route("math.add"));
///     }
/// }
/// ```
pub struct RpcHandler {
    pub(crate) kind: Kind,
    pub(crate) call: HandlerFn,
    pub(crate) payload: PayloadKind,
}

impl RpcHandler {
    /// A request-reply handler, as `#[ulo_rpc::message]` declares one: its reply goes back to the
    /// caller, in whichever of the four shapes the spec declares.
    pub fn message<F, Fut>(call: F) -> Self
    where
        F: Fn(RpcCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Reply, BoxError>> + Send + 'static,
    {
        RpcHandler::of(Kind::Message, call)
    }

    /// A fire-and-forget handler, as `#[ulo_rpc::event]` declares one: one payload in, nothing
    /// answered, whatever the call returns.
    pub fn event<F, Fut>(call: F) -> Self
    where
        F: Fn(RpcCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Reply, BoxError>> + Send + 'static,
    {
        RpcHandler::of(Kind::Event, call)
    }

    fn of<F, Fut>(kind: Kind, call: F) -> Self
    where
        F: Fn(RpcCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Reply, BoxError>> + Send + 'static,
    {
        let call: HandlerFn = Arc::new(move |cx| Box::pin(call(cx)));
        RpcHandler { kind, call, payload: PayloadKind::Serde }
    }

    /// The payload the call reads: [`PayloadKind::Binary`] for a `Payload<Bytes>` or `Bytes`,
    /// which a link whose codec is JSON cannot carry; `Serde`, the default, for anything else.
    pub fn payload(mut self, kind: PayloadKind) -> Self {
        self.payload = kind;
        self
    }
}
