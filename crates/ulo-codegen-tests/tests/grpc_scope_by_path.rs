//! A scoped `PreDispatch<Grpc>` entry runs for every call whose path its scope matches, a path no
//! handler answers included: an anonymous caller is refused alike whether or not the method
//! exists, so UNIMPLEMENTED tells only a caller the entry admitted which methods a service lacks.
//! Scoped entries run before the reflection service and never before the health service.

mod support;

use std::error::Error;
use std::fmt;

use http::HeaderValue;
use http::uri::PathAndQuery;
use tonic::{Code, Status};
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity, controller, handlers, injectable};
use ulo_codegen_tests::probe;
use ulo_grpc::Message;
use ulo_http::{Middleware, Request, Response};
use ulo_transport::{CallError, Classify};

use support::Running;

const KEY: &str = "Bearer open";

#[derive(Debug, Classify)]
#[classify(unauthorized)]
struct NoKey;

impl fmt::Display for NoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no key")
    }
}

impl Error for NoKey {}

/// Refuses a call whose `authorization` metadata is not the key, and marks one it admits
/// `x-auth: checked`.
#[injectable]
struct RequireKey;

impl Middleware for RequireKey {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        if req.headers().get("authorization").and_then(|value| value.to_str().ok()) != Some(KEY) {
            return Err(BoxError::from(CallError::from(NoKey)));
        }
        let mut response = next.run(req).await?;
        response.headers_mut().insert("x-auth", HeaderValue::from_static("checked"));
        Ok(response)
    }
}

/// Unscoped: marks every response `x-pre: seen`.
struct Mark;

impl Middleware for Mark {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        let mut response = next.run(req).await?;
        response.headers_mut().insert("x-pre", HeaderValue::from_static("seen"));
        Ok(response)
    }
}

#[controller]
struct Known;

#[handlers]
impl Known {
    #[ulo_grpc::method(probe::known::Ping)]
    async fn ping(&self) {}

    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }
}

struct Root(fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<RequireKey>();
        m.controller::<Known>();
        (self.0)(m);
    }
}

fn the_service_behind_a_key(m: &mut ModuleDef<'_>) {
    m.meta::<ulo_grpc::PreDispatch>()
        .apply_value(Mark)
        .apply_for::<RequireKey>(["/codegen.probe.v1.Known/*"])
        .exclude(["/codegen.probe.v1.Known/Ping"]);
}

fn every_path_behind_a_key(m: &mut ModuleDef<'_>) {
    m.meta::<ulo_grpc::PreDispatch>().apply_for::<RequireKey>(["/*"]);
}

