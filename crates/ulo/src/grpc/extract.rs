//! The client's side of a streaming gRPC call.

use std::any::type_name;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;

use crate::extractors::FromContext;
use crate::grpc::GrpcContext;
use crate::grpc::GrpcStatus;
use crate::grpc::runtime::RequestError;

/// A stream of messages the caller is sending, for a handler serving a
/// client-streaming or bidirectional rpc.
///
/// ```ignore
/// #[grpc_method]
/// async fn greet_all(&self, mut inbound: Inbound<GreetRequest>)
///     -> Result<GreetReply, NoName>
/// {
///     let mut names = Vec::new();
///     while let Some(req) = inbound.next().await {
///         names.push(req?.name);
///     }
///     Ok(GreetReply { message: names.join(", ") })
/// }
/// ```
///
/// An item fails when the caller's own stream does — a broken connection, a
/// message that will not decode. The failure arrives as a [`GrpcStatus`]
/// rather than tonic's, so a handler reading it names nothing from the wire
/// crate; the method's shape does that conversion where it installs the
/// request on the execution.
pub struct Inbound<T> {
    inner: Pin<Box<dyn Stream<Item = Result<T, GrpcStatus>> + Send>>,
}

impl<T> Inbound<T> {
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<T, GrpcStatus>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }
}

impl<T> Stream for Inbound<T> {
    type Item = Result<T, GrpcStatus>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

/// The stream a client-streaming or bidirectional call carries.
///
/// Taken from the execution once — a stream has nothing to hand a second
/// reader, and the macro rejects two takers at compile time. Asking for a
/// stream on a method whose caller sends one message fails the call with
/// `Internal` naming what it does carry.
impl<T: Send + 'static> FromContext<GrpcContext> for Inbound<T> {
    type Error = RequestError;

    const CONSUMES: bool = true;

    async fn extract(ctx: &GrpcContext) -> Result<Self, Self::Error> {
        let carrier = ctx.take_request()?;
        let carried = carrier.carries();
        carrier
            .take_message()
            .downcast::<Inbound<T>>()
            .map(|inbound| *inbound)
            .map_err(|_| RequestError::Mismatch {
                asked: type_name::<Inbound<T>>(),
                carried,
            })
    }
}

impl<T> std::fmt::Debug for Inbound<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Inbound")
    }
}
