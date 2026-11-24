//! The gRPC transport (transports DESIGN §6): handlers bound to the method markers `ulo-build`
//! generates, one `:path` dispatcher over tonic's `server::Grpc`, an HTTP/2 [`Server`] over
//! `ulo-server-hyper`, the pre-dispatch stage as [`PreDispatch`], status and details mapping,
//! health, reflection, and client modules.
//!
//! The crate is tokio-bound: tonic is tokio-only, and the server accepts through `ulo-listener-tokio`.
//!
//! ```ignore
//! # // reason: the handler needs types generated from a .proto by the caller's build.rs
//! // build.rs
//! ulo_build::configure().compile(&["proto/users.proto"], &["proto"])?;
//!
//! // src/pb.rs
//! ulo_grpc::include_proto!("users.v1");
//!
//! #[controller]
//! pub struct UsersGrpc {
//!     users: Dep<Users>,
//! }
//!
//! #[handlers]
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
//!
//! The code `ulo-build` writes reaches [`prost`], [`prost_types`], [`tonic`] and tonic-prost's codec
//! through this crate's re-exports, so an application's manifest names none of them. Code calling
//! a generated client names tonic's types from here: `ulo_grpc::tonic::Request`,
//! `ulo_grpc::tonic::Status`, `ulo_grpc::tonic::transport::Channel`.
//!
//! Message compression is gzip ([`Encoding`]): the server decompresses a gzip request unless
//! [`Server::accept_encodings`] says otherwise, and compresses a reply in gzip only when
//! [`Server::send_encodings`] lists it and the caller's `grpc-accept-encoding` does too. A reply
//! built from a [`Response`] marked [`uncompressed`](Response::uncompressed) is sent uncompressed
//! whatever the two agree.

mod client;
mod compression;
mod dispatch;
mod extract;
mod handler;
mod health;
mod metadata;
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

/// The JSON a [`Detail::Json`] carries, `serde_json`'s own type, re-exported so code building one
/// needs no `serde_json` dependency of its own.
pub mod json {
    #[doc(no_inline)]
    pub use serde_json::Value;
}

#[doc(hidden)]
pub mod __private;

#[doc(hidden)]
pub use ulo_transport::__derive;

pub use client::{ClientTls, GrpcClient, GrpcClientModule, GrpcEndpoint, outgoing};
pub use compression::Encoding;
pub use extract::{Message, Request, Streaming};
pub use handler::GrpcHandler;
pub use health::GrpcHealth;
/// The status [`GrpcHealth::set`] and [`GrpcHealth::set_named`] write and [`GrpcHealth::status`]
/// reads, tonic-health's own type, re-exported so an application setting one needs no
/// `tonic-health` dependency of its own.
#[doc(no_inline)]
pub use tonic_health::ServingStatus;
pub use metadata::{MetadataEntry, MetadataError};
pub use method::{Method, Shape};
pub use pre_dispatch::PreDispatch;
pub use server::Server;
pub use status::{WithGrpcCode, code_for, code_for_http, from_status, to_status};
pub use transport::{Grpc, GrpcCx, GrpcMetadata, IntoGrpcItem, IntoGrpcReply, IntoGrpcStream, PeerAddr, Reply, Response};
/// What [`GrpcCx::tls`] answers, HTTP's own types: a guard authorising by client certificate names
/// them from here.
#[doc(no_inline)]
pub use ulo_http::{PeerCertificates, TlsInfo};
pub use http;
pub use prost;
pub use prost_types;
pub use tonic;
pub use ulo_grpc_macros::method;
pub use ulo_transport::{
    CallError, Classify, Detail, Details, ErrorBounds, ErrorKind, ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Valid, Validate,
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
