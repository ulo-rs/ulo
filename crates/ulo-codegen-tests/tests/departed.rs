//! A caller that goes away while its request message is read, resetting the stream with `CANCEL`
//! or dropping the connection, reaches the error handlers as `ExtractError::Cancelled`, kind
//! `Cancelled`, code CANCELLED; a client-streamed request it leaves ends with a CANCELLED item
//! rather than as if complete. A body the caller broke stays `Malformed`, kind `BadRequest`: a
//! reset carrying `NO_ERROR`, which hyper reads as the body's end, `INTERNAL_ERROR` or
//! `PROTOCOL_ERROR`, and a body ending inside a message.
//!
//! The client is raw HTTP/2: HEADERS, a DATA frame holding a message cut two bytes short, then
//! the ending under test.

mod support;

use futures_util::{Stream, StreamExt, stream};
use prost::Message as _;
use prost::bytes::Bytes;
use tonic::Code;
use ulo::{BoxError, Dep, ErrorHandler, Guard, Module, ModuleDef, ModuleIdentity, controller, handlers, injectable};
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{CallError, ErrorKind, Grpc, GrpcCx, Message, Reply, Streaming};
use ulo_transport::ExtractError;

use support::{Record, Running};

/// What the error handler saw: the kind, the code the wire carries for it, and whether the
/// `ExtractError` under it is `Cancelled` or `Malformed`.
#[derive(Clone)]
struct Seen(Record<(ErrorKind, Code, &'static str)>);

/// How each client-streamed request ended, as the handler's own reader saw it.
#[derive(Clone)]
struct Ended(Record<Option<Code>>);

/// One entry per call that reached the pipeline, before its message is read.
#[derive(Clone)]
struct Entered(Record<()>);

#[injectable]
struct Arrived {
    entered: Dep<Entered>,
}

impl Guard<Grpc> for Arrived {
    async fn can_activate(&self, _cx: &GrpcCx) -> Result<bool, BoxError> {
        self.entered.0.push(());
        Ok(true)
    }
}

#[injectable]
struct Count {
    seen: Dep<Seen>,
}

impl ErrorHandler<Grpc> for Count {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        if let Some(call) = err.downcast_ref::<CallError>() {
            let extract = match call.source_as::<ExtractError>() {
                Some(ExtractError::Cancelled { .. }) => "cancelled",
                Some(ExtractError::Malformed { .. }) => "malformed",
                _ => "other",
            };
            self.seen.0.push((call.kind(), ulo_grpc::to_status(call).code(), extract));
        }
        Err(err)
    }
}

#[controller]
struct Known;

#[handlers]
#[guards(Arrived)]
#[error_handlers(Count)]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }
}

#[controller]
struct Trailing {
    ended: Dep<Ended>,
}

#[handlers]
#[guards(Arrived)]
impl Trailing {
    /// Reads the request stream to its end on a task of its own, since the reply is dropped with
    /// the reset stream, and records the code of the error item it ended with.
    #[ulo_grpc::method(probe::trailing::Chat)]
    fn chat(&self, mut requests: Streaming<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> + use<> {
        let ended = self.ended.clone();
        tokio::spawn(async move {
            let mut last = None;
            while let Some(item) = requests.next().await {
                if let Err(status) = item {
                    last = Some(status.code());
                }
            }
            ended.0.push(last);
        });
        stream::pending()
    }
}

struct Root(Seen, Ended, Entered);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.value(self.1.clone());
        m.value(self.2.clone());
        m.provide::<Count>();
        m.provide::<Arrived>();
        m.controller::<Known>();
        m.controller::<Trailing>();
    }
}

enum End {
    Stream,
    Reset(h2::Reason),
    DropConnection,
}

/// One gRPC message frame holding `message`, uncompressed.
fn frame(message: &[u8]) -> Vec<u8> {
    let mut out = vec![0];
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out
}

