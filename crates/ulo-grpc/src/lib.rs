//! The gRPC transport (transports DESIGN §6): handlers bound to the method markers `ulo-build`
//! generates, one `:path` dispatcher over tonic's `server::Grpc`, an HTTP/2 [`Server`] over
//! `ulo-server-hyper`, the pre-dispatch stage as [`PreDispatch`], status and details mapping,
//! health, reflection, and client modules.
//!
//! ```ignore
//! # // reason: the handler needs types generated from a .proto by the caller's build.rs
//! // build.rs
//! ulo_build::configure().compile(&["proto/users.proto"], &["proto"])?;
//!
//! // src/pb.rs
//! ulo_grpc::include_proto!("users.v1");
//!
//! #[routes]
//! #[guards(grpc = TokenGuard)]
//! impl UsersGrpc {
//!     #[ulo_grpc::method(pb::user_service::GetUser)]
//!     async fn get_user(&self, req: Message<pb::GetUserRequest>) -> Result<Response<pb::User>, UserError> {
//!         let user = self.users.find(req.0.id).await?;
//!         Ok(Response::new(user.into()).metadata("x-cache", "miss"))
//!     }
//! }
//! ```
//!
//! tonic's generated service trait is never implemented: the attribute checks the handler's
//! signature against the marker's types and shape, and the dispatcher calls it by path. A
//! controller written without the attribute mounts the same handler value, a [`GrpcHandler`]. gRPC
//! serves on its own port; nothing routes an `application/grpc` request arriving on the HTTP
//! server's port to it.

mod client;
mod dispatch;
mod extract;
mod handler;
mod health;
mod method;
mod pre_dispatch;
mod reflection;
mod server;
mod status;
mod trace;
mod transport;

/// The types [`Server`]'s and [`GrpcClientModule`]'s builders take that another `ulo` crate
/// defines, re-exported so an application configuring gRPC imports them from one place, as
/// `ulo_http::config` does for HTTP. Each name is the defining crate's type, not a copy.
pub mod config {
    #[doc(no_inline)]
    pub use ulo::Bound;
    #[doc(no_inline)]
    pub use ulo_net::{Endpoint, EndpointSpec, Tls};
    #[doc(no_inline)]
    pub use ulo_transport::Count;
}

/// The link a [`Detail::Help`] carries, named by [`Details`] and so re-exported. It sits under a
/// module, as in `ulo_rpc`, where the crate root has a `Link` trait of its own.
pub mod details {
    #[doc(no_inline)]
    pub use ulo_transport::Link;
}

#[doc(hidden)]
pub mod __private;

#[doc(hidden)]
pub use ulo_transport::__derive;

pub use client::{ClientTls, GrpcClient, GrpcClientModule, GrpcEndpoint, outgoing};
pub use extract::{Message, Request, Streaming};
pub use handler::GrpcHandler;
pub use health::GrpcHealth;
pub use method::{Method, Shape};
pub use pre_dispatch::PreDispatch;
pub use server::Server;
pub use status::{code_for, code_for_http, to_status};
pub use transport::{Grpc, GrpcCx, GrpcMetadata, IntoGrpcItem, IntoGrpcReply, IntoGrpcStream, PeerAddr, Reply, Response};
pub use http;
pub use ulo_grpc_macros::method;
pub use ulo_transport::{
    CallError, Classify, Detail, Details, ErrorKind, ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Valid, Validate,
};

/// The code `ulo-build` generated for the protobuf package `$package`: its messages, its clients
/// and the method markers, with the encoded file descriptor set as `FILE_DESCRIPTOR_SET` when the
/// build step wrote one: `ulo_grpc::include_proto!("users.v1");`.
#[macro_export]
macro_rules! include_proto {
    ($package:literal) => {
        include!(concat!(env!("OUT_DIR"), "/", $package, ".rs"));
    };
}
