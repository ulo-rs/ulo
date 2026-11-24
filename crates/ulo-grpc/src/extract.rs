//! The gRPC extractors, each consuming the request body, so a handler taking two of them fails the
//! pairwise check naming both (transports DESIGN §6.1). `#[method(Marker)]` checks the message type
//! each names against the marker's `Request`, and `Streaming<T>` against its shape, at compile time.
//!
//! Each decodes through `tonic::Streaming` given the request's encoding, so a message whose
//! compressed flag is set is inflated in it, into a buffer that stops growing at
//! [`MESSAGE_LIMIT`]: a message larger than the limit, on the wire or once inflated, is refused
//! before more than the limit is held.

use std::error::Error;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use http_body::{Frame, SizeHint};
use tonic::codec::{BufferSettings, Decoder};
use tonic::{Code, Status};
use tonic_prost::ProstDecoder;
use ulo::{AppHandle, BoxError, Redacted};
use ulo_http::HttpBody;
use ulo_transport::{ExtractError, FromCall};

use crate::transport::{Grpc, GrpcCx, GrpcMetadata};

/// The largest message a request may carry, tonic's own default, as it arrives and once
/// decompressed: a larger one fails extraction as `ExtractError::TooLarge`, which the wire sends
/// as RESOURCE_EXHAUSTED.
pub(crate) const MESSAGE_LIMIT: usize = 4 * 1024 * 1024;

/// The request message: `Message<pb::GetUserRequest>`.
#[derive(Clone, Debug)]
pub struct Message<T>(pub T);

impl<T> Message<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Message<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        first_message(cx, "message").await.map(Message)
    }
}

/// The whole request: the message with its metadata.
#[derive(Debug)]
pub struct Request<T> {
    pub(crate) message: T,
    pub(crate) metadata: GrpcMetadata,
}

impl<T> Request<T> {
    pub fn message(&self) -> &T {
        &self.message
    }

    pub fn metadata(&self) -> &GrpcMetadata {
        &self.metadata
    }

    pub fn into_inner(self) -> T {
        self.message
    }
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Request<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        let message = first_message(cx, "request").await?;
        Ok(Request { message, metadata: cx.metadata().clone() })
    }
}

/// A client-streamed request's messages, each decoded as `T`; an item that does not decode yields
/// `Err` and ends the stream. A caller that goes away before its last message, resetting the
/// stream with `CANCEL` or dropping the connection, ends it with a CANCELLED status, which
/// `from_status` reads as kind `Cancelled`, so a handler never takes a stream cut short for a
/// complete one.
pub struct Streaming<T> {
    pub(crate) inner: tonic::Streaming<T>,
    left: Departure,
    app: AppHandle,
    ended: bool,
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Streaming<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        let body = cx.take_body().ok_or(ExtractError::missing("stream"))?;
        let (inner, left) = decoding(cx, body, ProstDecoder::new(BufferSettings::default()));
        Ok(Streaming { inner, left, app: cx.app().clone(), ended: false })
    }
}

/// An item past the 4 MiB message limit is RESOURCE_EXHAUSTED whether its length prefix or its
/// inflated size passes it; tonic answers OUT_OF_RANGE for the first. A departed caller's end is
/// CANCELLED, its message the redacted read error, in place of the end or the error tonic gives.
impl<T> Stream for Streaming<T> {
    type Item = Result<T, tonic::Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.ended {
            return Poll::Ready(None);
        }
        let item = match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(message))) => return Poll::Ready(Some(Ok(message))),
            Poll::Pending => return Poll::Pending,
            Poll::Ready(item) => item,
        };
        if let Some(left) = this.left.take(&this.app) {
            this.ended = true;
            return Poll::Ready(Some(Err(Status::cancelled(format!("the caller went away: {left}")))));
        }
        Poll::Ready(item.map(|item| {
            item.map_err(|status| match status.code() {
                Code::OutOfRange => Status::resource_exhausted(status.message()),
                _ => status,
            })
        }))
    }
}

impl FromCall<Grpc> for GrpcMetadata {
    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        Ok(cx.metadata().clone())
    }
}

/// The request's messages decoded by `decoder`, each one flagged compressed inflated in the
/// request's encoding, at most [`MESSAGE_LIMIT`] bytes on the wire and once inflated, and where a
/// read that failed because the caller went away is recorded.
fn decoding<D>(cx: &GrpcCx, body: HttpBody, decoder: D) -> (tonic::Streaming<D::Item>, Departure)
where
    D: Decoder<Error = Status> + Send + 'static,
{
    let left = Departure::default();
    let body = Watched { inner: body, left: left.clone() };
    (tonic::Streaming::new_request(decoder, body, cx.inner.encoding, Some(MESSAGE_LIMIT)), left)
}

/// The read error of a caller that went away, once one is read.
#[derive(Clone, Default)]
struct Departure(Arc<Mutex<Option<BoxError>>>);