/// Opens a stream on `path`, sends a message cut two bytes short, waits for the call to pass its
/// guard, the `calls`th to, so its message is being read, and ends the stream as `end`.
async fn send(app: &Running, entered: &Entered, calls: usize, path: &str, end: End) {
    let tcp = tokio::net::TcpStream::connect(app.addr).await.expect("the client connects");
    let (client, connection) = h2::client::handshake(tcp).await.expect("the HTTP/2 handshake");
    let connection = tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut client = client.ready().await.expect("the connection is ready");
    let request = http::Request::post(format!("{}{path}", app.origin()))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(())
        .expect("a request");
    let (response, mut stream) = client.send_request(request, false).expect("the stream opens");
    let mut partial = frame(&"hello".to_owned().encode_to_vec());
    partial.truncate(partial.len() - 2);
    stream.send_data(Bytes::from(partial), matches!(end, End::Stream)).expect("the data is sent");
    entered.0.at_least(calls, "the call's guard").await;
    match end {
        End::Stream => {}
        End::Reset(reason) => stream.send_reset(reason),
        End::DropConnection => {
            connection.abort();
            drop((client, response));
        }
    }
}

/// Planted breaks: `caller_left` answering `false`, which files the `CANCEL` reset as `Missing`
/// and the dropped connection as `Malformed`; and `first_message` not reading the recorded
/// departure, which does the same.
#[tokio::test]
async fn a_caller_leaving_mid_message_is_cancelled_and_a_broken_message_is_a_bad_request() {
    let (seen, entered) = (Seen(Record::new()), Entered(Record::new()));
    let root = Root(seen.clone(), Ended(Record::new()), entered.clone());
    let app = Running::start(root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let upper = "/codegen.probe.v1.Known/Upper";
    let cancelled = (ErrorKind::Cancelled, Code::Cancelled, "cancelled");
    let malformed = (ErrorKind::BadRequest, Code::Internal, "malformed");
    let cases = [
        ("a reset carrying CANCEL", End::Reset(h2::Reason::CANCEL), cancelled),
        ("a dropped connection", End::DropConnection, cancelled),
        ("a reset carrying NO_ERROR", End::Reset(h2::Reason::NO_ERROR), malformed),
        ("a reset carrying INTERNAL_ERROR", End::Reset(h2::Reason::INTERNAL_ERROR), malformed),
        ("a reset carrying PROTOCOL_ERROR", End::Reset(h2::Reason::PROTOCOL_ERROR), malformed),
        ("a body ending inside the message", End::Stream, malformed),
    ];
    for (n, (case, end, expected)) in cases.into_iter().enumerate() {
        send(&app, &entered, n + 1, upper, end).await;
        let entries = seen.0.at_least(n + 1, case).await;
        assert_eq!(entries[n], expected, "{case}");
    }
    let tally = seen.0.snapshot();
    let count = |kind| tally.iter().filter(|(seen, ..)| *seen == kind).count();
    assert_eq!((count(ErrorKind::Cancelled), count(ErrorKind::BadRequest)), (2, 4), "the counter's tally: {tally:?}");
    app.stop().await;
}

/// Planted break: `Streaming::poll_next` not reading the recorded departure, which ends the stream
/// as if complete, `None`, for the reset and the drop alike.
#[tokio::test]
async fn a_client_stream_the_caller_left_ends_cancelled() {
    let (ended, entered) = (Ended(Record::new()), Entered(Record::new()));
    let root = Root(Seen(Record::new()), ended.clone(), entered.clone());
    let app = Running::start(root, ulo_grpc::Server::new("127.0.0.1:0").reflection(false)).await;
    let chat = "/codegen.probe.v1.Trailing/Chat";
    send(&app, &entered, 1, chat, End::Reset(h2::Reason::CANCEL)).await;
    assert_eq!(ended.0.at_least(1, "the reset stream's end").await[0], Some(Code::Cancelled), "a reset carrying CANCEL");
    send(&app, &entered, 2, chat, End::DropConnection).await;
    assert_eq!(ended.0.at_least(2, "the dropped stream's end").await[1], Some(Code::Cancelled), "a dropped connection");
    app.stop().await;
}
