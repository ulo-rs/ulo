//! gRPC's pre-dispatch stage (transports DESIGN §6.2, X23): `ulo_http::PreDispatch<Grpc>`, scoped
//! by method path, run by the server around its dispatcher through `ulo_http::stage`. A scope is
//! matched against the call's path, so an entry runs for a method no handler answers under its
//! scope too, before UNIMPLEMENTED. A response an entry answers with a non-200 HTTP status is
//! translated with `code_for_http`.

use std::net::SocketAddr;
use std::sync::Arc;

use ulo::{AppHandle, BoxError, BoxFuture, ExecutionRef, MountedHandler};
use ulo_http::stage::StageHost;
use ulo_http::{HttpBody, Request, Response};

use crate::dispatch::{Dispatcher, status_reply};
use crate::status;
use crate::transport::{Grpc, GrpcMetadata};

/// The gRPC pre-dispatch stage's entries one module declares:
///
/// ```ignore
/// # // reason: names tower-http, which this crate does not depend on
/// m.meta::<ulo_grpc::PreDispatch>()
///     .layer(TraceLayer::new_for_grpc())
///     .apply_for::<ApiKeyAuth>(["/users.v1.UserService/*"]);
/// ```
///
/// A scope matches the call's path, whether or not a handler answers it: the entry above refuses
/// an anonymous call to a method `UserService` lacks as it refuses one to a method it has, so
/// UNIMPLEMENTED reaches only a caller the entry admitted.
///
/// A metadata key apart from HTTP's `ulo_http::PreDispatch`, so an entry declared for one never
/// runs on the other.
pub type PreDispatch = ulo_http::PreDispatch<Grpc>;

/// One call's sub-step, as an entry's failure is offered to the error handlers: the global ones in
/// the unscoped sub-step, the matched handler's tiers then the global ones in the scoped one, and
/// the global ones alone for a path no handler answers. The failure is answered as a status in the
/// response's headers.
pub(crate) struct StageCx {
    dispatcher: Arc<Dispatcher>,
    exec: ExecutionRef,
    /// The path and metadata as the sub-step received them, for the context the error handlers
    /// read: the request itself is the failing entry's.
    path: Arc<str>,
    metadata: GrpcMetadata,
    peer: Option<SocketAddr>,
    handler: Option<MountedHandler<Grpc>>,
}

impl StageCx {
    pub(crate) fn new(dispatcher: &Arc<Dispatcher>, exec: ExecutionRef, req: &Request, handler: Option<MountedHandler<Grpc>>) -> Self {
        StageCx {
            dispatcher: Arc::clone(dispatcher),
            exec,
            path: Arc::from(req.path()),
            metadata: GrpcMetadata::from_headers(req.headers()),
            peer: req.conn.peer,
            handler,
        }
    }
}

impl StageHost for StageCx {
    fn app(&self) -> &AppHandle {
        &self.dispatcher.app
    }

    fn exec(&self) -> &ExecutionRef {
        &self.exec
    }

    fn fail(&self, err: BoxError) -> BoxFuture<'_, Response> {
        Box::pin(async move {
            let cx = self.dispatcher.context(
                &self.exec,
                Arc::clone(&self.path),
                self.metadata.clone(),
                self.peer,
                self.handler.clone(),
                None,
            );
            let reply = match ulo::recover(self.handler.as_ref(), &self.exec, &cx, err).await {
                Ok(reply) => reply,
                Err(err) => status_reply(status::render(err, &self.exec)),
            };
            reply.map(HttpBody::new)
        })
    }
}
