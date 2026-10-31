//! The HTTP transport (transports DESIGN §3): the [`Http`] marker and its context [`HttpCx`], the
//! router every backend shares, extractors, responses and Server-Sent Events, the pre-dispatch
//! stage ([`PreDispatch`], [`Middleware`], tower layers, [`Cors`]), the WebSocket upgrade hand-off
//! point, the [`Backend`] SPI with [`Server`], [`embed`], which runs the app inside another
//! framework's server, and [`config`], the other `ulo` crates' types the two servers' builders
//! take.
//!
//! ```ignore
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

pub use backend::{Backend, BackendLimits, HttpConfig};
pub use body::{Body, HttpBody};
pub use cors::{Cors, CorsError};
pub use cx::{HttpCx, PathParams};
pub use extract::{BodyStream, Form, Header, Host, Json, LastEventId, Multipart, Path, Query};
pub use handler::HttpHandler;
pub use limits::{BodyLimit, KB, MB, Timeout};
pub use middleware::Middleware;
pub use miss::{MethodNotAllowed, NoRoute};
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
