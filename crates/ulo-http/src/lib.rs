//! The HTTP transport (transports DESIGN §3): the [`Http`] marker and its context [`HttpCx`], the
//! router every backend shares, extractors, responses and Server-Sent Events, the pre-dispatch
//! stage ([`PreDispatch`], [`Middleware`], tower layers, [`Cors`]), the WebSocket upgrade hand-off
//! point, the [`Backend`] SPI with [`Server`], [`embed`], which runs the app inside another
//! framework's server, and [`config`], the other `ulo` crates' types the two servers' builders
//! take.
//!
//! ```
//! # use serde::Serialize;
//! # use ulo::{guards, injectable, routes, BoxError, Dep, Guard};
//! # use ulo_http::{CallError, ErrorKind, Http, HttpCx, Json, Path};
//! # #[derive(Serialize)]
//! # struct User;
//! # #[derive(Debug, ulo_http::Classify)]
//! # #[classify(not_found)]
//! # struct UserError;
//! # impl std::fmt::Display for UserError {
//! #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("no user") }
//! # }
//! # impl std::error::Error for UserError {}
//! # #[injectable]
//! # struct Users;
//! # impl Users {
//! #     async fn find(&self, _id: u64) -> Result<User, UserError> { Ok(User) }
//! # }
//! # #[injectable]
//! # struct AuthGuard;
//! # impl Guard<Http> for AuthGuard {
//! #     async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> { Ok(true) }
//! # }
//! # #[injectable]
//! # struct UsersController {
//! #     users: Dep<Users>,
//! # }
//! #[routes]
//! #[guards(http = AuthGuard)]
//! impl UsersController {
//!     #[ulo_http::get("/users/{id}")]
//!     async fn get(&self, Path(id): Path<u64>) -> Result<Json<User>, UserError> {
//!         Ok(Json(self.users.find(id).await?))
//!     }
//! }
//! ```
//!
//! The method attributes build an [`HttpHandler`] for each handler, and a controller written without
//! them builds the same value in its `Controller::mount`.
//!
//! `ulo-http` owns routing, so every backend routes identically; a backend crate converts between
//! its server and the `http` crate's types and maps the drain.

mod backend;
mod body;
mod cors;
mod cx;
mod extract;
mod handler;
mod limits;
mod miss;
mod pre_dispatch;
mod render;
mod request;
mod response;
mod router;
mod routing;
mod server;
mod service;
mod sse;
mod tower_bridge;
mod trace;
mod transport;
mod upgrade;

pub mod config;
/// The link a [`Detail::Help`] carries, named by [`Details`] and so re-exported. It sits under a
/// module, as in `ulo_rpc`, where the crate root has a `Link` trait of its own.
pub mod details {
    #[doc(no_inline)]
    pub use ulo_transport::Link;
}
pub mod embed;
pub mod middleware;

/// The `multer` types a [`Multipart`] read answers with: each part as a [`Field`](multipart::Field)
/// and a failed read as an [`Error`](multipart::Error). They are reached here and not at the
/// crate root, where `Error` would read as the crate's own:
///
/// ```compile_fail
/// use ulo_http::Field;
/// ```
pub mod multipart {
    pub use multer::{Error, Field};
}

/// The pre-dispatch stage as another transport runs it (X23): `ulo-grpc` builds
/// [`Stage<Grpc>`](stage::Stage) from `mounted.module_meta::<PreDispatch<Grpc>>()` in `prepare`,
/// takes each method path's [`ScopedStage`](stage::ScopedStage), and runs both around its
/// dispatcher with a [`StageHost`](stage::StageHost) that renders a failure as a gRPC status.
pub mod stage {
    pub use crate::pre_dispatch::{Rest, ScopedStage, Stage, StageHost};
}

#[doc(hidden)]
pub mod __private;

#[doc(hidden)]
pub use ulo_transport::__derive;

pub use backend::{Backend, BackendLimits, HttpConfig};
pub use body::{Body, HttpBody};
pub use cors::{Cors, CorsError};
pub use cx::{HttpCx, PathParams};
pub use extract::{BodyStream, Form, Header, Host, Json, LastEventId, Multipart, Path, Query};
pub use handler::HttpHandler;
pub use limits::{BodyLimit, KB, MB, Timeout};
pub use middleware::Middleware;
pub use miss::{MethodNotAllowed, NoRoute, NotAcceptable};
pub use pre_dispatch::PreDispatch;
pub use request::{ConnInfo, OnUpgrade, Request, TlsInfo, Upgraded};
pub use response::{Created, NoContent, Response, WithHeaders};
pub use routing::Routing;
pub use server::Server;
pub use service::AppService;
pub use sse::{Event, EventFieldError, EventId, EventName, Sse, SseItem};
pub use tower_bridge::Service;
pub use transport::{ClientAddr, Http, HttpCarried, RequestHead};
pub use upgrade::{UpgradeHandler, Upgrades};
pub use ulo_http_macros::{delete, get, head, options, patch, post, put};

pub use bytes::Bytes;
pub use http::{HeaderMap, HeaderValue, Method, StatusCode};
pub use ulo_transport::{
    CallError, Classify, Detail, Details, ErrorKind, ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Valid, Validate,
};
pub use {headers, http};
