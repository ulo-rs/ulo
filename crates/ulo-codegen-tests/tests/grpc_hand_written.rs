//! A controller written without the attribute, through `GrpcHandler` and `HandlerSpec`, beside its
//! `#[ulo_grpc::method]` twin: each of the four call shapes, a controller-tier guard, a method-tier
//! interceptor and error handler and declared metadata on both tiers answer the same and report the
//! same `HandlerInfo`, route and shape taken from the method marker. The server refuses, naming
//! the controller, a spec whose route or shape is not the marker's and a handler value that is not
//! a `GrpcHandler`.

mod support;

use std::error::Error;
use std::fmt;

use futures_util::{Stream, StreamExt, stream};
use http::HeaderValue;
use tonic::transport::Channel;
use tonic::{Code, Status};
use ulo::app::Bound as Listening;
use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dependencies, EnhancerSpec, ErrorHandler, Guard, HandlerInfo,
    HandlerSpec, Interceptor, MetaTier, Metadata, Module, ModuleDef, ModuleIdentity, Mount, Next, Resolver, Shape,
    StartupError, TypeName, injectable, routes,
};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{self, AddRequest, CountRequest, Number, Sum};
use ulo_grpc::{CallError, Classify, FromCall, Grpc, GrpcCx, GrpcHandler, Message, Reply, Request, Response, Streaming};

use support::{Running, within};

/// The metadata key `add` echoes from the request onto its reply.
const ECHO: &str = "x-echo";

/// The metadata key whose presence the controller's guard refuses.
const DENY: &str = "x-deny";

#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the counter refused the input")
    }
}

impl Error for Refusal {}

/// Declared metadata, one value per tier.
struct Tag(&'static str);

struct NoDeny;

impl Guard<Grpc> for NoDeny {
    async fn can_activate(&self, cx: &GrpcCx) -> Result<bool, BoxError> {
        Ok(cx.metadata().get(DENY).is_none())
    }
}

struct Stamp;

impl Interceptor<Grpc> for Stamp {
    async fn intercept(&self, _cx: &GrpcCx, next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        let mut reply = next.run().await?;
        reply.headers_mut().insert("x-stamp", HeaderValue::from_static("yes"));
        Ok(reply)
    }
}

/// Answers the handler's own `Refusal` with FAILED_PRECONDITION and hands any other error on.
struct Mend;

impl ErrorHandler<Grpc> for Mend {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        let refused = err.downcast_ref::<CallError>().and_then(|error| error.source_as::<Refusal>()).is_some();
        if refused { Err(BoxError::from(Status::failed_precondition("mended"))) } else { Err(err) }
    }
}

#[injectable]
struct MacroCounter;

#[routes]
#[guards(value = NoDeny)]
#[meta(Tag("controller"))]
impl MacroCounter {
    #[ulo_grpc::method(pb::counter::Add)]
    #[interceptors(value = Stamp)]
    #[error_handlers(value = Mend)]
    #[meta(Tag("add"))]
    async fn add(&self, req: Request<AddRequest>) -> Result<Response<Sum>, Refusal> {
        let AddRequest { a, b } = *req.message();
        if a < 0 || b < 0 {
            return Err(Refusal);
        }
        let echo = req.metadata().get(ECHO).unwrap_or("").to_owned();
        Ok(Response::new(Sum { sum: a + b }).metadata(ECHO, &echo))
    }

    #[ulo_grpc::method(pb::counter::Count)]
    async fn count(&self, req: Message<CountRequest>) -> impl Stream<Item = Result<Number, Refusal>> {
        stream::iter((1..=i64::from(req.0.up_to)).map(|value| Ok(Number { value })))
    }

    #[ulo_grpc::method(pb::counter::Total)]
    async fn total(&self, items: Streaming<Number>) -> Result<Sum, Refusal> {
        let mut items = items;
        let mut sum = 0;
        while let Some(item) = items.next().await {
            sum += item.map_err(|_| Refusal)?.value;
        }
        Ok(Sum { sum })
    }

    #[ulo_grpc::method(pb::counter::Double)]
    fn double(&self, items: Streaming<Number>) -> impl Stream<Item = Result<Number, Refusal>> {
        items.map(|item| item.map(|n| Number { value: n.value * 2 }).map_err(|_| Refusal))
    }
}

