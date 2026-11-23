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
use std::task::{Context, Poll};

use futures_core::Stream;
use tonic::codec::{BufferSettings, Decoder};
use tonic::{Code, Status};
use tonic_prost::ProstDecoder;
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
/// `Err` and ends the stream.
pub struct Streaming<T> {
    pub(crate) inner: tonic::Streaming<T>,
}

impl<T: prost::Message + Default + Send + 'static> FromCall<Grpc> for Streaming<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        let body = cx.take_body().ok_or(ExtractError::Missing { param: "stream" })?;
        Ok(Streaming { inner: decoding(cx, body, ProstDecoder::new(BufferSettings::default())) })
    }
}

/// An item past the 4 MiB message limit is RESOURCE_EXHAUSTED whether its length prefix or its
/// inflated size passes it; tonic answers OUT_OF_RANGE for the first.
impl<T> Stream for Streaming<T> {
    type Item = Result<T, tonic::Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx).map(|item| {
            item.map(|item| {
                item.map_err(|status| match status.code() {
                    Code::OutOfRange => Status::resource_exhausted(status.message()),
                    _ => status,
                })
            })
        })
    }
}

impl FromCall<Grpc> for GrpcMetadata {
    async fn from_call(cx: &GrpcCx) -> Result<Self, ExtractError> {
        Ok(cx.metadata().clone())
    }
}

/// The request's messages decoded by `decoder`, each one flagged compressed inflated in the
/// request's encoding, at most [`MESSAGE_LIMIT`] bytes on the wire and once inflated.
fn decoding<D>(cx: &GrpcCx, body: HttpBody, decoder: D) -> tonic::Streaming<D::Item>
where
    D: Decoder<Error = Status> + Send + 'static,
{
    tonic::Streaming::new_request(decoder, body, cx.inner.encoding, Some(MESSAGE_LIMIT))
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
/// [`Undecodable`] as its source, redacted through the app.
async fn first_message<T>(cx: &GrpcCx, param: &'static str) -> Result<T, ExtractError>
where
    T: prost::Message + Default + Send + 'static,
{
    let body = cx.take_body().ok_or(ExtractError::Missing { param })?;
    let mut messages = decoding(cx, body, ProstDecoder::<T>::new(BufferSettings::default()));
    match messages.message().await {
        Ok(Some(message)) => Ok(message),
        Ok(None) => Err(ExtractError::Missing { param }),
        Err(status) if matches!(status.code(), Code::OutOfRange | Code::ResourceExhausted) => {
            Err(ExtractError::TooLarge { param, limit: MESSAGE_LIMIT as u64 })
        }
        Err(status) if status.code() == Code::Internal => {
            Err(ExtractError::Malformed { param, source: cx.app().redact(Box::new(Undecodable(status))) })
        }
        Err(status) => Err(ExtractError::Malformed { param, source: cx.app().redact(Box::new(status)) }),
    }
}
