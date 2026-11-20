//! A service behind enhancers and pre-dispatch entries: guards by type and an interceptor by
//! value on the impl, a guard, an interceptor and an error handler on single methods, a global
//! error handler, and `PreDispatch<Grpc>` entries, one unscoped and one scoped to the service's
//! paths, whose HTTP answers the transport translates into gRPC codes. A guard reads every value
//! of a repeated key and the bytes of a `-bin` key, and the unscoped entry's header reaches every
//! answer below it, an inner entry's failure and an error handler's included.

mod support;

use std::error::Error;
use std::fmt;

use http::{HeaderValue, StatusCode};
use tonic::{Code, Status};
use ulo::{AnyErrorHandler, BoxError, Dep, ErrorHandler, Guard, Interceptor, Module, ModuleDef, ModuleIdentity, Next, controller, handlers, injectable};
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::vault_client::VaultClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_http::{HttpBody, Middleware, Request, Response};
use ulo_transport::{CallError, Classify};

use support::{Record, Running};

const TOKEN: &str = "Bearer open";

#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

impl Error for Refusal {}

/// The error `Claimed` fails with, which the global [`Rescue`] claims.
#[derive(Debug, Classify)]
#[classify(conflict)]
struct Stray;

impl fmt::Display for Stray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("stray")
    }
}

impl Error for Stray {}

/// What [`Witness`] read off each call it admitted: every `x-tag` value, and every `x-sig-bin`
/// value decoded.
#[derive(Clone)]
struct Witnessed(Record<(Vec<String>, Vec<Vec<u8>>)>);

/// Admits every call and records the repeated and binary metadata it carried.
#[injectable]
struct Witness {
    seen: Dep<Witnessed>,
}

impl Guard<Grpc> for Witness {
    async fn can_activate(&self, cx: &GrpcCx) -> Result<bool, BoxError> {
        let map = cx.metadata().map();
        let tags = map.get_all("x-tag").iter().map(|value| value.to_str().unwrap_or("not ASCII").to_owned()).collect();
        let signatures = map.get_all_bin("x-sig-bin").iter().map(|value| value.to_bytes().map_or_else(|_| b"undecodable".to_vec(), |bytes| bytes.to_vec())).collect();
        self.seen.0.push((tags, signatures));
        Ok(true)
    }
}

/// Admits a call whose `authorization` metadata carries the token. Named by type, so bound in the
/// module.
#[injectable]
struct TokenGuard;

impl Guard<Grpc> for TokenGuard {
    async fn can_activate(&self, cx: &GrpcCx) -> Result<bool, BoxError> {
        Ok(cx.metadata().get("authorization") == Some(TOKEN))
    }
}

struct Deny;

impl Guard<Grpc> for Deny {
    async fn can_activate(&self, _cx: &GrpcCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

/// Writes reply header `name: value` on every reply the call answers.
struct Header(&'static str, &'static str);

impl Interceptor<Grpc> for Header {
    async fn intercept(&self, _cx: &GrpcCx, next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        let mut reply = next.run().await?;
        reply.headers_mut().insert(self.0, HeaderValue::from_static(self.1));
        Ok(reply)
    }
}

/// Answers the handler's own `Refusal` with FAILED_PRECONDITION and hands any other error on,
/// a guard's refusal among them.
struct Mend;

impl ErrorHandler<Grpc> for Mend {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        let refused = err.downcast_ref::<ulo_transport::CallError>().and_then(|error| error.source_as::<Refusal>()).is_some();
        if refused { Err(BoxError::from(Status::failed_precondition("mended"))) } else { Err(err) }
    }
}

/// Global: answers a [`Stray`] with `Tick { n: 7 }`, claiming it; hands anything else on.
struct Rescue;

impl ErrorHandler<Grpc> for Rescue {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        let stray = err.downcast_ref::<CallError>().and_then(|error| error.source_as::<Stray>()).is_some();
        if stray { Ok(cx.reply(Tick { n: 7 })?) } else { Err(err) }
    }
}

#[controller]
struct Vault;

#[handlers]
#[guards(grpc = TokenGuard, grpc = Witness)]
#[interceptors(grpc(value = Header("x-served-by", "vault")))]
impl Vault {
    #[ulo_grpc::method(probe::vault::Open)]
    fn open(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }

    #[ulo_grpc::method(probe::vault::Sealed)]
    #[guards(value = Deny)]
    fn sealed(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }

    #[ulo_grpc::method(probe::vault::Fragile)]
    #[interceptors(value = Header("x-stamped", "yes"))]
    #[error_handlers(value = Mend)]
    fn fragile(&self, req: Message<Ticks>) -> Result<Tick, Refusal> {
        if req.0.fail { Err(Refusal) } else { Ok(Tick { n: req.0.count }) }
    }

