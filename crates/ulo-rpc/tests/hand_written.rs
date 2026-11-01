//! A controller written without the attributes, through `RpcHandler` and `HandlerSpec`: a unary
//! message, a streamed reply and a classified failure answer what their attribute-written twins
//! answer, with the same `HandlerInfo`; and the server refuses, naming the controller, a handler
//! mounted with no pattern, with a pattern no broker accepts, an event declared streaming, and a
//! binary payload on a JSON link.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use ulo::app::Bound as Listening;
use ulo::scope::Auto;
use ulo::{
    App, AppHandle, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef,
    ModuleIdentity, Mount, Resolver, Shape, Signal, StartupError, TypeName, injectable, routes,
};
use ulo_rpc::{Bytes, CallError, Classify, ErrorKind, FromCall, Payload, PayloadKind, Reply, Rpc, RpcClient, RpcCx, RpcHandler};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Debug, Classify)]
#[classify(conflict)]
struct Taken;

impl std::fmt::Display for Taken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the name is taken")
    }
}

impl std::error::Error for Taken {}

#[injectable]
struct Macro;

#[routes]
impl Macro {
    #[ulo_rpc::message("m.add")]
    fn add(&self, Payload((a, b)): Payload<(i64, i64)>) -> i64 {
        a + b
    }

    #[ulo_rpc::message("m.count")]
    fn count(&self, Payload(n): Payload<u32>) -> impl Stream<Item = Result<u32, CallError>> + use<> {
        stream::iter((1..=n).map(Ok))
    }

    #[ulo_rpc::message("m.claim")]
    fn claim(&self, Payload(_name): Payload<String>) -> Result<(), Taken> {
        Err(Taken)
    }
}

struct Hand;

impl Construct for Hand {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Hand)
    }
}

async fn add(cx: RpcCx) -> Result<Reply, BoxError> {
    let Payload((a, b)) = <Payload<(i64, i64)> as FromCall<Rpc>>::from_call(&cx).await?;
    cx.exec().get::<Hand>().await?;
    Ok(cx.reply(&(a + b))?)
}

async fn count(cx: RpcCx) -> Result<Reply, BoxError> {
    let Payload(n) = <Payload<u32> as FromCall<Rpc>>::from_call(&cx).await?;
    Ok(cx.reply_stream(stream::iter((1..=n).map(Ok::<_, CallError>))))
}

async fn claim(cx: RpcCx) -> Result<Reply, BoxError> {
    let Payload(_name) = <Payload<String> as FromCall<Rpc>>::from_call(&cx).await?;
    Err(CallError::from(Taken).into())
}

impl Controller for Hand {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Rpc, _>::new("add", RpcHandler::message(add)).route("h.add"));
        m.handler(HandlerSpec::<Rpc, _>::new("count", RpcHandler::message(count)).route("h.count").shape(Shape::ServerStreaming));
        m.handler(HandlerSpec::<Rpc, _>::new("claim", RpcHandler::message(claim)).route("h.claim"));
    }
}

struct Root(fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        (self.0)(m);
    }
}