struct HandCounter;

impl Construct for HandCounter {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(HandCounter)
    }
}

async fn add(cx: GrpcCx) -> Result<Reply, BoxError> {
    let req = <Request<AddRequest> as FromCall<Grpc>>::from_call(&cx).await?;
    cx.exec().get::<HandCounter>().await?;
    let AddRequest { a, b } = *req.message();
    if a < 0 || b < 0 {
        return Err(CallError::from(Refusal).into());
    }
    let echo = req.metadata().get(ECHO).unwrap_or("").to_owned();
    Ok(cx.reply(Response::new(Sum { sum: a + b }).metadata(ECHO, &echo)))
}

async fn count(cx: GrpcCx) -> Result<Reply, BoxError> {
    let Message(req) = <Message<CountRequest> as FromCall<Grpc>>::from_call(&cx).await?;
    Ok(cx.reply_stream(stream::iter((1..=i64::from(req.up_to)).map(|value| Ok::<_, Refusal>(Number { value })))))
}

async fn total(cx: GrpcCx) -> Result<Reply, BoxError> {
    let mut items = <Streaming<Number> as FromCall<Grpc>>::from_call(&cx).await?;
    let mut sum = 0;
    while let Some(item) = items.next().await {
        sum += item.map_err(|_| CallError::from(Refusal))?.value;
    }
    Ok(cx.reply(Sum { sum }))
}

async fn double(cx: GrpcCx) -> Result<Reply, BoxError> {
    let items = <Streaming<Number> as FromCall<Grpc>>::from_call(&cx).await?;
    Ok(cx.reply_stream(items.map(|item| item.map(|n| Number { value: n.value * 2 }).map_err(|_| Refusal))))
}

impl Controller for HandCounter {
    fn mount(m: &mut Mount<'_>) {
        let mut controller = EnhancerSpec::<Grpc>::new();
        controller.guard_value(NoDeny);
        let mut add_tier = EnhancerSpec::<Grpc>::new();
        add_tier.interceptor_value(Stamp).error_handler_value(Mend);
        let tagged = |method: Option<&'static str>| {
            let mut meta = Metadata::new();
            meta.controller(Tag("controller"));
            if let Some(method) = method {
                meta.method(Tag(method));
            }
            meta
        };
        m.handler(
            GrpcHandler::new::<pb::counter::Add, _, _>(add)
                .spec("add")
                .controller(controller.clone())
                .method(add_tier)
                .meta(tagged(Some("add"))),
        );
        m.handler(GrpcHandler::new::<pb::counter::Count, _, _>(count).spec("count").controller(controller.clone()).meta(tagged(None)));
        m.handler(GrpcHandler::new::<pb::counter::Total, _, _>(total).spec("total").controller(controller.clone()).meta(tagged(None)));
        m.handler(GrpcHandler::new::<pb::counter::Double, _, _>(double).spec("double").controller(controller).meta(tagged(None)));
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

fn numbers(values: &[i64]) -> impl Stream<Item = Number> + Send + 'static {
    stream::iter(values.iter().map(|&value| Number { value }).collect::<Vec<_>>())
}

fn denied<T>(message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(DENY, "1".parse().unwrap_or_else(|error| panic!("bad metadata value: {error}")));
    request
}

fn status_of<T: fmt::Debug>(answer: Result<T, Status>) -> (Code, String) {
    match answer {
        Err(status) => (status.code(), status.message().to_owned()),
        Ok(reply) => panic!("expected a status, got {reply:?}"),
    }
}

