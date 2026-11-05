//! A `grpc-` header a `PreDispatch<Grpc>` entry adds to a reply the framework wrote is dropped,
//! one it rewrites is written back, and each is logged at `warn` once per process per name, as
//! for an interceptor: an entry cannot forge a status onto a 200 reply carrying a message, onto a
//! trailers-only one, a stream's, an error handler's, or onto the health or reflection service's
//! reply. A name an interceptor and an entry both drop is logged once for each, both on the first
//! reply that carries both.
//!
//! In a file of its own: the once-per-process record would otherwise depend on which test of the
//! binary dropped a name first.

mod support;

use std::error::Error;
use std::fmt;

use futures_util::{Stream, StreamExt, stream};
use http::HeaderValue;
use http::uri::PathAndQuery;
use tonic::{Code, Status};
use ulo::{BoxError, ErrorHandler, Interceptor, Module, ModuleDef, ModuleIdentity, Next, injectable, routes};
use ulo_codegen_tests::probe;
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::shapes_client::ShapesClient;
use ulo_codegen_tests::probe::{Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_http::{Middleware, Request, Response};
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

/// What `Mend` claims.
#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Mendable;

impl fmt::Display for Mendable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("mend me")
    }
}

impl Error for Mendable {}

/// Claims a `Mendable` with a trailers-only status through the context.
struct Mend;

impl ErrorHandler<Grpc> for Mend {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        let mendable = err.downcast_ref::<ulo_transport::CallError>().and_then(|error| error.source_as::<Mendable>()).is_some();
        if mendable { Ok(cx.reply_status(Status::failed_precondition("mended"))) } else { Err(err) }
    }
}

/// Unscoped: writes a status, a message and an unlisted `grpc-` name onto every response it passes,
/// and an ordinary header.
struct Forge;

impl Middleware for Forge {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        let mut response = next.run(req).await?;
        let headers = response.headers_mut();
        headers.insert("grpc-status", HeaderValue::from_static("0"));
        headers.insert("grpc-message", HeaderValue::from_static("forged"));
        headers.insert("grpc-forged", HeaderValue::from_static("yes"));
        headers.insert("x-kept", HeaderValue::from_static("kept"));
        Ok(response)
    }
}

/// Scoped to `Upper`: writes `grpc-status: 13` onto the reply it passes.
#[injectable]
struct Fail;

impl Middleware for Fail {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        let mut response = next.run(req).await?;
        response.headers_mut().insert("grpc-status", HeaderValue::from_static("13"));
        Ok(response)
    }
}

/// Writes `grpc-forged`, a name `Forge` writes too, onto the reply it passes.
struct AlsoForge;

impl Interceptor<Grpc> for AlsoForge {
    async fn intercept(&self, _cx: &GrpcCx, next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        let mut reply = next.run().await?;
        reply.headers_mut().insert("grpc-forged", HeaderValue::from_static("by the interceptor"));
        Ok(reply)
    }
}

#[injectable]
struct Known;

#[routes]
#[error_handlers(grpc(value = Mend))]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    #[interceptors(value = AlsoForge)]
    fn upper(&self, text: Message<String>) -> Result<String, Refusal> {
        if text.0 == "fail" {
            return Err(Refusal);
        }
        Ok(text.0.to_uppercase())
    }

    #[ulo_grpc::method(probe::known::Ping)]
    fn ping(&self) -> Result<(), Mendable> {
        Err(Mendable)
    }
}

#[injectable]
struct Shapes;

#[routes]
impl Shapes {
    #[ulo_grpc::method(probe::shapes::Watch)]
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
        m.provide::<Fail>();
        m.meta::<ulo_grpc::PreDispatch>().apply_value(Forge).apply_for::<Fail>(["/codegen.probe.v1.Known/Upper"]);
    }
}

fn header<'a>(metadata: &'a tonic::metadata::MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

/// How many captured warnings name `header` as written by `writer`: `"a pre-dispatch entry"` or
/// `"an interceptor"`.
fn warned_about(capture: &Capture, writer: &str, header: &str) -> usize {
    capture
        .warnings()
        .iter()
        .filter(|line| line.contains(&format!("{writer} wrote a `grpc-` header")) && line.ends_with(&format!("header={header}")))
        .count()
}