/// `listen()`'s answer, its refusal boxed: a `StartupError` is large.
async fn listen(register: fn(&mut ModuleDef<'_>)) -> Result<App<Listening>, Box<StartupError>> {
    App::builder(Root(register))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .map_err(Box::new)
}

struct Running {
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: RpcClient,
}

async fn start(register: fn(&mut ModuleDef<'_>)) -> Running {
    let app = listen(register).await.expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");
    Running { handle, serving, client }
}

impl Running {
    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

fn both(m: &mut ModuleDef<'_>) {
    m.controller::<Macro>();
    m.controller::<Hand>();
}

#[tokio::test]
async fn hand_written_handlers_answer_as_their_attribute_written_twins() {
    let app = start(both).await;
    for (prefix, controller) in [("m", TypeName::of::<Macro>()), ("h", TypeName::of::<Hand>())] {
        let sum = tokio::time::timeout(PATIENCE, app.client.request::<_, i64>(&format!("{prefix}.add"), &(2i64, 3i64))).await;
        assert_eq!(sum.expect("in time").expect("an answer"), 5, "{prefix}");

        let counted = tokio::time::timeout(PATIENCE, async {
            let stream = app.client.stream::<_, u32>(&format!("{prefix}.count"), &3u32).await.expect("the stream opens");
            stream.map(|item| item.expect("an item")).collect::<Vec<u32>>().await
        })
        .await
        .expect("in time");
        assert_eq!(counted, [1, 2, 3], "{prefix}");

        let refused = tokio::time::timeout(PATIENCE, app.client.request::<_, ()>(&format!("{prefix}.claim"), &"ada")).await;
        let error = refused.expect("in time").expect_err("a refusal");
        assert_eq!(error.kind(), ErrorKind::Conflict, "{prefix}: {error}");
        assert_eq!(error.message(), "the name is taken", "{prefix}");

        let handlers = app.handle.handlers();
        let of = |name: &str| {
            handlers
                .iter()
                .find(|info| info.controller().key().type_name() == controller && info.name() == name)
                .unwrap_or_else(|| panic!("{prefix}: no handler `{name}`"))
                .clone()
        };
        for (name, shape) in [("add", Shape::Unary), ("count", Shape::ServerStreaming), ("claim", Shape::Unary)] {
            let info = of(name);
            assert_eq!(info.transport(), "rpc", "{prefix}.{name}");
            assert_eq!(info.route(), Some(format!("{prefix}.{name}").as_str()), "{prefix}.{name}");
            assert_eq!(info.prefix(), None, "{prefix}.{name}");
            assert_eq!(info.shape(), shape, "{prefix}.{name}");
            assert_eq!(info.entries().count(), 0, "{prefix}.{name}");
        }
    }
    app.stop().await;
}

/// `listen()`'s refusal and its text.
async fn refused(register: fn(&mut ModuleDef<'_>)) -> String {
    let Err(error) = listen(register).await else { panic!("listen() accepted an app it should refuse") };
    assert!(matches!(*error, StartupError::Configure(_)), "{error}");
    error.to_string()
}

macro_rules! controller {
    ($name:ident, |$m:ident| $mount:expr) => {
        struct $name;

        impl Construct for $name {
            type Scope = Auto;

            fn dependencies(_: &mut Dependencies) {}

            async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
                Ok($name)
            }
        }

        impl Controller for $name {
            fn mount($m: &mut Mount<'_>) {
                $mount;
            }
        }
    };
}

async fn nothing(_cx: RpcCx) -> Result<Reply, BoxError> {
    Ok(Reply::None)
}

controller!(Patternless, |m| m.handler(HandlerSpec::<Rpc, _>::new("lost", RpcHandler::message(nothing))));
controller!(Spaced, |m| m.handler(HandlerSpec::<Rpc, _>::new("spaced", RpcHandler::message(nothing)).route("orders created")));
controller!(Blank, |m| m.handler(HandlerSpec::<Rpc, _>::new("blank", RpcHandler::message(nothing)).route("")));
controller!(Controlled, |m| m.handler(HandlerSpec::<Rpc, _>::new("nul", RpcHandler::message(nothing)).route("orders\u{0}created")));
controller!(StreamedEvent, |m| m.handler(
    HandlerSpec::<Rpc, _>::new("noted", RpcHandler::event(nothing)).route("orders.noted").shape(Shape::ServerStreaming)
));
controller!(Binary, |m| m.handler(
    HandlerSpec::<Rpc, _>::new("raw", RpcHandler::message(nothing).payload(PayloadKind::Binary)).route("blobs.put")
));

#[tokio::test]
async fn a_handler_mounted_with_no_pattern_is_refused_naming_its_controller() {
    let text = refused(|m| {
        m.controller::<Patternless>();
    })
    .await;
    assert!(text.contains("Patternless::lost` is mounted for RPC with no pattern; `HandlerSpec::route` sets it"), "{text}");
}

#[tokio::test]
async fn a_pattern_holding_whitespace_is_refused_as_the_attribute_refuses_it() {
    let text = refused(|m| {
        m.controller::<Spaced>();
    })
    .await;
    assert!(text.contains("Spaced::spaced` has the pattern `orders created`, and a pattern holds no whitespace or control character"), "{text}");
}

#[tokio::test]
async fn an_event_declared_streaming_is_refused() {
    let text = refused(|m| {
        m.controller::<StreamedEvent>();
    })
    .await;
    assert!(text.contains("StreamedEvent::noted` (pattern `orders.noted`) is an `#[event]` handler with a server-streaming shape"), "{text}");
}

#[tokio::test]
async fn a_binary_payload_on_a_json_link_is_refused() {
    let text = refused(|m| {
        m.controller::<Binary>();
    })
    .await;
    assert!(text.contains("Binary::raw` (pattern `blobs.put`) takes a binary payload"), "{text}");
}

#[tokio::test]
async fn an_empty_pattern_is_refused_as_the_attribute_refuses_it() {
    let text = refused(|m| {
        m.controller::<Blank>();
    })
    .await;
    assert!(text.contains("Blank::blank` has the pattern ``, and a pattern is not empty"), "{text}");
}

#[tokio::test]
async fn a_pattern_holding_a_control_character_is_refused_as_the_attribute_refuses_it() {
    let text = refused(|m| {
        m.controller::<Controlled>();
    })
    .await;
    assert!(text.contains("Controlled::nul` has the pattern `orders\u{0}created`, and a pattern holds no whitespace or control character"), "{text}");
}

#[injectable]
struct BinaryMacro;

#[routes]
impl BinaryMacro {
    #[ulo_rpc::message("blobs.macro")]
    fn put(&self, Payload(_blob): Payload<Bytes>) {}
}

#[tokio::test]
async fn an_attribute_s_binary_payload_on_a_json_link_is_refused() {
    let text = refused(|m| {
        m.controller::<BinaryMacro>();
    })
    .await;
    assert!(text.contains("BinaryMacro::put` (pattern `blobs.macro`) takes a binary payload"), "{text}");
}
