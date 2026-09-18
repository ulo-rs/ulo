//! What a reply carries beside its message, as the pipeline sees it.
//!
//! `ulo` names no tonic type, so [`ReplyEnvelope`] is core's and the carrier around
//! `tonic::Response<T>` is here — the shape [`RequestCarrier`](ulo::grpc::RequestCarrier) takes on
//! the request side. The `#[grpc_methods]` wrapper wraps a reply on the way into the pipeline and
//! downcasts it on the way out.

use std::any::{Any, type_name};

use tonic::metadata::{Ascii, MetadataKey, MetadataValue};
use ulo::grpc::{GrpcReply, InvalidHeader, ReplyEnvelope};

/// The answer an enhancer hands back when it replies for a method itself.
///
/// A cache hit, or an error handler recovering a call, builds the `tonic::Response<T>` the method
/// answers with and passes it here:
///
/// ```ignore
/// Ok(ulo_grpc::reply(tonic::Response::new(CreateOrderResponse { id: 7, .. })))
/// ```
pub fn reply<T: Send + 'static>(response: tonic::Response<T>) -> GrpcReply {
    GrpcReply::new(Envelope(response))
}

/// A `tonic::Response<T>` behind the trait core declares.
///
/// The newtype is what makes the impl legal: `ReplyEnvelope` belongs to `ulo` and
/// `tonic::Response` to tonic, so neither is local here and the orphan rule refuses the direct
/// impl. `RequestCarrier`'s two carriers are newtypes for the same reason.
pub struct Envelope<T>(pub tonic::Response<T>);

impl<T: Send + 'static> ReplyEnvelope for Envelope<T> {
    /// A binary (`-bin`) header answers `None`: its value is not ASCII, and this seam reads
    /// strings. The request side drops binary metadata at the same boundary.
    fn header(&self, key: &str) -> Option<&str> {
        self.0.metadata().get(key).and_then(|v| v.to_str().ok())
    }

    fn set_header(&mut self, key: &str, value: &str) -> Result<(), InvalidHeader> {
        // Parsed rather than passed through: `MetadataMap::insert` takes `&str` as a key and
        // panics on one the wire cannot carry, and an enhancer's header is not worth a panic.
        let parsed_key =
            MetadataKey::<Ascii>::from_bytes(key.as_bytes()).map_err(|_| InvalidHeader {
                key: key.to_string(),
            })?;
        let parsed_value = MetadataValue::<Ascii>::try_from(value).map_err(|_| InvalidHeader {
            key: key.to_string(),
        })?;
        self.0.metadata_mut().insert(parsed_key, parsed_value);
        Ok(())
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        Box::new(self.0)
    }

    fn as_any(&self) -> &(dyn Any + Send) {
        &self.0
    }

    fn as_any_mut(&mut self) -> &mut (dyn Any + Send) {
        &mut self.0
    }

    /// The `tonic::Response<T>` inside, not the `Reply<T>` around it — the name a downcast asks
    /// for and the one a mismatch diagnostic prints.
    fn carries(&self) -> &'static str {
        type_name::<tonic::Response<T>>()
    }
}
