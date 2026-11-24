//! The `grpc.testing.TestService` the gRPC project's interop client calls, served by `ulo-grpc`.
//!
//! The behaviour follows the project's `doc/interop-test-descriptions.md`: payloads are zero
//! bytes of the requested size, `response_status` fails a call with the code and message it names,
//! `x-grpc-test-echo-initial` is echoed on the reply's headers and `x-grpc-test-echo-trailing-bin`
//! on its trailers, on the unary and the full-duplex call. `UnimplementedCall` and the
//! `UnimplementedService` have no handler, which is how the cases `unimplemented_method` and
//! `unimplemented_service` are answered.
//!
//! `expect_compressed` is checked against each request message's compressed flag, as the
//! project's C++ server checks it: a message the client says it compressed must carry the flag,
//! and one it says it did not must not, else INVALID_ARGUMENT. The flags are read off the request
//! body as it passes a pre-dispatch entry ([`Flags`]), ulo-grpc handing a handler the decoded
//! message alone.
//!
//! The server compresses every message of a reply when the client's `grpc-accept-encoding` and
//! the server's `send_encodings` agree on gzip. A unary request's `response_compressed: false`
//! sends its reply uncompressed, through `Response::uncompressed`; one reading `true`, or absent,
//! leaves the reply to that agreement. `ResponseParameters::compressed` is not read: no setting
//! sends one message of a reply stream uncompressed apart from the rest.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream};
use http_body::{Body, Frame, SizeHint};
use ulo::{App, AppBuilder, BoxError, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_grpc::tonic::{Code, Status};
use ulo_grpc::{CallError, GrpcCx, GrpcMetadata, Message, MetadataEntry, Request, Response, Streaming};
use ulo_http::middleware::{Middleware, Next};
use ulo_http::HttpBody;

pub mod pb {
    ulo_grpc::include_proto!("grpc.testing");
}

use pb::{
    BoolValue, EchoStatus, Payload, PayloadType, SimpleRequest, SimpleResponse, StreamingInputCallRequest, StreamingInputCallResponse,
    StreamingOutputCallRequest, StreamingOutputCallResponse,
};

/// The request metadata key whose value the reply's headers echo.
const ECHO_INITIAL: &str = "x-grpc-test-echo-initial";

/// The request metadata key whose binary value the reply's trailers echo.
const ECHO_TRAILING: &str = "x-grpc-test-echo-trailing-bin";

/// `response` with the two metadata entries `custom_metadata` sends echoed: the initial one on its
/// headers and the trailing one on its trailers.
fn echoed<T>(response: Response<T>, metadata: &GrpcMetadata) -> Result<Response<T>, Status> {
    let mut response = response;
    if let Some(value) = metadata.get(ECHO_INITIAL) {
        response = response.metadata(ECHO_INITIAL, value);
    }
    if let Some(value) = metadata.map().get_bin(ECHO_TRAILING) {
        let bytes = value.to_bytes().map_err(|_| Status::invalid_argument(format!("`{ECHO_TRAILING}` is not base64")))?;
        let entry = MetadataEntry::binary(ECHO_TRAILING, bytes).map_err(|error| Status::internal(error.to_string()))?;
        response = response.trailer_entry(entry);
    }
    Ok(response)
}

/// The status a `response_status` asks for, `None` for an absent or OK one.
fn demanded(status: Option<&EchoStatus>) -> Option<Status> {
    status.filter(|status| status.code != 0).map(|status| Status::new(Code::from_i32(status.code), status.message.clone()))
}

fn payload(size: i32) -> Result<Payload, Status> {
    let size = usize::try_from(size).map_err(|_| Status::invalid_argument(format!("requested a response with invalid length {size}")))?;
    Ok(Payload { r#type: PayloadType::Compressable.into(), body: vec![0; size] })
}

/// A reply stream's item error converts into a `CallError`; `from_status` carries the status whole.
fn failure(status: Status) -> CallError {
    ulo_grpc::from_status(status)
}

fn interval(microseconds: i32) -> Duration {
    Duration::from_micros(u64::try_from(microseconds).unwrap_or(0))
}

async fn answer(size: i32, wait: Duration) -> Result<StreamingOutputCallResponse, CallError> {
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    payload(size).map(|payload| StreamingOutputCallResponse { payload: Some(payload), ..Default::default() }).map_err(failure)
}

/// The compressed flag of each message of one body, in order, recorded as the message's length
/// prefix passes ([`Tap`]): a handler takes one per request message it decoded, which arrived
/// whole after its prefix, and the compression client reads a reply's.
#[derive(Clone, Default)]
pub struct Flags(Arc<Mutex<VecDeque<bool>>>);

impl Flags {
    fn push(&self, compressed: bool) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push_back(compressed);
    }

    /// The flag of the earliest message not yet taken.
    pub fn next(&self) -> Option<bool> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).pop_front()
    }

    /// Every flag not yet taken, earliest first.
    pub fn take_all(&self) -> Vec<bool> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).drain(..).collect()
    }
}