impl Departure {
    fn record(&self, error: BoxError) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    /// The recorded error, redacted through `app`, taken once.
    fn take(&self, app: &AppHandle) -> Option<Redacted> {
        let error = self.0.lock().unwrap_or_else(PoisonError::into_inner).take()?;
        Some(app.redact(error))
    }
}

/// The request body as tonic reads it, a read that failed because the caller went away recorded in
/// `left` and handed on as CANCELLED. tonic ends a request stream at a CANCELLED body error as if
/// the body had ended (`tonic::codec::decode`), which reads a unary request as carrying no message
/// and a client-streamed one as complete; the extractors read the recorded error instead.
struct Watched {
    inner: HttpBody,
    left: Departure,
}

impl http_body::Body for Watched {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Err(error))) if caller_left(&*error) => {
                self.left.record(error);
                Poll::Ready(Some(Err(Box::new(Status::cancelled("the caller went away")))))
            }
            polled => polled,
        }
    }

    fn is_end_stream(&self) -> bool {
        http_body::Body::is_end_stream(&self.inner)
    }

    fn size_hint(&self) -> SizeHint {
        http_body::Body::size_hint(&self.inner)
    }
}

/// Whether a request body's read failed because the caller went away, down the error's `source()`
/// chain to 32 levels: an HTTP/2 `RST_STREAM` or `GOAWAY` carrying `CANCEL`, which a client sends
/// to abandon a call, or the connection ending under the stream, an I/O error of kind
/// `UnexpectedEof`, `ConnectionReset`, `ConnectionAborted`, `BrokenPipe` or `NotConnected`, bare or
/// inside h2's error. A reset carrying `NO_ERROR` never arrives as an error: hyper ends the body at
/// it, so it reads as a body ending inside a message, `Malformed`. A reset with any other reason,
/// `PROTOCOL_ERROR`, `INTERNAL_ERROR` or `REFUSED_STREAM` among them, reports a fault in the peer's
/// use of the protocol and stays `Malformed`.
fn caller_left(error: &(dyn Error + 'static)) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, NotConnected, UnexpectedEof};
    let gone = |io: &std::io::Error| matches!(io.kind(), UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe | NotConnected);
    let mut next = Some(error);
    for _ in 0..32 {
        let Some(link) = next else { return false };
        if let Some(h2) = link.downcast_ref::<h2::Error>() {
            return h2.reason() == Some(h2::Reason::CANCEL) || h2.get_io().is_some_and(gone);
        }
        if link.downcast_ref::<std::io::Error>().is_some_and(gone) {
            return true;
        }
        next = link.source();
    }
    false
}

/// A request body that does not decode as the method's messages, the failures tonic's codec gives
/// INTERNAL: a compressed flag on a call whose `grpc-encoding` names no compression, a flag other
/// than 0 and 1, a compressed message that does not inflate, a body ending inside a message, and a
/// message framed whole that prost does not decode. Carried as an `ExtractError::Malformed`'s
/// source, and sent as INTERNAL, the code the compression specification requires for the first
/// and gRPC's status code table gives "error parsing request proto", as a `Streaming` item that
/// does not decode is sent. A body read that tonic fails INTERNAL is carried the same way: a
/// stream the caller reset mid-message with `INTERNAL_ERROR` or `PROTOCOL_ERROR`, whose reply no
/// one reads.
#[derive(Debug)]
pub(crate) struct Undecodable(Status);

impl fmt::Display for Undecodable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.message())
    }
}

impl Error for Undecodable {}

/// The first message of the request body. A body another extractor took, or one ending before
/// any message, is `Missing`; one over [`MESSAGE_LIMIT`], as it arrives or once inflated, is
/// `TooLarge`; one that does not decode as `T`, or breaks the framing, is `Malformed` with an
/// [`Undecodable`] as its source, redacted through the app; one the caller went away from before
/// its message arrived whole is `Cancelled`, the read error, redacted, as its source.
async fn first_message<T>(cx: &GrpcCx, param: &'static str) -> Result<T, ExtractError>
where
    T: prost::Message + Default + Send + 'static,
{
    let body = cx.take_body().ok_or(ExtractError::missing(param))?;
    let (mut messages, left) = decoding(cx, body, ProstDecoder::<T>::new(BufferSettings::default()));
    let read = messages.message().await;
    if !matches!(read, Ok(Some(_)))
        && let Some(source) = left.take(cx.app())
    {
        return Err(ExtractError::cancelled(param, source));
    }
    match read {
        Ok(Some(message)) => Ok(message),
        Ok(None) => Err(ExtractError::missing(param)),
        Err(status) if matches!(status.code(), Code::OutOfRange | Code::ResourceExhausted) => {
            Err(ExtractError::too_large(param, MESSAGE_LIMIT as u64))
        }
        Err(status) if status.code() == Code::Internal => {
            Err(ExtractError::malformed(param, cx.app().redact(Box::new(Undecodable(status)))))
        }
        Err(status) => Err(ExtractError::malformed(param, cx.app().redact(Box::new(status)))),
    }
}
