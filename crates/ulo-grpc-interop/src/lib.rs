//! The `grpc.testing.TestService` the gRPC project's interop client calls, served by `ulo-grpc`.
//!
//! The behaviour follows the project's `doc/interop-test-descriptions.md`: payloads are zero
//! bytes of the requested size, `response_status` fails a call with the code and message it names,
//! `x-grpc-test-echo-initial` is echoed on the reply's headers and `x-grpc-test-echo-trailing-bin`
//! on its trailers, on the unary and the full-duplex call. `UnimplementedCall` and the
//! `UnimplementedService` have no handler, which is how the cases `unimplemented_method` and
//! `unimplemented_service` are answered.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use tonic::{Code, Status};
use ulo::{App, AppBuilder, controller, handlers, module};
use ulo_grpc::{CallError, GrpcMetadata, Message, MetadataEntry, Request, Response, Streaming};

pub mod pb {
    ulo_grpc::include_proto!("grpc.testing");
}

use pb::{
    EchoStatus, Payload, PayloadType, SimpleRequest, SimpleResponse, StreamingInputCallRequest, StreamingInputCallResponse,
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
    async fn unary_call(&self, request: Request<SimpleRequest>) -> Result<Response<SimpleResponse>, Status> {
        if let Some(status) = demanded(request.message().response_status.as_ref()) {
            return Err(status);
        }
        let reply = SimpleResponse { payload: Some(payload(request.message().response_size)?), ..Default::default() };
        echoed(Response::new(reply), request.metadata())
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
    async fn streaming_input_call(&self, mut requests: Streaming<StreamingInputCallRequest>) -> Result<StreamingInputCallResponse, Status> {
        let mut total = 0usize;
        while let Some(request) = requests.next().await {
            total += request?.payload.map_or(0, |payload| payload.body.len());
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

#[module(controllers = [TestService])]
pub struct AppModule;

/// The application, before a runtime is chosen.
pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