#[tokio::test]
async fn a_pre_dispatch_entry_neither_adds_a_grpc_name_nor_rewrites_one_the_framework_wrote_and_each_is_logged_once() {
    use tonic_health::pb::HealthCheckRequest;
    use tonic_health::pb::health_check_response::ServingStatus;
    use tonic_health::pb::health_client::HealthClient;

    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let server = ulo_grpc::Server::new("127.0.0.1:0").file_descriptor_set(probe::FILE_DESCRIPTOR_SET).reflection(true);
    let app = Running::start(Root, server).await;
    let mut client = KnownClient::new(app.channel().await);

    // A 200 reply carrying a message: the scoped entry's `grpc-status: 13` and the unscoped one's
    // names are dropped, so the call ends with the trailers the reply was written with.
    for round in 0..2 {
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
        if round == 0 {
            // Both writers' warnings for `grpc-forged` are logged by the first reply.
            for writer in ["an interceptor", "a pre-dispatch entry"] {
                assert_eq!(warned_about(&capture, writer, "grpc-forged"), 1, "{writer}: {:?}", capture.warnings());
            }
        }
    }

    // A trailers-only reply: the handler's error and the UNIMPLEMENTED of a missing method keep
    // the status the framework wrote against the entries' `grpc-status: 0`.
    let status = support::within("the failing Upper call", client.upper("fail".to_owned())).await;
    let status = status.err().unwrap_or_else(|| panic!("the failing Upper answered OK"));
    assert_eq!((status.code(), status.message()), (Code::InvalidArgument, "refused on purpose"), "{status:?}");
    assert_eq!(header(status.metadata(), "grpc-forged"), None, "{:?}", status.metadata());
    let mut grpc = tonic::client::Grpc::new(app.channel().await);
    support::within("the channel's readiness", grpc.ready()).await.unwrap_or_else(|error| panic!("not ready: {error}"));
    let codec = tonic_prost::ProstCodec::<String, String>::default();
    let missing = PathAndQuery::from_static("/codegen.probe.v1.Known/Missing");
    let status = support::within("a missing method", grpc.unary(tonic::Request::new("ada".to_owned()), missing, codec)).await;
    let status = status.err().unwrap_or_else(|| panic!("a missing method answered OK"));
    assert_eq!(status.code(), Code::Unimplemented, "{status:?}");

    // An error handler's reply built through the context keeps the status it wrote.
    let status = support::within("the Ping call", client.ping(())).await.err().unwrap_or_else(|| panic!("Ping answered OK"));
    assert_eq!((status.code(), status.message()), (Code::FailedPrecondition, "mended"), "{status:?}");
    assert_eq!(header(status.metadata(), "grpc-forged"), None, "{:?}", status.metadata());

    // A stream's headers are screened alike: the entry's `grpc-status` would end it before its
    // first item.
    let mut shapes = ShapesClient::new(app.channel().await);
    let reply = support::within("the Watch call", shapes.watch(Ticks { count: 2, fail: false })).await;
    let reply = reply.unwrap_or_else(|status| panic!("the forged status reached the caller on a stream: {status:?}"));
    for dropped in ["grpc-status", "grpc-message", "grpc-forged"] {
        assert_eq!(header(reply.metadata(), dropped), None, "{dropped} reached the caller: {:?}", reply.metadata());
    }
    let items: Vec<u32> = support::within("the Watch items", reply.into_inner().collect::<Vec<_>>())
        .await
        .into_iter()
        .map(|item| item.unwrap_or_else(|status| panic!("a Watch item failed: {status:?}")).n)
        .collect();
    assert_eq!(items, [1, 2]);

    // So is the reflection service's.
    let listed = list_services(&app).await;
    assert!(listed.iter().any(|name| name == "codegen.probe.v1.Known"), "{listed:?}");

    // The health service's reply is screened alike.
    let mut health = HealthClient::new(app.channel().await);
    let checked = support::within("a health check", health.check(HealthCheckRequest { service: String::new() })).await;
    let checked = checked.unwrap_or_else(|status| panic!("the forged status reached the health caller: {status:?}"));
    assert_eq!(header(checked.metadata(), "grpc-forged"), None, "{:?}", checked.metadata());
    assert_eq!(checked.into_inner().status(), ServingStatus::Serving);
    let unknown = support::within("a health check", health.check(HealthCheckRequest { service: "nowhere.v1.Gone".to_owned() })).await;
    let unknown = unknown.err().unwrap_or_else(|| panic!("an unknown service's check answered OK"));
    assert_eq!(unknown.code(), Code::NotFound, "{unknown:?}");
    drop(health);

    for name in ["grpc-status", "grpc-message"] {
        assert_eq!(warned_about(&capture, "a pre-dispatch entry", name), 1, "{name}: {:?}", capture.warnings());
        assert_eq!(warned_about(&capture, "an interceptor", name), 0, "{name}: {:?}", capture.warnings());
    }
    // The interceptor, screened inside the entries, and the entry both dropped `grpc-forged`: each
    // writer's warning is logged once. Planted break: the record keyed by the name alone.
    assert_eq!(warned_about(&capture, "an interceptor", "grpc-forged"), 1, "{:?}", capture.warnings());
    assert_eq!(warned_about(&capture, "a pre-dispatch entry", "grpc-forged"), 1, "{:?}", capture.warnings());
    app.stop().await;
}

/// The services the reflection service lists, failing on a forged status.
async fn list_services(app: &Running) -> Vec<String> {
    use tonic_reflection::pb::v1::ServerReflectionRequest;
    use tonic_reflection::pb::v1::server_reflection_client::ServerReflectionClient;
    use tonic_reflection::pb::v1::server_reflection_request::MessageRequest;
    use tonic_reflection::pb::v1::server_reflection_response::MessageResponse;

    let listing = ServerReflectionRequest { host: String::new(), message_request: Some(MessageRequest::ListServices(String::new())) };
    let mut client = ServerReflectionClient::new(app.channel().await);
    let reply = support::within("the reflection call", client.server_reflection_info(stream::iter([listing]))).await;
    let reply = reply.unwrap_or_else(|status| panic!("the forged status reached the reflection caller: {status:?}"));
    assert_eq!(header(reply.metadata(), "grpc-forged"), None, "{:?}", reply.metadata());
    let mut replies = reply.into_inner();
    let reply = support::within("a reflection reply", replies.next())
        .await
        .unwrap_or_else(|| panic!("the reflection stream ended with no reply"))
        .unwrap_or_else(|status| panic!("the reflection reply failed: {status:?}"));
    match reply.message_response {
        Some(MessageResponse::ListServicesResponse(listed)) => listed.service.into_iter().map(|service| service.name).collect(),
        other => panic!("unexpected reflection reply {other:?}"),
    }
}
