//! A `Response` whose `.metadata` or `.trailer` was given a key or value `MetadataEntry::ascii`
//! refuses fails the reply, as an HTTP reply whose header does not parse does: the caller receives
//! INTERNAL with the message withheld and no message, and the error handlers see a `CallError` of
//! kind `Internal` whose `IntoReplyError` holds the first refusal, naming the key and the reason.
//! A stream's `Err` item built by `from_status` from an `OK` status is sent as INTERNAL, its
//! message saying an OK status was used as an error, and so is an `OK` status a handler returns
//! bare as its error.

mod support;

use std::error::Error as _;

use futures_util::{Stream, stream};
use tonic::{Code, Status};
use ulo::{BoxError, Dep, ErrorHandler, Module, ModuleDef, ModuleIdentity, controller, handlers, injectable};
use ulo_codegen_tests::probe::trailing_client::TrailingClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{CallError, ErrorKind, Grpc, GrpcCx, IntoReplyError, Message, MetadataError, Reply, Response};

use support::{Record, Running, within};

/// What `One` builds for each `count`: the refused entries, then two refusals on one reply, then
/// an entry nothing refuses.
fn one_reply(count: u32) -> Response<Tick> {
    let reply = Response::new(Tick { n: 1 });
    match count {
        0 => reply.metadata("grpc-x", "v"),
        1 => reply.trailer("connection", "v"),
        2 => reply.trailer("x-bad-bin", "v"),
        3 => reply.metadata("x-a", "line\nbreak"),
        4 => reply.metadata("x-fine", "1").metadata("te", "trailers").trailer("grpc-y", "v"),
        _ => reply.metadata("x-fine", "1"),
    }
}

/// The refusal `one_reply(count)` fails with.
fn refusal(count: u32) -> MetadataError {
    let key = |key: &str| key.to_owned();
    match count {
        0 => MetadataError::Reserved { key: key("grpc-x") },
        1 => MetadataError::Reserved { key: key("connection") },
        2 => MetadataError::AsciiUnderBinaryKey { key: key("x-bad-bin") },
        3 => MetadataError::InvalidAscii { key: key("x-a") },
        4 => MetadataError::Reserved { key: key("te") },
        _ => unreachable!("count {count} builds no refused entry"),
    }
}

/// What the error handler saw of each error it was offered: the `CallError`'s kind, the refusal
/// its `IntoReplyError` holds, and that error's text.
#[derive(Clone)]
struct Seen(Record<(ErrorKind, Option<MetadataError>, String)>);

/// Records the error and hands it on unchanged.
#[injectable]
struct Observe {
    seen: Dep<Seen>,
}

impl ErrorHandler<Grpc> for Observe {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        let call = err.downcast_ref::<CallError>().unwrap_or_else(|| panic!("not a `CallError`: {err:?}"));
        let encoding = call.source_as::<IntoReplyError>();
        let refused = encoding.and_then(|error| error.source()).and_then(|source| source.downcast_ref::<MetadataError>()).cloned();
        let text = encoding.map(ToString::to_string).unwrap_or_default();
        self.seen.0.push((call.kind(), refused, text));
        Err(err)
    }
}

#[controller]
struct Refusing;

#[handlers]
impl Refusing {
    #[ulo_grpc::method(probe::trailing::One)]
    #[error_handlers(Observe)]
    fn one(&self, req: Message<Ticks>) -> Response<Tick> {
        one_reply(req.0.count)
    }

    /// `count` 0: two ticks under a refused trailer. `count` 1: one tick, then an `Err` item
    /// built from an `OK` status.
    #[ulo_grpc::method(probe::trailing::Feed)]
    fn feed(&self, req: Message<Ticks>) -> Response<impl Stream<Item = Result<Tick, CallError>> + use<>> {
        let items = match req.0.count {
            0 => vec![Ok(Tick { n: 0 }), Ok(Tick { n: 1 })],
            _ => vec![Ok(Tick { n: 0 }), Err(ulo_grpc::from_status(Status::ok("done")))],
        };
        let reply = Response::new(stream::iter(items));
        if req.0.count == 0 { reply.trailer("grpc-x", "v") } else { reply }
    }