/// `expect_compressed` against the flag of the message just decoded.
fn check_compressed(cx: &GrpcCx, expect: Option<&BoolValue>) -> Result<(), Status> {
    let expected = expect.is_some_and(|expect| expect.value);
    let flags = cx.ext::<Flags>().map_err(|error| Status::internal(format!("no compressed flags were recorded: {error}")))?;
    match flags.next() {
        Some(compressed) if compressed == expected => Ok(()),
        Some(compressed) => Err(Status::invalid_argument(format!(
            "expect_compressed is {expected}, and the message's compressed flag is {}",
            u8::from(compressed)
        ))),
        None => Err(Status::internal("a message arrived with no compressed flag recorded")),
    }
}

/// The pre-dispatch entry recording [`Flags`] for each call.
struct RecordFlags;

impl Middleware for RecordFlags {
    async fn handle(&self, mut req: ulo_http::Request, next: Next<'_>) -> Result<ulo_http::Response, BoxError> {
        let flags = Flags::default();
        req.head.extensions.insert(flags.clone());
        let body = std::mem::take(&mut req.body);
        req.body = HttpBody::new(Tap::new(body, flags));
        next.run(req).await
    }
}

/// A gRPC body passed through as it is, each message's compressed flag recorded in [`Flags`] as
/// its prefix passes.
pub struct Tap<B> {
    inner: B,
    flags: Flags,
    /// The bytes of the current prefix read so far.
    prefix: Vec<u8>,
    /// The bytes of the current message still to pass.
    skip: usize,
}

impl<B> Tap<B> {
    pub fn new(inner: B, flags: Flags) -> Self {
        Tap { inner, flags, prefix: Vec::with_capacity(5), skip: 0 }
    }

    fn observe(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            if self.skip > 0 {
                let passed = self.skip.min(data.len());
                self.skip -= passed;
                data = &data[passed..];
                continue;
            }
            self.prefix.push(data[0]);
            data = &data[1..];
            if let [flag, a, b, c, d] = self.prefix[..] {
                self.flags.push(flag != 0);
                self.skip = u32::from_be_bytes([a, b, c, d]) as usize;
                self.prefix.clear();
            }
        }
    }
}