async fn start(register: fn(&mut ModuleDef<'_>)) -> Running {
    Running::start(Root(register), ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await
}

/// A unary `StringValue` call on `path`, as any client can dial it: the answer, or the status the
/// call failed with.
async fn dial(app: &Running, path: &'static str, key: Option<&str>) -> Result<tonic::Response<String>, Status> {
    let mut grpc = tonic::client::Grpc::new(app.channel().await);
    support::within("the channel's readiness", grpc.ready()).await.unwrap_or_else(|error| panic!("not ready: {error}"));
    let mut request = tonic::Request::new("ada".to_owned());
    if let Some(key) = key {
        request.metadata_mut().insert("authorization", key.parse().unwrap_or_else(|error| panic!("bad key {key}: {error}")));
    }
    let codec = tonic_prost::ProstCodec::<String, String>::default();
    support::within(path, grpc.unary(request, PathAndQuery::from_static(path), codec)).await
}

fn header<'a>(metadata: &'a tonic::metadata::MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

fn failed(path: &str, reply: Result<tonic::Response<String>, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => panic!("{path}: expected the call to fail, got {:?}", reply.into_inner()),
    }
}

#[tokio::test]
async fn a_scoped_entry_refuses_an_anonymous_call_to_a_missing_method_as_it_refuses_one_to_a_method_served() {
    let app = start(the_service_behind_a_key).await;

    // Anonymous: the served method, a method the service lacks, the served method's name in
    // another case, with a trailing slash, with a further segment and behind a dot segment are
    // refused alike.
    for path in [
        "/codegen.probe.v1.Known/Upper",
        "/codegen.probe.v1.Known/Missing",
        "/codegen.probe.v1.Known/upper",
        "/codegen.probe.v1.Known/Upper/",
        "/codegen.probe.v1.Known/Upper/deeper",
        "/codegen.probe.v1.Known/../codegen.probe.v1.Known/Upper",
    ] {
        let status = failed(path, dial(&app, path, None).await);
        assert_eq!((status.code(), status.message()), (Code::Unauthenticated, "no key"), "{path}: {status:?}");
        assert_eq!(header(status.metadata(), "x-pre"), Some("seen"), "{path}: the unscoped entry before it ran");
    }

    // With the key the served method answers, and every other path is UNIMPLEMENTED: none of
    // them reaches `Upper`.
    let reply = dial(&app, "/codegen.probe.v1.Known/Upper", Some(KEY)).await.unwrap_or_else(|status| panic!("Upper failed: {status:?}"));
    assert_eq!(header(reply.metadata(), "x-auth"), Some("checked"));
    assert_eq!(reply.into_inner(), "ADA");
    for path in [
        "/codegen.probe.v1.Known/Missing",
        "/codegen.probe.v1.Known/upper",
        "/codegen.probe.v1.Known/Upper/",
        "/codegen.probe.v1.Known/Upper/deeper",
        "/codegen.probe.v1.Known/../codegen.probe.v1.Known/Upper",
    ] {
        let status = failed(path, dial(&app, path, Some(KEY)).await);
        assert_eq!(status.code(), Code::Unimplemented, "{path}: {status:?}");
        assert_eq!(header(status.metadata(), "x-auth"), Some("checked"), "{path}: the scoped entry admitted it");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_path_outside_the_scope_or_excluded_from_it_is_answered_without_the_entry() {
    let app = start(the_service_behind_a_key).await;
    // The excluded method with a trailing slash, which the exclusion matches too; a service name
    // in another case, another service, and a path reaching the scope only once its dot segment
    // is resolved, which the scope does not match and the dispatcher, comparing paths as sent,
    // routes nowhere.
    for path in [
        "/codegen.probe.v1.Known/Ping/",
        "/codegen.probe.v1.known/Upper",
        "/codegen.probe.v1.Ghost/Upper",
        "/codegen.probe.v1.Ghost/../codegen.probe.v1.Known/Upper",
    ] {
        let status = failed(path, dial(&app, path, None).await);
        assert_eq!(status.code(), Code::Unimplemented, "{path}: {status:?}");
        assert_eq!(header(status.metadata(), "x-auth"), None, "{path}: the scoped entry ran");
    }
    app.stop().await;
}

#[tokio::test]
async fn an_entry_scoped_to_every_path_refuses_an_anonymous_call_to_a_service_the_server_lacks() {
    let app = start(every_path_behind_a_key).await;
    // `/*` needs one segment at least, so `/` is outside it, as on HTTP.
    let status = failed("/", dial(&app, "/", None).await);
    assert_eq!(status.code(), Code::Unimplemented, "/: {status:?}");
    assert_eq!(header(status.metadata(), "x-auth"), None, "/: the scoped entry ran");
    let path = "/codegen.probe.v1.Ghost/Any";
    let status = failed(path, dial(&app, path, None).await);
    assert_eq!((status.code(), status.message()), (Code::Unauthenticated, "no key"), "{status:?}");
    let status = failed(path, dial(&app, path, Some(KEY)).await);
    assert_eq!(status.code(), Code::Unimplemented, "{status:?}");
    assert_eq!(header(status.metadata(), "x-auth"), Some("checked"));
    app.stop().await;
}

/// The services a `grpc.reflection.v1` client lists, with `key` as its `authorization`.
async fn list_services(app: &Running, key: Option<&str>) -> Result<Vec<String>, Status> {
    use tonic_reflection::pb::v1::ServerReflectionRequest;
    use tonic_reflection::pb::v1::server_reflection_client::ServerReflectionClient;
    use tonic_reflection::pb::v1::server_reflection_request::MessageRequest;
    use tonic_reflection::pb::v1::server_reflection_response::MessageResponse;

    let listing = ServerReflectionRequest { host: String::new(), message_request: Some(MessageRequest::ListServices(String::new())) };
    let mut request = tonic::Request::new(futures_util::stream::iter([listing]));
    if let Some(key) = key {
        request.metadata_mut().insert("authorization", key.parse().unwrap_or_else(|error| panic!("bad key {key}: {error}")));
    }
    let mut client = ServerReflectionClient::new(app.channel().await);
    let mut replies = support::within("the reflection call", client.server_reflection_info(request)).await?.into_inner();
    let reply = support::within("a reflection reply", futures_util::StreamExt::next(&mut replies))
        .await
        .unwrap_or_else(|| panic!("the reflection stream ended with no reply"))?;
    match reply.message_response {
        Some(MessageResponse::ListServicesResponse(listed)) => Ok(listed.service.into_iter().map(|service| service.name).collect()),
        other => panic!("unexpected reflection reply {other:?}"),
    }
}

#[tokio::test]
async fn an_entry_scoped_to_every_path_refuses_an_anonymous_reflection_listing_and_lets_health_through() {
    use tonic_health::pb::health_check_response::ServingStatus;
    use tonic_health::pb::health_client::HealthClient;
    use tonic_health::pb::HealthCheckRequest;

    let server = ulo_grpc::Server::new("127.0.0.1:0").file_descriptor_set(probe::FILE_DESCRIPTOR_SET).reflection(true);
    let app = Running::start(Root(every_path_behind_a_key), server).await;

    let status = list_services(&app, None).await.err().unwrap_or_else(|| panic!("an anonymous caller listed the services"));
    assert_eq!((status.code(), status.message()), (Code::Unauthenticated, "no key"), "{status:?}");
    let listed = list_services(&app, Some(KEY)).await.unwrap_or_else(|status| panic!("the listing with the key failed: {status:?}"));
    assert!(listed.iter().any(|service| service == "codegen.probe.v1.Known"), "{listed:?}");

    // Health is for probes that carry no credentials: `Check` and `Watch` answer the anonymous
    // caller.
    let mut health = HealthClient::new(app.channel().await);
    for service in ["", "codegen.probe.v1.Known"] {
        let request = HealthCheckRequest { service: service.to_owned() };
        let checked = support::within("a health check", health.check(request.clone())).await;
        let checked = checked.unwrap_or_else(|status| panic!("Check {service:?} refused: {status:?}"));
        assert_eq!(checked.into_inner().status(), ServingStatus::Serving, "Check {service:?}");
        let watched = support::within("a health watch", health.watch(request)).await;
        let mut updates = watched.unwrap_or_else(|status| panic!("Watch {service:?} refused: {status:?}")).into_inner();
        let first = support::within("the first health update", futures_util::StreamExt::next(&mut updates))
            .await
            .unwrap_or_else(|| panic!("Watch {service:?} ended with no update"))
            .unwrap_or_else(|status| panic!("Watch {service:?} failed: {status:?}"));
        assert_eq!(first.status(), ServingStatus::Serving, "Watch {service:?}");
    }
    drop(health);
    app.stop().await;
}

fn reflection_behind_a_key(m: &mut ModuleDef<'_>) {
    m.meta::<ulo_grpc::PreDispatch>().apply_for::<RequireKey>(["/grpc.reflection.v1.ServerReflection/*"]);
}

#[tokio::test]
async fn an_entry_scoped_to_the_reflection_service_alone_starts_and_refuses_an_anonymous_listing() {
    let server = ulo_grpc::Server::new("127.0.0.1:0").file_descriptor_set(probe::FILE_DESCRIPTOR_SET).reflection(true);
    let app = Running::start(Root(reflection_behind_a_key), server).await;

    let status = list_services(&app, None).await.err().unwrap_or_else(|| panic!("an anonymous caller listed the services"));
    assert_eq!((status.code(), status.message()), (Code::Unauthenticated, "no key"), "{status:?}");
    let listed = list_services(&app, Some(KEY)).await.unwrap_or_else(|status| panic!("the listing with the key failed: {status:?}"));
    assert!(listed.iter().any(|service| service == "codegen.probe.v1.Known"), "{listed:?}");
    // A method outside the scope answers the anonymous caller.
    let reply = dial(&app, "/codegen.probe.v1.Known/Upper", None).await.unwrap_or_else(|status| panic!("Upper refused: {status:?}"));
    assert_eq!(reply.into_inner(), "ADA");
    app.stop().await;
}
