//! A `CallError`'s message and details reach a gRPC caller through the app's redaction: a
//! registered secret's value is sent as `[redacted]` in the status message and in
//! `grpc-status-details-bin`, from a handler's error, from an error `from_status` built, and from a
//! reply stream's failing item, and from a `tonic::Status` a handler returns bare, whose message and
//! text-bearing details are redacted and whose metadata is sent as written. A secret under four
//! characters is not matched, and a message holding no secret is sent byte for byte.

mod support;

use std::collections::{BTreeMap, HashMap};

use futures_util::{Stream, StreamExt, stream};
use tonic::Code;
use tonic_types::StatusExt;
use ulo::{Module, ModuleDef, ModuleIdentity, Secret, controller, handlers};
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::shapes_client::ShapesClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{CallError, Detail, Details, ErrorKind, Message};

use support::{Running, within};

const SECRET: &str = "s3cr3t-token-xyz";
const SHORT: &str = "abc";

#[controller]
struct Leaks;

#[handlers]
impl Leaks {
    /// Fails as its request names: `handler` with the secret in the message and an `ErrorInfo`,
    /// `status` through a `tonic::Status` holding it, `short` with the short secret, anything else
    /// with the text as given.
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> Result<String, CallError> {
        Err(match text.0.as_str() {
            "handler" => {
                let metadata = BTreeMap::from([("key".to_owned(), SECRET.to_owned())]);
                CallError::new(ErrorKind::NotFound, format!("key {SECRET} refused"))
                    .with_details(Details::new().with(Detail::ErrorInfo { reason: "refused".to_owned(), domain: "d".to_owned(), metadata }))
            }
            "status" => ulo_grpc::from_status(tonic::Status::failed_precondition(format!("key {SECRET} refused"))),
            "status details" => {
                let details = tonic_types::ErrorDetails::with_error_info("refused", "d", HashMap::from([("key".to_owned(), SECRET.to_owned())]));
                let mut status = tonic::Status::with_error_details(Code::FailedPrecondition, "refused", details);
                status.metadata_mut().insert("x-key", SECRET.parse().expect("an ascii value"));
                ulo_grpc::from_status(status)
            }
            "short" => CallError::new(ErrorKind::Conflict, format!("key {SHORT} refused")),
            other => CallError::new(ErrorKind::Conflict, other.to_owned()),
        })
    }
}

#[controller]
struct Streams;

#[handlers]
impl Streams {
    #[ulo_grpc::method(probe::shapes::Watch)]
    fn watch(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> + use<> {
        stream::iter(vec![Ok(Tick { n: 1 }), Err(CallError::new(ErrorKind::Unavailable, format!("key {SECRET} refused")))])
    }
}

#[controller]
struct Bare;

#[handlers]
impl Bare {
    /// Fails with a bare `tonic::Status` holding the secret in its message, its metadata and, as
    /// `Ticks::count` says, an `ErrorInfo` (1), a `google.protobuf.Value` (2) or a detail of the
    /// application's own type (3); or (4) in its message alone, beside a detail of the
    /// application's own type holding no secret, [`contact`].
    #[ulo_grpc::method(probe::shapes::Lookup)]
    fn lookup(&self, req: Message<Ticks>) -> Result<Tick, tonic::Status> {
        let message = format!("key {SECRET} refused");
        let mut status = if req.0.count == 1 {
            let details = tonic_types::ErrorDetails::with_error_info("refused", "d", HashMap::from([("key".to_owned(), SECRET.to_owned())]));
            tonic::Status::with_error_details(Code::NotFound, message, details)
        } else if req.0.count == 4 {
            tonic::Status::with_details(Code::NotFound, message, contact())
        } else if req.0.count == 3 {
            let info = tonic_types::pb::ErrorInfo { reason: format!("key {SECRET}"), domain: "d".to_owned(), metadata: HashMap::new() };
            let any = prost_types::Any { type_url: "type.googleapis.com/app.v1.KeyDetail".to_owned(), value: prost::Message::encode_to_vec(&info) };
            let packed = tonic_types::Status { code: Code::NotFound as i32, message: message.clone(), details: vec![any] };
            tonic::Status::with_details(Code::NotFound, message, prost::Message::encode_to_vec(&packed).into())
        } else {
            let json = Details::new().with(Detail::Json(serde_json::json!({ "tried": [SECRET] })));
            ulo_grpc::to_status(&CallError::new(ErrorKind::NotFound, message).with_details(json))
        };
        status.metadata_mut().insert("x-key", SECRET.parse().expect("an ascii value"));
        Err(status)
    }
}

/// A `google.rpc.Status` packing one detail of an application's own type, a URL with no path in
/// field 1 and an address holding `@` in field 2: neither field changes under redaction, while the
/// bytes read as one text would.
fn contact() -> prost::bytes::Bytes {
    let mut value = vec![0x0a, 24];
    value.extend_from_slice(b"https://docs.example.com");
    value.extend_from_slice(&[0x12, 7]);
    value.extend_from_slice(b"me@corp");
    let any = prost_types::Any { type_url: "type.googleapis.com/app.v1.Contact".to_owned(), value };
    let packed = tonic_types::Status { code: Code::NotFound as i32, message: "m".to_owned(), details: vec![any] };
    prost::Message::encode_to_vec(&packed).into()
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.secret(&Secret::new(SECRET));
        m.secret(&Secret::new(SHORT));
        m.controller::<Leaks>();
        m.controller::<Streams>();
        m.controller::<Bare>();
    }
}

