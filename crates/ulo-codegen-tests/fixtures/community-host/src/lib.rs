//! An HTTP host for `ulo-http`'s embedding, over a bare hyper server: the host owns the listener
//! and the connections, and the app answers through `Handle::service`.
//!
//! Everything here is `ulo-http`'s and `ulo`'s public API: `Embed` and `EmbedLimits` declare the
//! host, `Handle::stopping` is its graceful-shutdown signal, `Handle::host` hands the app the
//! server future, and `ConnInfo` carries the peer address.

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;

use http::{Request, StatusCode, Uri};
use hyper::body::Incoming;
use hyper::service::Service as HyperService;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tower::Service as _;
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};
use ulo_http::embed::{Embed, EmbedLimits, OriginalPath};
use ulo_http::{ConnInfo, HttpBody, OnUpgrade, Response, Upgraded};

/// The host.
pub struct Hyperlite;

impl Embed for Hyperlite {
    const NAME: &'static str = "hyperlite";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
            .without_forward_miss("a service answers every request it is handed, and a bare hyper server has no router to hand one back to")
            .without_tls_info("the host serves plain TCP")
    }

    /// Hyper's `OnUpgrade`, which its server leaves in a request's extensions.
    fn take_upgrade(extensions: &mut http::Extensions) -> Option<OnUpgrade> {
        let pending = extensions.remove::<hyper::upgrade::OnUpgrade>()?;
        Some(OnUpgrade::new(async move {
            let upgraded = pending.await?;
            Ok::<_, BoxError>(Upgraded::from_futures(TokioIo::new(upgraded).compat()))
        }))
    }
}

/// The embedding server for this host.
pub type Embedded = ulo_http::embed::Embedded<Hyperlite>;

/// Its handle.
pub type Handle = ulo_http::embed::Handle<Hyperlite>;

type AppService = ulo_http::embed::Service<Hyperlite>;

/// The app's service for one connection: puts the connection's `ConnInfo` and the path the client
/// sent into the request's extensions, strips the mount prefix, and hands the request to the app.
/// A request outside the prefix is the host's own: an empty 404.
pub struct HostService {
    app: AppService,
    peer: SocketAddr,
    prefix: Option<&'static str>,
}

impl HostService {
    pub fn new(handle: &Handle, peer: SocketAddr) -> Self {
        HostService { app: handle.service(), peer, prefix: None }
    }

    /// Mounts the app under `prefix`, written `/api` and not `/api/`.
    pub fn nested_at(self, prefix: &'static str) -> Self {
        HostService { prefix: Some(prefix), ..self }
    }

    fn strip(&self, uri: &Uri) -> Option<Uri> {
        let Some(prefix) = self.prefix else { return Some(uri.clone()) };
        let rest = match uri.path().strip_prefix(prefix)? {
            "" => "/",
            rest if rest.starts_with('/') => rest,
            _ => return None,
        };
        let mut parts = uri.clone().into_parts();
        parts.path_and_query = match uri.query() {
            Some(query) => format!("{rest}?{query}"),
            None => rest.to_owned(),
        }
        .parse()
        .ok();
        Uri::from_parts(parts).ok()
    }
}

impl HyperService<Request<Incoming>> for HostService {
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn call(&self, mut req: Request<Incoming>) -> Self::Future {
        let Some(stripped) = self.strip(req.uri()) else {
            return Box::pin(async { Ok(not_found()) });
        };
        let conn = ConnInfo::new(req.version()).peer(self.peer);
        req.extensions_mut().insert(conn);
        let original = OriginalPath::from(req.uri());
        req.extensions_mut().insert(original);
        *req.uri_mut() = stripped;
        Box::pin(self.app.clone().call(req))
    }
}

fn not_found() -> Response {
    let mut response = Response::new(HttpBody::empty());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

/// Serves `listener` until `handle.stopping()` resolves, then stops accepting and waits for the
/// connections in flight. Each accepted stream goes through `wrap` before hyper reads it, and each
/// connection is answered by the service `service_for` builds from its peer's address.
pub async fn serve<S, W, F, Svc>(listener: TcpListener, handle: &Handle, wrap: W, service_for: F) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    W: Fn(TcpStream) -> S,
    F: Fn(SocketAddr) -> Svc,
    Svc: HyperService<Request<Incoming>, Response = Response, Error = Infallible> + Send + 'static,
    Svc::Future: Send + 'static,
{
    let graceful = GracefulShutdown::new();
    let builder = auto::Builder::new(TokioExecutor::new());
    let mut stopping = std::pin::pin!(handle.stopping());
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                let connection = builder.serve_connection_with_upgrades(TokioIo::new(wrap(stream)), service_for(peer));
                let connection = graceful.watch(connection.into_owned());
                tokio::spawn(async move {
                    let _ = connection.await;
                });
            }
            () = &mut stopping => break,
        }
    }
    drop(listener);
    graceful.shutdown().await;
    Ok(())
}

/// Installs `server` as the host's server future and awaits the app's `serve(signal)`: the one
/// shutdown trigger is the app's.
pub async fn run(
    app: App<Bound>,
    handle: &Handle,
    server: impl Future<Output = io::Result<()>> + Send + 'static,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError> {
    if let Err(error) = handle.host(server) {
        let _ = app.handle().close(Signal::new("the hyperlite host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
}
