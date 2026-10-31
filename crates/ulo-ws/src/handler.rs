use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use ulo::{BoxError, BoxFuture, Mount};

use crate::connection::Connection;
use crate::gateway::{
    AfterInit, ConnectHandler, ConnectRefused, DisconnectReason, GatewayConfig, GatewayRef, OnConnect, OnDisconnect,
};
use crate::transport::{ConnectCx, Reply, WsCx};

/// A message handler's call: extraction, the controller, the handler and the reply, run by
/// `dispatch` after every guard admits.
pub(crate) type HandlerFn = Arc<dyn Fn(WsCx) -> BoxFuture<'static, Result<Reply, BoxError>> + Send + Sync>;

/// One message handler of an envelope gateway: the value a controller's
/// [`mount`](ulo::Controller::mount) passes to [`HandlerSpec::new`](ulo::HandlerSpec::new),
/// which the hand-off and the standalone server read back when they prepare.
/// `#[ulo_ws::message("chat.send")]` builds this value, and a gateway written without it builds it
/// by hand.
///
/// The event is the spec's [`route`](ulo::HandlerSpec::route): a message whose event field names it
/// reaches the call. A handler mounted with no event, `cancel` (the envelope's own), or an event
/// another handler of the gateway answers, is refused when the server prepares. The gateway's
/// connect handler is mounted once per controller type before the first message handler, through
/// `m.once::<G>(<G as GatewayConfig>::mount_gateway)`, as the attribute does.
///
/// The call runs once per message, in that message's execution, after every guard admits and
/// inside the interceptors. It reads its parameters through [`FromCall`](crate::FromCall),
/// [`Payload<T>`](crate::Payload) among them, its gateway through `cx.exec().get::<G>()`, and
/// answers a [`Reply`]: [`WsCx::reply`](crate::WsCx::reply) encodes one value with the gateway's
/// codec, [`WsCx::reply_stream`](crate::WsCx::reply_stream) a stream of them, and `Reply::None`
/// answers nothing.
///
/// ```
/// use ulo::scope::Auto;
/// use ulo::{BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Mount, Resolver};
/// use ulo_ws::{FromCall, GatewayConfig, GatewayHooks, GatewaySettings, Payload, Reply, Ws, WsCx, WsHandler};
///
/// struct Echo;
///
/// impl Construct for Echo {
///     type Scope = Auto;
///
///     fn dependencies(_: &mut Dependencies) {}
///
///     async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
///         Ok(Echo)
///     }
/// }
///
/// impl GatewayConfig for Echo {
///     fn settings() -> GatewaySettings {
///         GatewaySettings::at("/echo")
///     }
///
///     fn mount_gateway(m: &mut Mount<'_>) {
///         GatewayHooks::<Self>::new().mount(m);
///     }
/// }
///
/// async fn echo(cx: WsCx) -> Result<Reply, BoxError> {
///     let Payload(text) = <Payload<String> as FromCall<Ws>>::from_call(&cx).await?;
///     Ok(cx.reply(&text)?)
/// }
///
/// impl Controller for Echo {
///     fn mount(m: &mut Mount<'_>) {
///         m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
///         m.handler(HandlerSpec::<Ws, _>::new("echo", WsHandler::new(echo)).route("echo"));
///     }
/// }
/// ```
pub struct WsHandler {
    pub(crate) call: HandlerFn,
}

impl WsHandler {
    /// The handler answering its event with `call`.
    pub fn new<F, Fut>(call: F) -> Self
    where
        F: Fn(WsCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Reply, BoxError>> + Send + 'static,
    {
        WsHandler { call: Arc::new(move |cx| Box::pin(call(cx))) }
    }
}

/// `OnConnect::on_connect` as the connect handler calls it.
pub(crate) type OnConnectFn<G> = for<'a> fn(&'a G, &'a ConnectCx) -> BoxFuture<'a, Result<(), ConnectRefused>>;

/// `OnDisconnect::on_disconnect` as the connection's terminal execution calls it.
pub(crate) type OnDisconnectFn<G> = for<'a> fn(&'a G, &'a Connection, DisconnectReason) -> BoxFuture<'a, ()>;

/// `AfterInit::after_init` as the hand-off's `bound` or the standalone server calls it.
pub(crate) type AfterInitFn<G> = for<'a> fn(&'a G, GatewayRef) -> BoxFuture<'a, ()>;

/// The connection hooks an envelope gateway `G` runs, and the mount of its connect handler:
/// what [`GatewayConfig::mount_gateway`] writes for a gateway whose messages reach
/// [`WsHandler`]s. Each method names a hook trait `G` implements; one `G` does not implement is a
/// compile error at the call. `#[ulo_ws::gateway(..)]` writes the same calls for the hooks it finds
/// implemented.
///
/// A gateway implementing [`Gateway`](crate::Gateway) by hand, which reads every message raw,
/// mounts through `Gateway::mount` instead.
pub struct GatewayHooks<G> {
    pub(crate) on_connect: Option<OnConnectFn<G>>,
    pub(crate) on_disconnect: Option<OnDisconnectFn<G>>,
    pub(crate) after_init: Option<AfterInitFn<G>>,
    _gateway: PhantomData<fn() -> G>,
}

impl<G> GatewayHooks<G> {
    /// No hooks: every connection the connect guards admit is accepted.
    pub fn new() -> Self {
        GatewayHooks { on_connect: None, on_disconnect: None, after_init: None, _gateway: PhantomData }
    }

    /// Runs [`OnConnect::on_connect`] after the connect guards admit; its `Err` refuses the
    /// connection with its close code.
    pub fn on_connect(mut self) -> Self
    where
        G: OnConnect,
    {
        self.on_connect = Some(call_on_connect::<G>);
        self
    }

    /// Runs [`OnDisconnect::on_disconnect`] after each admitted connection ends.
    pub fn on_disconnect(mut self) -> Self
    where
        G: OnDisconnect,
    {
        self.on_disconnect = Some(call_on_disconnect::<G>);
        self
    }

    /// Runs [`AfterInit::after_init`] once, after `listen()`.
    pub fn after_init(mut self) -> Self
    where
        G: AfterInit,
    {
        self.after_init = Some(call_after_init::<G>);
        self
    }

    /// Mounts `G`'s connect handler under [`WsConnect`](crate::WsConnect), carrying its settings,
    /// connect guards, session factory and these hooks. Called inside `Mount::once::<G>`.
    pub fn mount(self, m: &mut Mount<'_>)
    where
        G: GatewayConfig,
    {
        crate::gateway::mount::<G>(m, ConnectHandler::attributed(self));
    }
}

impl<G> Default for GatewayHooks<G> {
    fn default() -> Self {
        GatewayHooks::new()
    }
}

fn call_on_connect<'a, G: OnConnect>(gateway: &'a G, cx: &'a ConnectCx) -> BoxFuture<'a, Result<(), ConnectRefused>> {
    Box::pin(gateway.on_connect(cx))
}

fn call_on_disconnect<'a, G: OnDisconnect>(gateway: &'a G, conn: &'a Connection, why: DisconnectReason) -> BoxFuture<'a, ()> {
    Box::pin(gateway.on_disconnect(conn, why))
}

fn call_after_init<G: AfterInit>(gateway: &G, gw: GatewayRef) -> BoxFuture<'_, ()> {
    Box::pin(gateway.after_init(gw))
}
