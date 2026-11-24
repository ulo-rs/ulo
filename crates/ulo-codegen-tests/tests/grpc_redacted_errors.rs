//! A `CallError`'s message and details reach a gRPC caller through the app's redaction: a
//! registered secret's value is sent as `[redacted]` in the status message and in
//! `grpc-status-details-bin`, from a handler's error, from an error `from_status` built, and from a
//! reply stream's failing item. A secret under four characters is not matched, and a message
//! holding no secret is sent byte for byte.

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
