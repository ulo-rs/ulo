//! What a proto method carries, and how it reaches the handler's parameters.
//!
//! `#[grpc_methods]` writes the tonic trait impl, whose method signatures name
//! each request type. A macro cannot resolve a name to learn one, so it
//! projects through a marker toni-build wrote beside the trait:
//!
//! ```ignore
//! async fn greet(&self, request: ::tonic::Request<<greeter_toni::Greet as MethodShape>::Arg>)
//!     -> Result<::tonic::Response<GreetReply>, ::tonic::Status>
//! ```
//!
//! The body hands the request to the execution through the same marker, and
//! every parameter of the handler — the message included — is then an
//! ordinary `FromContext<GrpcContext>`.

use std::any::{Any, type_name};
use std::ops::{Deref, DerefMut};

use toni::context::GrpcContext;
use toni::extractors::{FromContext, Inbound};
use toni::grpc_runtime::{RequestCarrier, RequestError};
use toni::{GrpcCode, GrpcStatus};

/// One proto method's request, as its trait declares it.
///
/// Implemented by the marker types toni-build writes, one per method, in a
/// module beside the service's `*_server` module. A hand-written trait gets
/// its markers the same way, from `toni_build::shapes_in_file`.
pub trait MethodShape {
    /// What `tonic::Request<_>` carries: the message, or `tonic::Streaming<Message>`
    /// where the caller streams.
    type Arg: Send + 'static;

    /// Hand the request to the execution, where the handler's parameters take
    /// it — [`message`] or [`stream`], which is the one fact the marker adds
    /// to the type.
    fn install(request: tonic::Request<Self::Arg>, ctx: &GrpcContext);
}

/// Install a request whose caller sent one message.
pub fn message<T: Send + 'static>(request: tonic::Request<T>, ctx: &GrpcContext) {
    ctx.install_request(Box::new(MessageRequest(request)));
}

/// Install a request whose caller streams.
pub fn stream<T: Send + 'static>(request: tonic::Request<tonic::Streaming<T>>, ctx: &GrpcContext) {
    ctx.install_request(Box::new(StreamRequest(request)));
}

struct MessageRequest<T>(tonic::Request<T>);

impl<T: Send + 'static> RequestCarrier for MessageRequest<T> {
    fn take_message(self: Box<Self>) -> Box<dyn Any + Send> {
        Box::new(self.0.into_inner())
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }

    fn carries(&self) -> &'static str {
        type_name::<T>()
    }
}

struct StreamRequest<T>(tonic::Request<tonic::Streaming<T>>);

impl<T: Send + 'static> RequestCarrier for StreamRequest<T> {
    /// The caller's stream fails with tonic's status; a handler reading it
    /// sees toni's, so the conversion happens once here.
    fn take_message(self: Box<Self>) -> Box<dyn Any + Send> {
        let items = toni::futures::StreamExt::map(self.0.into_inner(), |item| {
            item.map_err(|status| {
                GrpcStatus::new(
                    GrpcCode::from_i32(status.code() as i32),
                    status.message().to_string(),
                )
            })
        });
        Box::new(Inbound::new(items))
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }

    fn carries(&self) -> &'static str {
        type_name::<Inbound<T>>()
    }
}

/// The whole request as tonic decoded it: the metadata map with its binary
/// entries, the extensions, the peer — for a handler that wants the wire's own
/// view rather than the context's. `HttpRequest` is the same idea on HTTP.
///
/// Taken from the execution once, like `Payload<T>`, and only on a method
/// whose caller sends one message: a streamed request is read as `Inbound<T>`
/// with `&GrpcContext` beside it.
#[derive(Debug)]
pub struct GrpcRequest<T>(pub tonic::Request<T>);

impl<T> GrpcRequest<T> {
    pub fn into_inner(self) -> tonic::Request<T> {
        self.0
    }
}

impl<T> Deref for GrpcRequest<T> {
    type Target = tonic::Request<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for GrpcRequest<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Send + 'static> FromContext<GrpcContext> for GrpcRequest<T> {
    type Error = RequestError;

    const CONSUMES: bool = true;

    async fn extract(ctx: &GrpcContext) -> Result<Self, Self::Error> {
        let carrier = ctx.take_request()?;
        let carried = carrier.carries();
        carrier
            .into_any()
            .downcast::<MessageRequest<T>>()
            .map(|request| GrpcRequest(request.0))
            .map_err(|_| RequestError::Mismatch {
                asked: type_name::<tonic::Request<T>>(),
                carried,
            })
    }
}