/// What one handler reports: transport, name, route, prefix, shape, and each declared `Tag` with
/// its tier.
#[derive(Debug, PartialEq)]
struct Reported {
    transport: &'static str,
    name: &'static str,
    route: Option<String>,
    prefix: Option<String>,
    shape: Shape,
    entries: Vec<(&'static str, MetaTier)>,
    tags: Vec<&'static str>,
}

impl Reported {
    fn of(info: &HandlerInfo) -> Reported {
        Reported {
            transport: info.transport(),
            name: info.name(),
            route: info.route().map(str::to_owned),
            prefix: info.prefix().map(str::to_owned),
            shape: info.shape(),
            entries: info.entries().collect(),
            tags: info.meta_all::<Tag>().map(|tag| tag.0).collect(),
        }
    }
}

/// Every answer the scenarios draw from one app, and what its handlers report.
#[derive(Debug, PartialEq)]
struct Observed {
    sum: (i64, Option<String>, Option<String>),
    refused: (Code, String),
    guarded_add: (Code, String),
    counted: Vec<i64>,
    guarded_count: (Code, String),
    total: i64,
    doubled: Vec<i64>,
    reported: Vec<Reported>,
}

async fn observe(register: fn(&mut ModuleDef<'_>), controller: TypeName) -> Observed {
    let app = Running::start(Root(register), ulo_grpc::Server::new("127.0.0.1:0")).await;
    let mut client = CounterClient::new(app.channel().await);
    let observed = within("the scenarios", scenarios(&mut client)).await;
    let mut reported: Vec<Reported> = app
        .handle
        .handlers()
        .iter()
        .filter(|info| info.controller().key().type_name() == controller)
        .map(|info| Reported::of(info))
        .collect();
    reported.sort_by_key(|reported| reported.name);
    app.stop().await;
    Observed { reported, ..observed }
}

async fn scenarios(client: &mut CounterClient<Channel>) -> Observed {
    let mut request = tonic::Request::new(AddRequest { a: 2, b: 3 });
    request.metadata_mut().insert(ECHO, "ping".parse().unwrap_or_else(|error| panic!("bad metadata value: {error}")));
    let reply = client.add(request).await.unwrap_or_else(|status| panic!("Add failed: {status:?}"));
    let header = |key: &str| reply.metadata().get(key).and_then(|value| value.to_str().ok()).map(str::to_owned);
    let (echo, stamp) = (header(ECHO), header("x-stamp"));
    let sum = (reply.into_inner().sum, echo, stamp);

    let refused = status_of(client.add(AddRequest { a: -1, b: 3 }).await);
    let guarded_add = status_of(client.add(denied(AddRequest { a: 2, b: 3 })).await);

    let replies = client.count(CountRequest { up_to: 3 }).await.unwrap_or_else(|status| panic!("Count failed: {status:?}"));
    let counted: Vec<i64> = replies
        .into_inner()
        .map(|item| item.unwrap_or_else(|status| panic!("a Count item failed: {status:?}")).value)
        .collect()
        .await;
    let guarded_count = status_of(client.count(denied(CountRequest { up_to: 3 })).await.map(|_| ()));

    let total = client.total(numbers(&[4, 5, 6])).await.unwrap_or_else(|status| panic!("Total failed: {status:?}"));
    let replies = client.double(numbers(&[1, 2, 3])).await.unwrap_or_else(|status| panic!("Double failed: {status:?}"));
    let doubled: Vec<i64> = replies
        .into_inner()
        .map(|item| item.unwrap_or_else(|status| panic!("a Double item failed: {status:?}")).value)
        .collect()
        .await;

    Observed {
        sum,
        refused,
        guarded_add,
        counted,
        guarded_count,
        total: total.into_inner().sum,
        doubled,
        reported: Vec::new(),
    }
}

#[tokio::test]
async fn a_hand_written_controller_answers_and_reports_as_its_attribute_written_twin() {
    let by_macro = observe(
        |m| {
            m.controller::<MacroCounter>();
        },
        TypeName::of::<MacroCounter>(),
    )
    .await;
    let by_hand = observe(
        |m| {
            m.controller::<HandCounter>();
        },
        TypeName::of::<HandCounter>(),
    )
    .await;

    assert_eq!(by_macro.sum, (5, Some("ping".to_owned()), Some("yes".to_owned())));
    assert_eq!(by_macro.refused, (Code::FailedPrecondition, "mended".to_owned()));
    assert_eq!(by_macro.guarded_add.0, Code::PermissionDenied);
    assert_eq!(by_macro.guarded_count.0, Code::PermissionDenied);
    assert_eq!(by_macro.counted, [1, 2, 3]);
    assert_eq!(by_macro.total, 15);
    assert_eq!(by_macro.doubled, [2, 4, 6]);
    let shapes: Vec<(&str, Option<&str>, Shape)> =
        by_macro.reported.iter().map(|reported| (reported.name, reported.route.as_deref(), reported.shape)).collect();
    assert_eq!(
        shapes,
        [
            ("add", Some("/codegen.v1.Counter/Add"), Shape::Unary),
            ("count", Some("/codegen.v1.Counter/Count"), Shape::ServerStreaming),
            ("double", Some("/codegen.v1.Counter/Double"), Shape::Bidi),
            ("total", Some("/codegen.v1.Counter/Total"), Shape::ClientStreaming),
        ]
    );
    for reported in &by_macro.reported {
        assert_eq!((reported.transport, reported.prefix.as_deref()), ("grpc", None), "{}", reported.name);
    }
    let add = by_macro.reported.iter().find(|reported| reported.name == "add").expect("`add` is reported");
    assert_eq!(add.tags, ["add", "controller"]);

    assert_eq!(by_hand, by_macro);
}

/// `listen()`'s refusal, as text.
async fn refused(register: fn(&mut ModuleDef<'_>)) -> String {
    let listened: Result<App<Listening>, StartupError> = App::builder(Root(register))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0"))
        .listen()
        .await;
    let Err(error) = listened else { panic!("listen() accepted an app it should refuse") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
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

controller!(Rerouted, |m| m.handler(
    GrpcHandler::new::<pb::counter::Add, _, _>(add).spec("add").route("/codegen.v1.Counter/Plus")
));
controller!(Reshaped, |m| m.handler(
    GrpcHandler::new::<pb::counter::Count, _, _>(count).spec("count").shape(Shape::Unary)
));
controller!(Routeless, |m| m.handler(HandlerSpec::<Grpc, _>::new("add", GrpcHandler::new::<pb::counter::Add, _, _>(add))));
controller!(Shapeless, |m| m.handler(
    HandlerSpec::<Grpc, _>::new("count", GrpcHandler::new::<pb::counter::Count, _, _>(count)).route("/codegen.v1.Counter/Count")
));
controller!(Unmarked, |m| m.handler(HandlerSpec::<Grpc, _>::new("lost", 7u32).route("/codegen.v1.Counter/Add")));

#[tokio::test]
async fn a_spec_reporting_another_route_than_its_marker_is_refused_naming_both() {
    let text = refused(|m| {
        m.controller::<Rerouted>();
    })
    .await;
    assert!(
        text.contains(
            "Rerouted::add`'s spec reports the route `/codegen.v1.Counter/Plus` and a unary shape, and its method marker \
             names `/codegen.v1.Counter/Add`, a unary method"
        ),
        "{text}"
    );
}

#[tokio::test]
async fn a_spec_reporting_another_shape_than_its_marker_is_refused_naming_both() {
    let text = refused(|m| {
        m.controller::<Reshaped>();
    })
    .await;
    assert!(
        text.contains(
            "Reshaped::count`'s spec reports the route `/codegen.v1.Counter/Count` and a unary shape, and its method \
             marker names `/codegen.v1.Counter/Count`, a server-streaming method"
        ),
        "{text}"
    );
}

#[tokio::test]
async fn a_spec_with_no_route_is_refused_naming_the_marker_s_path() {
    let text = refused(|m| {
        m.controller::<Routeless>();
    })
    .await;
    assert!(
        text.contains(
            "Routeless::add`'s spec reports no route and a unary shape, and its method marker names \
             `/codegen.v1.Counter/Add`, a unary method"
        ),
        "{text}"
    );
}

/// A spec that sets no shape reports the default, unary, which a streaming marker refuses.
#[tokio::test]
async fn a_spec_with_no_shape_for_a_streaming_marker_is_refused_naming_both() {
    let text = refused(|m| {
        m.controller::<Shapeless>();
    })
    .await;
    assert!(
        text.contains(
            "Shapeless::count`'s spec reports the route `/codegen.v1.Counter/Count` and a unary shape, and its method \
             marker names `/codegen.v1.Counter/Count`, a server-streaming method"
        ),
        "{text}"
    );
}

#[tokio::test]
async fn a_handler_value_that_is_not_a_grpc_handler_is_refused_naming_its_controller() {
    let text = refused(|m| {
        m.controller::<Unmarked>();
    })
    .await;
    assert!(text.contains("Unmarked::lost` is mounted for gRPC with a handler value that is not a `ulo_grpc::GrpcHandler`"), "{text}");
}
