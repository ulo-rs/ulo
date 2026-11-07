//! Trailing metadata in each position a reply carries it, read by a tonic client: after a unary
//! reply's message, in a trailers-only error, at the end of a server stream and of a bidi stream,
//! and with the status of a stream's `Err` item. A `-bin` value arrives as the bytes it was built
//! from, and a name gRPC over HTTP/2 reserves never arrives, whether a `Response` or a handler's
//! `tonic::Status` carried it.
//!
//! A unary method is also called as a server stream, the same bytes on the wire, so the client
//! reports its headers and its trailers apart: tonic's unary call merges the two.

mod support;

use futures_util::{Stream, StreamExt, stream};
use http::uri::PathAndQuery;
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::{Code, Status};
use ulo::{Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_codegen_tests::probe::trailing_client::TrailingClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{CallError, ErrorKind, Message, MetadataEntry, Response, Streaming};

use support::{Running, within};

/// Bytes no ASCII value can carry: NUL, DEL, the high half, a newline and a colon.
const RAW: &[u8] = &[0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff, b'\n', b':'];

/// The headers and trailers every reply here carries, with three entries the builder drops: a
/// reserved name, a connection-specific name h2 refuses in trailers, and an ASCII value under a
/// `-bin` key.
fn with_metadata<T>(response: Response<T>) -> Response<T> {
    response
        .metadata_entry(MetadataEntry::binary("x-head-bin", RAW).expect("a binary key"))
        .metadata("x-head", "h")
        .trailer("x-rows", "3")
        .trailer_entry(MetadataEntry::binary("x-digest-bin", RAW).expect("a binary key"))
        .trailer("grpc-x", "dropped")
        .trailer("connection", "dropped")
        .trailer("x-bad-bin", "dropped")
}

/// A status carrying metadata of its own, a reserved name and a forged details trailer among it.
fn refusal(code: Code) -> Status {
    let mut status = Status::new(code, "refused with metadata");
    let metadata = status.metadata_mut();
    metadata.insert("x-why", MetadataValue::from_static("quota"));
    metadata.insert_bin("x-why-bin", MetadataValue::from_bytes(RAW));
    metadata.insert("grpc-x", MetadataValue::from_static("forged"));
    metadata.insert_bin("grpc-status-details-bin", MetadataValue::from_bytes(b"forged"));
    status
}

/// A `CallError` naming another server's status as its cause, with a kind and message of its own:
/// what `Feed` ends with when asked for no messages and a failure.
fn wrapped() -> CallError {
    CallError::new(ErrorKind::Unavailable, "the upstream failed").with_source(refusal(Code::NotFound))
}

#[controller]
struct Trailing;

#[handlers]
impl Trailing {
    #[ulo_grpc::method(probe::trailing::One)]
    fn one(&self, req: Message<Ticks>) -> Result<Response<Tick>, Status> {
        if req.0.fail {
            return Err(refusal(Code::FailedPrecondition));
        }
        Ok(with_metadata(Response::new(Tick { n: 1 })))
    }

    #[ulo_grpc::method(probe::trailing::Feed)]
    fn feed(&self, req: Message<Ticks>) -> Response<impl Stream<Item = Result<Tick, CallError>> + use<>> {
        let ticks = stream::iter((0..req.0.count).map(|n| Ok(Tick { n })));
        let end = req.0.fail.then(|| Err(if req.0.count == 0 { wrapped() } else { ulo_grpc::from_status(refusal(Code::Aborted)) }));
        with_metadata(Response::new(ticks.chain(stream::iter(end))))
    }

    #[ulo_grpc::method(probe::trailing::Chat)]
    fn chat(&self, requests: Streaming<Ticks>) -> Response<impl Stream<Item = Result<Tick, CallError>> + use<>> {
        with_metadata(Response::new(requests.map(|request| request.map(|ticks| Tick { n: ticks.count }).map_err(ulo_grpc::from_status))))
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Trailing>();
    }
}

async fn start() -> Running {
    Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await
}

fn ascii<'a>(metadata: &'a MetadataMap, key: &str) -> Option<&'a str> {
    metadata.get(key).and_then(|value| value.to_str().ok())
}

fn binary(metadata: &MetadataMap, key: &str) -> Option<Vec<u8>> {
    metadata.get_bin(key).map(|value| value.to_bytes().unwrap_or_else(|error| panic!("{key} is not base64: {error}")).to_vec())
}

/// The trailers the reply set, and neither entry its builder dropped.
fn assert_trailers(trailers: &MetadataMap) {
    assert_eq!(ascii(trailers, "x-rows"), Some("3"), "{trailers:?}");
    assert_eq!(binary(trailers, "x-digest-bin").as_deref(), Some(RAW), "{trailers:?}");
    assert_eq!(trailers.get("grpc-x"), None, "a reserved name reached the trailers: {trailers:?}");
    assert!(!trailers.contains_key("connection"), "a connection-specific name reached the trailers: {trailers:?}");
    assert!(!trailers.contains_key("x-bad-bin"), "an ASCII value under a `-bin` key reached the trailers: {trailers:?}");
}

/// The headers the reply set, and none of its trailers.
fn assert_headers(headers: &MetadataMap) {
    assert_eq!(binary(headers, "x-head-bin").as_deref(), Some(RAW), "{headers:?}");
    assert_eq!(ascii(headers, "x-head"), Some("h"), "{headers:?}");
    assert!(!headers.contains_key("x-rows") && !headers.contains_key("x-digest-bin"), "a trailer was sent as a header: {headers:?}");
}