impl<B: Body<Data = Bytes> + Unpin> Body for Tap<B> {
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = self.get_mut();
        let frame = ready!(Pin::new(&mut this.inner).poll_frame(cx));
        if let Some(Ok(frame)) = &frame
            && let Some(data) = frame.data_ref()
        {
            this.observe(data);
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// What a full-duplex call has read and not yet answered.
struct Duplex {
    requests: Streaming<StreamingOutputCallRequest>,
    queued: VecDeque<(i32, Duration)>,
    done: bool,
}

impl Duplex {
    /// The next reply, answering each request's parameters as they arrive rather than after the
    /// caller closes its side, since `ping_pong` waits for each reply before sending the next.
    async fn next(mut self) -> Option<(Result<StreamingOutputCallResponse, CallError>, Duplex)> {
        loop {
            if self.done {
                return None;
            }
            if let Some((size, wait)) = self.queued.pop_front() {
                let reply = answer(size, wait).await;
                self.done = reply.is_err();
                return Some((reply, self));
            }
            match self.requests.next().await? {
                Err(status) => {
                    self.done = true;
                    return Some((Err(failure(status)), self));
                }
                Ok(request) => {
                    if let Some(status) = demanded(request.response_status.as_ref()) {
                        self.done = true;
                        return Some((Err(failure(status)), self));
                    }
                    for parameters in request.response_parameters {
                        self.queued.push_back((parameters.size, interval(parameters.interval_us)));
                    }
                }
            }
        }
    }
}

#[controller]
pub struct TestService;

#[handlers]
impl TestService {
    #[ulo_grpc::method(pb::test_service::EmptyCall)]
    async fn empty_call(&self, _request: Message<pb::Empty>) -> pb::Empty {
        pb::Empty {}
    }

    #[ulo_grpc::method(pb::test_service::UnaryCall)]
    async fn unary_call(&self, cx: GrpcCx, request: Request<SimpleRequest>) -> Result<Response<SimpleResponse>, Status> {
        check_compressed(&cx, request.message().expect_compressed.as_ref())?;
        if let Some(status) = demanded(request.message().response_status.as_ref()) {
            return Err(status);
        }
        let reply = SimpleResponse { payload: Some(payload(request.message().response_size)?), ..Default::default() };
        let reply = match request.message().response_compressed {
            Some(BoolValue { value: false }) => Response::new(reply).uncompressed(),
            _ => Response::new(reply),
        };
        echoed(reply, request.metadata())
    }

    #[ulo_grpc::method(pb::test_service::StreamingOutputCall)]
    async fn streaming_output_call(
        &self,
        request: Message<StreamingOutputCallRequest>,
    ) -> impl Stream<Item = Result<StreamingOutputCallResponse, CallError>> {
        let queued: VecDeque<_> =
            request.0.response_parameters.into_iter().map(|parameters| (parameters.size, interval(parameters.interval_us))).collect();
        stream::unfold(queued, |mut queued| async move {
            let (size, wait) = queued.pop_front()?;
            Some((answer(size, wait).await, queued))
        })
    }

    #[ulo_grpc::method(pb::test_service::StreamingInputCall)]
    async fn streaming_input_call(&self, cx: GrpcCx, mut requests: Streaming<StreamingInputCallRequest>) -> Result<StreamingInputCallResponse, Status> {
        let mut total = 0usize;
        while let Some(request) = requests.next().await {
            let request = request?;
            check_compressed(&cx, request.expect_compressed.as_ref())?;
            total += request.payload.map_or(0, |payload| payload.body.len());
        }
        Ok(StreamingInputCallResponse { aggregated_payload_size: i32::try_from(total).unwrap_or(i32::MAX) })
    }

    #[ulo_grpc::method(pb::test_service::FullDuplexCall)]
    async fn full_duplex_call(
        &self,
        metadata: GrpcMetadata,
        requests: Streaming<StreamingOutputCallRequest>,
    ) -> Result<Response<impl Stream<Item = Result<StreamingOutputCallResponse, CallError>>>, Status> {
        let replies = stream::unfold(Duplex { requests, queued: VecDeque::new(), done: false }, Duplex::next);
        echoed(Response::new(replies), &metadata)
    }
}

pub struct AppModule;

impl Module for AppModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<TestService>();
        m.meta::<ulo_grpc::PreDispatch>().apply_value(RecordFlags).supplies::<Flags>().adopt::<Flags>();
    }
}

/// The application, before a runtime is chosen.
pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