async fn failed(client: &mut KnownClient<tonic::transport::Channel>, text: &str) -> tonic::Status {
    match within("the Upper call", client.upper(text.to_owned())).await {
        Err(status) => status,
        Ok(reply) => panic!("Upper({text}) answered {:?}", reply.into_inner()),
    }
}

/// Planted break: `render_as_is` sending the error as recognised, skipping `__into_sent`, which
/// sends the secret in the message and the `ErrorInfo`, and sends the `from_status` error as the
/// status it was built from.
#[tokio::test]
async fn a_registered_secret_in_a_call_error_is_sent_redacted() {
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let mut client = KnownClient::new(app.channel().await);

    let status = failed(&mut client, "handler").await;
    assert_eq!((status.code(), status.message()), (Code::NotFound, "key [redacted] refused"));
    let info = status.get_error_details().error_info().cloned().unwrap_or_else(|| panic!("no ErrorInfo in {status:?}"));
    assert_eq!(info.metadata, HashMap::from([("key".to_owned(), "[redacted]".to_owned())]));
    assert!(!String::from_utf8_lossy(status.details()).contains(SECRET), "the secret is in grpc-status-details-bin");

    let status = failed(&mut client, "status").await;
    assert_eq!((status.code(), status.message()), (Code::FailedPrecondition, "key [redacted] refused"), "an error `from_status` built");

    let status = failed(&mut client, "status details").await;
    assert_eq!((status.code(), status.message()), (Code::FailedPrecondition, "refused"), "a `from_status` error with the secret in a detail");
    assert!(!String::from_utf8_lossy(status.details()).contains(SECRET), "the secret is in a `from_status` error's details");
    assert_eq!(status.metadata().get("x-key").map(|value| value.as_bytes()), Some(SECRET.as_bytes()), "a `from_status` error's metadata");
    app.stop().await;
}

/// A secret under four characters is not matched, and a message with no secret is sent as
/// written. Planted break: no floor on the secret's length.
#[tokio::test]
async fn a_short_secret_and_a_message_without_one_are_sent_as_written() {
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let mut client = KnownClient::new(app.channel().await);
    assert_eq!(failed(&mut client, "short").await.message(), format!("key {SHORT} refused"));
    assert_eq!(failed(&mut client, "the order changed").await.message(), "the order changed");
    app.stop().await;
}

/// Planted break: the reply stream's failing item rendered unredacted.
#[tokio::test]
async fn a_registered_secret_in_a_stream_item_s_error_is_sent_redacted() {
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let mut client = ShapesClient::new(app.channel().await);
    let mut items = within("the Watch call", client.watch(Ticks { count: 1, fail: false })).await.expect("the stream opens").into_inner();
    assert_eq!(within("the first item", items.next()).await.map(|item| item.map(|tick| tick.n).ok()), Some(Some(1)));
    let ended = within("the failing item", items.next()).await.expect("an item").expect_err("the stream fails");
    assert_eq!((ended.code(), ended.message()), (Code::Unavailable, "key [redacted] refused"));
    app.stop().await;
}

/// Planted break: `render_as_is` sending a bare status as it came. The caller sees the secret in
/// the message and in the details, whichever type packs them, and the metadata, which a handler
/// sets deliberately, arrives as written.
#[tokio::test]
async fn a_registered_secret_in_a_bare_status_is_sent_redacted_and_its_metadata_is_not() {
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let mut client = ShapesClient::new(app.channel().await);
    for count in [1, 2, 3, 4] {
        let status = within("the Lookup call", client.lookup(Ticks { count, fail: false })).await.expect_err("the call fails");
        assert_eq!((status.code(), status.message()), (Code::NotFound, "key [redacted] refused"), "count {count}");
        assert!(!String::from_utf8_lossy(status.details()).contains(SECRET), "the secret is in grpc-status-details-bin, count {count}");
        assert_eq!(status.metadata().get("x-key").map(|value| value.as_bytes()), Some(SECRET.as_bytes()), "metadata, count {count}");
        if count == 1 {
            let info = status.get_error_details().error_info().cloned().unwrap_or_else(|| panic!("no ErrorInfo in {status:?}"));
            assert_eq!(info.metadata, HashMap::from([("key".to_owned(), "[redacted]".to_owned())]));
            assert_eq!(info.reason, "refused");
        }
        if count == 4 {
            assert_eq!(status.details(), contact().as_ref(), "a detail no field of which holds a secret is sent as built");
        }
        if count == 3 {
            let packed = <tonic_types::Status as prost::Message>::decode(status.details()).expect("a google.rpc.Status");
            let info = <tonic_types::pb::ErrorInfo as prost::Message>::decode(packed.details[0].value.as_slice()).expect("the detail still decodes");
            assert_eq!(info.reason, "key [redacted]");
        }
    }
    app.stop().await;
}
