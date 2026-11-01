//! A reply's metadata never carries a name beginning `grpc-`, which gRPC over HTTP/2 reserves, nor
//! `te`, `content-type` or `content-length`. The entry is dropped, the rest of the metadata is
//! kept, and the status, message and details the framework writes still arrive.

mod support;

use std::error::Error;
use std::fmt;

use http::{HeaderValue, StatusCode};
use http::uri::PathAndQuery;
use tonic::{Code, Status};
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_codegen_tests::probe;
use ulo_grpc::{Message, Response};
use ulo_http::middleware::Next;
use ulo_http::{HttpBody, Middleware, Request};
use ulo_transport::Classify;

use support::Running;

#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused on purpose")
    }
}

impl Error for Refusal {}

#[injectable]
struct Known;

#[routes]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> Result<Response<String>, Refusal> {
        if text.0 == "fail" {
            return Err(Refusal);
        }
        Ok(Response::new(text.0.to_uppercase())
            // Listed by name before the prefix screen.
            .metadata("grpc-message-type", "listed")
            // Under the prefix only.
            .metadata("grpc-x", "unlisted")
            .metadata("grpc-retry-pushback-ms", "10")
            .metadata("GRPC-UPPER", "unlisted")
            // Not a prefix match.
            .metadata("x-grpc-y", "kept")
            .metadata("grpcish", "kept")
            // HTTP/2's own, which the reply writes.
            .metadata("te", "trailers")
            .metadata("content-length", "999")
            .metadata("content-type", "text/plain")
            .metadata("x-kept", "kept"))
    }
}

/// Answers HTTP 401 with a `grpc-` header and an ordinary one, as a pre-dispatch entry refusing a call.
struct Refuse;

impl Middleware for Refuse {
    async fn handle(&self, _req: Request, _next: Next<'_>) -> Result<ulo_http::Response, BoxError> {
        let mut response = ulo_http::Response::new(HttpBody::empty());
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        response.headers_mut().insert("grpc-x", HeaderValue::from_static("unlisted"));
        response.headers_mut().insert("grpc-encoding", HeaderValue::from_static("listed"));
        response.headers_mut().insert("x-kept", HeaderValue::from_static("kept"));
        response.headers_mut().insert("content-type", HeaderValue::from_static("text/plain"));
        response.headers_mut().insert("te", HeaderValue::from_static("trailers"));
        Ok(response)
    }
}

struct Root(bool);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Known>();
        if self.0 {
            m.meta::<ulo_grpc::PreDispatch>().apply_value(Refuse);
        }
    }
}

async fn dial(app: &Running, text: &str) -> Result<tonic::Response<String>, Status> {
    let mut grpc = tonic::client::Grpc::new(app.channel().await);
    support::within("the channel's readiness", grpc.ready()).await.unwrap_or_else(|error| panic!("not ready: {error}"));
    let codec = tonic_prost::ProstCodec::<String, String>::default();
    let path = PathAndQuery::from_static("/codegen.probe.v1.Known/Upper");
    support::within("the call", grpc.unary(tonic::Request::new(text.to_owned()), path, codec)).await
}

async fn start(refuse: bool) -> Running {
    Running::start(Root(refuse), ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await
}

fn header<'a>(metadata: &'a tonic::metadata::MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn a_handler_s_grpc_prefixed_reply_metadata_is_dropped_listed_or_not_and_the_rest_is_kept() {
    let app = start(false).await;
    let reply = dial(&app, "ada").await.unwrap_or_else(|status| panic!("Upper failed: {status:?}"));
    let metadata = reply.metadata();
    for dropped in ["grpc-message-type", "grpc-x", "grpc-retry-pushback-ms", "grpc-upper", "te", "content-length"] {
        assert_eq!(header(metadata, dropped), None, "{dropped} reached the caller: {metadata:?}");
    }
    let types: Vec<_> = metadata.get_all("content-type").iter().map(|value| value.to_str().unwrap_or("not ASCII")).collect();
    assert_eq!(types, ["application/grpc"], "the reply's own content type alone: {metadata:?}");
    assert_eq!(header(metadata, "x-grpc-y"), Some("kept"), "a name that does not start with grpc- passes");
    assert_eq!(header(metadata, "grpcish"), Some("kept"), "the prefix is `grpc-`, dash included");
    assert_eq!(header(metadata, "x-kept"), Some("kept"));
    assert_eq!(reply.into_inner(), "ADA", "the status trailer still ended the call as OK");
    app.stop().await;
}

#[tokio::test]
async fn an_error_reply_still_carries_the_status_and_message_the_framework_writes() {
    let app = start(false).await;
    let status = dial(&app, "fail").await.err().unwrap_or_else(|| panic!("expected a failure"));
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(status.message(), "refused on purpose");
    app.stop().await;
}

#[tokio::test]
async fn a_pre_dispatch_refusal_keeps_its_ordinary_headers_and_loses_every_grpc_prefixed_one_beside_the_status() {
    let app = start(true).await;
    let status = dial(&app, "ada").await.err().unwrap_or_else(|| panic!("expected a refusal"));
    assert_eq!(status.code(), Code::Unauthenticated);
    assert!(status.message().contains("401"), "message: {}", status.message());
    assert_eq!(header(status.metadata(), "x-kept"), Some("kept"));
    for dropped in ["grpc-x", "grpc-encoding", "te"] {
        assert_eq!(header(status.metadata(), dropped), None, "{dropped} survived: {:?}", status.metadata());
    }
    let types: Vec<_> = status.metadata().get_all("content-type").iter().map(|value| value.to_str().unwrap_or("not ASCII")).collect();
    assert_eq!(types, ["application/grpc"], "the status's own content type alone, not the entry's: {:?}", status.metadata());
    app.stop().await;
}