    #[ulo_grpc::method(probe::trailing::Chat)]
    fn chat(&self) -> Result<impl Stream<Item = Result<Tick, CallError>> + use<>, Status> {
        Err::<stream::Empty<Result<Tick, CallError>>, _>(Status::ok("done"))
    }
}

struct Root(Seen);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.provide::<Observe>();
        m.controller::<Refusing>();
    }
}

async fn start() -> (Running, Seen) {
    let seen = Seen(Record::new());
    let app = Running::start(Root(seen.clone()), ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    (app, seen)
}

#[tokio::test]
async fn a_refused_entry_fails_the_reply_internal_with_the_message_withheld() {
    let (app, _) = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    for count in 0..=4 {
        let reply = within("the call", client.one(Ticks { count, fail: false })).await;
        let status = reply.err().unwrap_or_else(|| panic!("count {count}: the reply was sent"));
        assert_eq!((status.code(), status.message()), (Code::Internal, "internal error"), "count {count}");
    }
    let fine = within("the call", client.one(Ticks { count: 5, fail: false })).await.unwrap_or_else(|status| panic!("count 5 failed: {status:?}"));
    assert_eq!(fine.metadata().get("x-fine").and_then(|value| value.to_str().ok()), Some("1"));
    app.stop().await;
}

#[tokio::test]
async fn the_error_handlers_see_the_first_refusal_naming_the_key_and_the_reason() {
    let (app, seen) = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    for count in 0..=4 {
        let _ = within("the call", client.one(Ticks { count, fail: false })).await;
    }
    let seen = seen.0.at_least(5, "the refusals").await;
    for (count, (kind, refused, text)) in (0..).zip(seen) {
        let expected = refusal(count);
        assert_eq!(kind, ErrorKind::Internal, "count {count}");
        assert_eq!(refused.as_ref(), Some(&expected), "count {count}");
        let key = match &expected {
            MetadataError::Reserved { key } | MetadataError::AsciiUnderBinaryKey { key } | MetadataError::InvalidAscii { key } => key,
            other => unreachable!("{other:?}"),
        };
        assert!(text.contains(key.as_str()) && text.contains(&expected.to_string()), "count {count}: {text}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_refused_trailer_on_a_stream_fails_the_reply_before_any_message() {
    let (app, _) = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let status = match within("the call", client.feed(Ticks { count: 0, fail: false })).await {
        Err(status) => status,
        Ok(response) => {
            let mut ticks = response.into_inner();
            let first = within("the first message", ticks.message()).await;
            first.err().unwrap_or_else(|| panic!("a message was sent under a refused trailer"))
        }
    };
    assert_eq!((status.code(), status.message()), (Code::Internal, "internal error"));
    app.stop().await;
}

#[tokio::test]
async fn an_ok_status_ending_a_stream_is_sent_as_internal() {
    let (app, _) = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let response = within("the call", client.feed(Ticks { count: 1, fail: false })).await.unwrap_or_else(|status| panic!("Feed failed: {status:?}"));
    let mut ticks = response.into_inner();
    assert_eq!(within("the first message", ticks.message()).await.ok().flatten(), Some(Tick { n: 0 }));
    let status = within("the end", ticks.message()).await.err().unwrap_or_else(|| panic!("the stream ended OK"));
    assert_eq!((status.code(), status.message()), (Code::Internal, "an OK status was used as an error"));
    app.stop().await;
}

#[tokio::test]
async fn an_ok_status_a_handler_returns_bare_is_sent_as_internal() {
    let (app, _) = start().await;
    let mut client = TrailingClient::new(app.channel().await);
    let status = match within("the call", client.chat(stream::iter([Ticks { count: 0, fail: false }]))).await {
        Err(status) => status,
        Ok(response) => {
            let mut ticks = response.into_inner();
            within("the end", ticks.message()).await.err().unwrap_or_else(|| panic!("the call ended OK"))
        }
    };
    assert_eq!((status.code(), status.message()), (Code::Internal, "an OK status was used as an error"));
    app.stop().await;
}