    #[ulo_grpc::method(probe::vault::Claimed)]
    fn claimed(&self, _req: Message<Ticks>) -> Result<Tick, Stray> {
        Err(Stray)
    }
}

/// A second service, outside the scoped entry's paths.
#[controller]
struct Known;

#[handlers]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }
}

/// Unscoped: marks every response `x-pre: seen`, misses included.
struct Mark;

impl Middleware for Mark {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        let mut response = next.run(req).await?;
        response.headers_mut().insert("x-pre", HeaderValue::from_static("seen"));
        Ok(response)
    }
}

/// Scoped to the vault: answers a request carrying `x-refuse: <status>` with that HTTP status, in
/// place of the call, and fails one carrying `x-fail: unauthorized` with `Unauthorized`.
#[injectable]
struct Refuse;

impl Middleware for Refuse {
    async fn handle(&self, req: Request, next: ulo_http::middleware::Next<'_>) -> Result<Response, BoxError> {
        if req.headers().get("x-fail").is_some_and(|value| value == "unauthorized") {
            return Err(CallError::unauthorized("Bearer").into());
        }
        let status = req.headers().get("x-refuse").and_then(|value| value.to_str().ok()).and_then(|value| value.parse::<u16>().ok());
        match status.and_then(|status| StatusCode::from_u16(status).ok()) {
            Some(status) => {
                let mut response = Response::new(HttpBody::empty());
                *response.status_mut() = status;
                Ok(response)
            }
            None => next.run(req).await,
        }
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<TokenGuard>();
        m.provide::<Witness>();
        m.value(Witnessed(Record::new()));
        m.provide::<Refuse>();
        let rescue: std::sync::Arc<AnyErrorHandler<Grpc>> = std::sync::Arc::new(Rescue);
        m.enhancer::<AnyErrorHandler<Grpc>>().value(rescue);
        m.meta::<ulo_grpc::PreDispatch>().apply_value(Mark).apply_for::<Refuse>(["/codegen.probe.v1.Vault/*"]);
        m.controller::<Vault>();
        m.controller::<Known>();
    }
}

async fn start() -> Running {
    Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0")).await
}

fn call<T>(message: T, metadata: &[(&'static str, &str)]) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    for (key, value) in metadata {
        request.metadata_mut().insert(*key, value.parse().unwrap_or_else(|error| panic!("bad metadata value {value}: {error}")));
    }
    request
}

fn ticks(count: u32, fail: bool) -> Ticks {
    Ticks { count, fail }
}

fn failed<T: fmt::Debug>(reply: Result<tonic::Response<T>, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => panic!("expected the call to fail, got {:?}", reply.into_inner()),
    }
}

fn header<'a>(metadata: &'a tonic::metadata::MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn an_impl_guard_by_type_reads_the_metadata_of_every_method() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let status = failed(client.open(call(ticks(1, false), &[])).await);
    assert_eq!(status.code(), Code::PermissionDenied, "{status:?}");
    let status = failed(client.fragile(call(ticks(1, false), &[])).await);
    assert_eq!(status.code(), Code::PermissionDenied, "{status:?}");

    let reply = client.open(call(ticks(4, false), &[("authorization", TOKEN)])).await.unwrap_or_else(|status| panic!("Open failed: {status:?}"));
    assert_eq!(header(reply.metadata(), "x-served-by"), Some("vault"));
    assert_eq!(header(reply.metadata(), "x-pre"), Some("seen"));
    assert_eq!(reply.into_inner(), Tick { n: 4 });
    app.stop().await;
}

#[tokio::test]
async fn a_method_guard_refuses_its_method_alone() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let status = failed(client.sealed(call(ticks(1, false), &[("authorization", TOKEN)])).await);
    assert_eq!((status.code(), status.message()), (Code::PermissionDenied, "forbidden"), "{status:?}");
    app.stop().await;
}

/// Two `x-tag` entries and two `x-sig-bin` entries, the second's bytes neither ASCII nor UTF-8,
/// reach the guard as written, in the order sent.
#[tokio::test]
async fn a_guard_reads_every_value_of_a_repeated_key_and_the_bytes_of_a_binary_one() {
    use tonic::metadata::{MetadataValue, AsciiMetadataValue};

    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let mut request = call(ticks(3, false), &[("authorization", TOKEN)]);
    let metadata = request.metadata_mut();
    metadata.append("x-tag", AsciiMetadataValue::from_static("first"));
    metadata.append("x-tag", AsciiMetadataValue::from_static("second"));
    metadata.append_bin("x-sig-bin", MetadataValue::from_bytes(b"sig"));
    metadata.append_bin("x-sig-bin", MetadataValue::from_bytes(&[0x00, 0xff, 0x80]));
    let reply = client.open(request).await.unwrap_or_else(|status| panic!("Open failed: {status:?}"));
    assert_eq!(reply.into_inner(), Tick { n: 3 });

    let seen = app.handle.get::<Witnessed>().await.unwrap_or_else(|error| panic!("the record is bound: {error}"));
    let expected = (vec!["first".to_owned(), "second".to_owned()], vec![b"sig".to_vec(), vec![0x00, 0xff, 0x80]]);
    assert_eq!(seen.0.at_least(1, "the guard's record").await, vec![expected]);
    app.stop().await;
}