/// The reserved names a status carried, absent beside its own metadata.
fn assert_status_metadata(status: &Status) {
    let metadata = status.metadata();
    assert_eq!(ascii(metadata, "x-why"), Some("quota"), "{metadata:?}");
    assert_eq!(binary(metadata, "x-why-bin").as_deref(), Some(RAW), "{metadata:?}");
    assert_eq!(metadata.get("grpc-x"), None, "a reserved name in the status's metadata reached the caller: {metadata:?}");
    assert!(status.details().is_empty(), "a forged `grpc-status-details-bin` reached the caller: {:?}", status.details());
}

/// `path` called as a server stream: the headers, the messages and the trailers, apart.
async fn split(app: &Running, path: &'static str, request: Ticks) -> Result<(MetadataMap, Vec<Tick>, MetadataMap), Status> {
    let mut grpc = tonic::client::Grpc::new(app.channel().await);
    within("the channel's readiness", grpc.ready()).await.unwrap_or_else(|error| panic!("not ready: {error}"));
    let codec = tonic_prost::ProstCodec::<Ticks, Tick>::default();
    let response = within("the call", grpc.server_streaming(tonic::Request::new(request), PathAndQuery::from_static(path), codec)).await?;
    let headers = response.metadata().clone();
    let mut messages = response.into_inner();
    let mut ticks = Vec::new();
    while let Some(tick) = within("the next message", messages.message()).await? {
        ticks.push(tick);
    }
    let trailers = within("the trailers", messages.trailers()).await?.unwrap_or_else(|| panic!("the reply ended without trailers"));
    Ok((headers, ticks, trailers))
}

#[tokio::test]
async fn a_unary_reply_carries_its_trailers_after_its_message() {
    let app = start().await;
    let (headers, ticks, trailers) = split(&app, "/codegen.probe.v1.Trailing/One", Ticks { count: 1, fail: false })
        .await
        .unwrap_or_else(|status| panic!("One failed: {status:?}"));
    assert_eq!(ticks, [Tick { n: 1 }]);
    assert_headers(&headers);
    assert_trailers(&trailers);
    app.stop().await;
}

#[tokio::test]
async fn a_unary_error_carries_its_status_s_metadata_in_the_trailers_only_reply() {
    let app = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let status = within("the call", client.one(Ticks { count: 1, fail: true })).await.err().unwrap_or_else(|| panic!("One answered"));
    assert_eq!((status.code(), status.message()), (Code::FailedPrecondition, "refused with metadata"));
    assert_status_metadata(&status);
    app.stop().await;
}

#[tokio::test]
async fn a_server_stream_carries_its_trailers_at_its_end() {
    let app = start().await;
    let (headers, ticks, trailers) = split(&app, "/codegen.probe.v1.Trailing/Feed", Ticks { count: 2, fail: false })
        .await
        .unwrap_or_else(|status| panic!("Feed failed: {status:?}"));
    assert_eq!(ticks, [Tick { n: 0 }, Tick { n: 1 }]);
    assert_headers(&headers);
    assert_trailers(&trailers);
    app.stop().await;
}

#[tokio::test]
async fn a_stream_s_err_item_carries_its_status_s_metadata_beside_the_reply_s_trailers() {
    let app = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let response = within("the call", client.feed(Ticks { count: 1, fail: true })).await.unwrap_or_else(|status| panic!("Feed failed: {status:?}"));
    assert_headers(response.metadata());
    let mut ticks = response.into_inner();
    assert_eq!(within("the first message", ticks.message()).await.ok().flatten(), Some(Tick { n: 0 }));
    let status = within("the failure", ticks.message()).await.err().unwrap_or_else(|| panic!("the stream did not fail"));
    assert_eq!((status.code(), status.message()), (Code::Aborted, "refused with metadata"));
    assert_status_metadata(&status);
    assert_trailers(status.metadata());
    app.stop().await;
}

#[tokio::test]
async fn a_bidi_stream_carries_its_trailers_at_its_end() {
    let app = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let requests = stream::iter([Ticks { count: 7, fail: false }, Ticks { count: 8, fail: false }]);
    let response = within("the call", client.chat(requests)).await.unwrap_or_else(|status| panic!("Chat failed: {status:?}"));
    assert_headers(response.metadata());
    let mut replies = response.into_inner();
    let mut ticks = Vec::new();
    while let Some(tick) = within("the next message", replies.message()).await.unwrap_or_else(|status| panic!("Chat failed: {status:?}")) {
        ticks.push(tick);
    }
    assert_eq!(ticks, [Tick { n: 7 }, Tick { n: 8 }]);
    let trailers = within("the trailers", replies.trailers())
        .await
        .unwrap_or_else(|status| panic!("Chat failed: {status:?}"))
        .unwrap_or_else(|| panic!("the reply ended without trailers"));
    assert_trailers(&trailers);
    app.stop().await;
}

#[tokio::test]
async fn an_err_item_naming_a_status_as_its_cause_renders_by_its_own_kind_and_message() {
    let app = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let response = within("the call", client.feed(Ticks { count: 0, fail: true })).await.unwrap_or_else(|status| panic!("Feed failed: {status:?}"));
    let status = within("the failure", response.into_inner().message()).await.err().unwrap_or_else(|| panic!("the stream did not fail"));
    assert_eq!((status.code(), status.message()), (Code::Unavailable, "the upstream failed"), "{status:?}");
    assert_eq!(ascii(status.metadata(), "x-why"), None, "the cause's metadata reached the caller: {:?}", status.metadata());
    app.stop().await;
}
