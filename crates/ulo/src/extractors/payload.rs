use std::any::type_name;

use crate::context::GrpcContext;
use crate::extractors::FromContext;
use crate::grpc_runtime::RequestError;

/// The message payload, deserialised.
///
/// One spelling shared by the transports whose message *is* the payload —
/// WebSocket, RPC and gRPC. Each supplies its own `FromContext` impl, since
/// what a frame, a call and a proto method carry differ, and so do the ways
/// they can fail to arrive.
///
/// HTTP carries no impl for this. There the payload is the request body, and
/// `Json<T>`, `Bytes`, `Body<T>` and `BodyStream` name it more precisely than one
/// word could.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payload<T>(pub T);

/// The message a gRPC call carries, already decoded by tonic.
///
/// Taken from the execution once, so a second reader in the same handler is a
/// compile error naming both. `T` is the method's request message; asking for
/// another type, or for a message on a method whose caller streams, fails the
/// call with `Internal` naming what it does carry.
impl<T: Send + 'static> FromContext<GrpcContext> for Payload<T> {
    type Error = RequestError;

    const CONSUMES: bool = true;

    async fn extract(ctx: &GrpcContext) -> Result<Self, Self::Error> {
        let carrier = ctx.take_request()?;
        let carried = carrier.carries();
        carrier
            .take_message()
            .downcast::<T>()
            .map(|message| Payload(*message))
            .map_err(|_| RequestError::Mismatch {
                asked: type_name::<T>(),
                carried,
            })
    }
}