#[tokio::test]
async fn method_and_impl_interceptors_both_write_the_reply_headers() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let reply = client.fragile(call(ticks(2, false), &[("authorization", TOKEN)])).await.unwrap_or_else(|status| panic!("Fragile failed: {status:?}"));
    assert_eq!(header(reply.metadata(), "x-served-by"), Some("vault"));
    assert_eq!(header(reply.metadata(), "x-stamped"), Some("yes"));
    let reply = client.open(call(ticks(2, false), &[("authorization", TOKEN)])).await.unwrap_or_else(|status| panic!("Open failed: {status:?}"));
    assert_eq!(header(reply.metadata(), "x-stamped"), None, "a method's interceptor ran for another method");
    app.stop().await;
}

#[tokio::test]
async fn a_method_error_handler_reshapes_its_method_s_error() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let status = failed(client.fragile(call(ticks(2, true), &[("authorization", TOKEN)])).await);
    assert_eq!((status.code(), status.message()), (Code::FailedPrecondition, "mended"));
    assert_eq!(header(status.metadata(), "x-pre"), Some("seen"), "the outer entry's header on the reshaped failure");
    app.stop().await;
}

/// The unscoped entry sits outside the error chain: an answer a global error handler claims passes
/// back through it.
#[tokio::test]
async fn an_answer_a_global_error_handler_claims_carries_the_outer_entry_s_header() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let reply = client.claimed(call(ticks(1, false), &[("authorization", TOKEN)])).await.unwrap_or_else(|status| panic!("Claimed failed: {status:?}"));
    assert_eq!(header(reply.metadata(), "x-pre"), Some("seen"));
    assert_eq!(reply.into_inner(), Tick { n: 7 }, "the error handler's answer");
    app.stop().await;
}

/// An inner entry failing is rendered below the outer one, which decorates the rendering.
#[tokio::test]
async fn an_inner_entry_s_failure_carries_the_outer_entry_s_header() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let status = failed(client.open(call(ticks(1, false), &[("authorization", TOKEN), ("x-fail", "unauthorized")])).await);
    assert_eq!(status.code(), Code::Unauthenticated, "{status:?}");
    assert_eq!(header(status.metadata(), "x-pre"), Some("seen"));
    app.stop().await;
}

#[tokio::test]
async fn a_pre_dispatch_entry_s_http_status_is_translated_by_the_specification_s_table() {
    let app = start().await;
    let mut client = VaultClient::new(app.channel().await);
    let table = [
        ("400", Code::Internal),
        ("401", Code::Unauthenticated),
        ("403", Code::PermissionDenied),
        ("404", Code::Unimplemented),
        ("429", Code::Unavailable),
        ("502", Code::Unavailable),
        ("503", Code::Unavailable),
        ("504", Code::Unavailable),
        ("418", Code::Unknown),
    ];
    for (http, code) in table {
        let status = failed(client.open(call(ticks(1, false), &[("authorization", TOKEN), ("x-refuse", http)])).await);
        assert_eq!(status.code(), code, "HTTP {http}: {status:?}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_scoped_pre_dispatch_entry_leaves_another_service_alone() {
    let app = start().await;
    let mut client = KnownClient::new(app.channel().await);
    let reply = client.upper(call("ada".to_owned(), &[("x-refuse", "401")])).await.unwrap_or_else(|status| panic!("Upper failed: {status:?}"));
    assert_eq!(header(reply.metadata(), "x-pre"), Some("seen"));
    assert_eq!(reply.into_inner(), "ADA");
    app.stop().await;
}

#[tokio::test]
async fn an_unscoped_pre_dispatch_entry_sees_a_path_off_the_table() {
    let app = start().await;
    let mut grpc = tonic::client::Grpc::new(app.channel().await);
    support::within("the channel's readiness", grpc.ready()).await.unwrap_or_else(|error| panic!("not ready: {error}"));
    let path = http::uri::PathAndQuery::from_static("/codegen.probe.v1.Vault/Missing");
    let codec = tonic_prost::ProstCodec::<Ticks, Tick>::default();
    let status = failed(grpc.unary(tonic::Request::new(ticks(1, false)), path, codec).await);
    assert_eq!(status.code(), Code::Unimplemented, "{status:?}");
    assert_eq!(header(status.metadata(), "x-pre"), Some("seen"));
    app.stop().await;
}
