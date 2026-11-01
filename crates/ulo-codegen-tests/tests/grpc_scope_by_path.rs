//! A scoped `PreDispatch<Grpc>` entry runs for every call whose path its scope matches, a path no
//! handler answers included: an anonymous caller is refused alike whether or not the method
//! exists, so UNIMPLEMENTED tells only a caller the entry admitted which methods a service lacks.

mod support;

use std::error::Error;
use std::fmt;

use http::HeaderValue;
use http::uri::PathAndQuery;
use tonic::{Code, Status};
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity, injectable, routes};
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

#[injectable]
struct Known;

#[routes]
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
