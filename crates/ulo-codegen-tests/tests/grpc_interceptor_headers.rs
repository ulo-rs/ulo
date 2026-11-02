//! A `grpc-` header an interceptor adds to a reply the framework wrote is dropped, one it rewrites
//! is written back, and each is logged at `warn` once per process per name, so an interceptor
//! cannot forge a status: a trailers-only reply keeps the status and message the framework wrote.
//!
//! In a file of its own: the once-per-process record would otherwise depend on which test of the
//! binary dropped a name first.

mod support;

use std::error::Error;
use std::fmt;

use futures_util::{Stream, StreamExt, stream};
use http::HeaderValue;
use tonic::{Code, Status};
use ulo::{BoxError, ErrorHandler, Interceptor, Module, ModuleDef, ModuleIdentity, Next, injectable, routes};
use ulo_codegen_tests::probe;
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::shapes_client::ShapesClient;
use ulo_codegen_tests::probe::{Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_transport::Classify;

use support::{Capture, Running};

#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused on purpose")
    }
}

impl Error for Refusal {}

/// Writes a status, a message and an unlisted `grpc-` name onto the reply it passes, and an
/// ordinary header.
struct Forge;

impl Interceptor<Grpc> for Forge {
    async fn intercept(&self, _cx: &GrpcCx, next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        let mut reply = next.run().await?;
        let headers = reply.headers_mut();
        headers.insert("grpc-status", HeaderValue::from_static("13"));
        headers.insert("grpc-message", HeaderValue::from_static("forged"));
        headers.insert("grpc-forged", HeaderValue::from_static("yes"));
        headers.insert("x-kept", HeaderValue::from_static("kept"));
        Ok(reply)
    }
}

/// Answers in the handler's place with a trailers-only status through the context, then rewrites
/// that status and its message and adds a `grpc-` name of its own.
struct Deny;

impl Interceptor<Grpc> for Deny {
    async fn intercept(&self, cx: &GrpcCx, _next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        let mut reply = cx.reply_status(Status::permission_denied("denied by the interceptor"));
        let headers = reply.headers_mut();
        headers.insert("grpc-status", HeaderValue::from_static("0"));
        headers.insert("grpc-message", HeaderValue::from_static("rewritten"));
        headers.insert("grpc-added", HeaderValue::from_static("yes"));
        Ok(reply)
    }
}

/// Claims the handler's `Refusal` with a trailers-only status through the context.
struct Mend;

impl ErrorHandler<Grpc> for Mend {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        let refused = err.downcast_ref::<ulo_transport::CallError>().and_then(|error| error.source_as::<Refusal>()).is_some();
        if refused { Ok(cx.reply_status(Status::failed_precondition("mended"))) } else { Err(err) }
    }
}

#[injectable]
struct Known;

#[routes]
#[error_handlers(grpc(value = Mend))]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    #[interceptors(value = Forge)]
    fn upper(&self, text: Message<String>) -> Result<String, Refusal> {
        if text.0 == "fail" {
            return Err(Refusal);
        }
        Ok(text.0.to_uppercase())
    }

    #[ulo_grpc::method(probe::known::Ping)]
    #[interceptors(value = Deny)]
    async fn ping(&self) {}
}

#[injectable]
struct Shapes;

#[routes]
impl Shapes {
    #[ulo_grpc::method(probe::shapes::Watch)]
    #[interceptors(value = Forge)]
    fn watch(&self, req: Message<Ticks>) -> impl Stream<Item = Result<Tick, Refusal>> {
        stream::iter((1..=req.0.count).map(|n| Ok(Tick { n })))
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Known>();
        m.controller::<Shapes>();
    }
}

fn header<'a>(metadata: &'a tonic::metadata::MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

/// How many captured warnings name `header`.
fn warned_about(capture: &Capture, header: &str) -> usize {
    capture.warnings().iter().filter(|line| line.contains("`grpc-` header") && line.ends_with(&format!("header={header}"))).count()
}

#[tokio::test]
async fn an_interceptor_neither_adds_a_grpc_name_nor_rewrites_one_the_framework_wrote_and_each_is_logged_once() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let mut client = KnownClient::new(app.channel().await);

    // Added to a reply carrying a message: dropped, so the forged status never ends the call.
    for _ in 0..2 {
        let reply = support::within("the Upper call", client.upper("ada".to_owned())).await;
        let reply = reply.unwrap_or_else(|status| panic!("the forged status reached the caller: {status:?}"));
        for dropped in ["grpc-message", "grpc-forged"] {
            assert_eq!(header(reply.metadata(), dropped), None, "{dropped} reached the caller: {:?}", reply.metadata());
        }
        // tonic hands a unary reply's trailers over with its headers: the `grpc-status` read here
        // is the trailer's.
        assert_eq!(header(reply.metadata(), "grpc-status"), Some("0"), "{:?}", reply.metadata());
        assert_eq!(header(reply.metadata(), "x-kept"), Some("kept"), "an ordinary header passes");
        assert_eq!(reply.into_inner(), "ADA");
    }

    // A trailers-only reply an interceptor answered through the context: the status and message
    // the context wrote stand against the interceptor's own rewrite, and the name it added goes.
    for _ in 0..2 {
        let status = support::within("the Ping call", client.ping(())).await.err().unwrap_or_else(|| panic!("Ping answered OK"));
        assert_eq!((status.code(), status.message()), (Code::PermissionDenied, "denied by the interceptor"), "{status:?}");
        assert_eq!(header(status.metadata(), "grpc-added"), None, "{:?}", status.metadata());
    }

    // An error handler claiming the handler's error through the context is answered as it wrote.
    let status = support::within("the failing Upper call", client.upper("fail".to_owned())).await;
    let status = status.err().unwrap_or_else(|| panic!("the failing Upper answered OK"));
    assert_eq!((status.code(), status.message()), (Code::FailedPrecondition, "mended"), "{status:?}");

    // A stream reply's headers are screened alike: the forged status does not end the call.
    let mut shapes = ShapesClient::new(app.channel().await);
    let reply = support::within("the Watch call", shapes.watch(Ticks { count: 2, fail: false })).await;
    let reply = reply.unwrap_or_else(|status| panic!("the forged status reached the caller on a stream: {status:?}"));
    for dropped in ["grpc-status", "grpc-message", "grpc-forged"] {
        assert_eq!(header(reply.metadata(), dropped), None, "{dropped} reached the caller: {:?}", reply.metadata());
    }
    assert_eq!(header(reply.metadata(), "x-kept"), Some("kept"), "an ordinary header passes");
    let items: Vec<u32> = support::within("the Watch items", reply.into_inner().collect::<Vec<_>>())
        .await
        .into_iter()
        .map(|item| item.unwrap_or_else(|status| panic!("a Watch item failed: {status:?}")).n)
        .collect();
    assert_eq!(items, [1, 2]);

    for name in ["grpc-status", "grpc-message", "grpc-forged", "grpc-added"] {
        assert_eq!(warned_about(&capture, name), 1, "{name}: {:?}", capture.warnings());
    }
    app.stop().await;
}
