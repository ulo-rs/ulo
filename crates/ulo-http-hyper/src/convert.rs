//! The conversion at hyper's edge: a hyper request into a `ulo_http::Request`, the response back.

use hyper::body::Incoming;
use ulo::BoxError;
use ulo_http::{ConnInfo, HttpBody, OnUpgrade, Request, Upgraded};
use ulo_server_hyper::FuturesIo;

/// The 400 with `Connection: close` for an HTTP/1.1 request carrying no `Host` field, an
/// absolute-form target's included (RFC 9112 §3.2), answered before the app sees the request; `None`
/// for any other. A `Host` with an empty value is a field, the form §3.2 gives a target with no
/// authority, and passes. HTTP/1.0 needs no `Host`, and HTTP/2 carries the authority in
/// `:authority`. The check lives here, where a request is known to come off the wire: the service
/// also answers requests an application builds in a test and an embedding host hands over, and a
/// host keeps its own policy for the requests it reads.
pub(crate) fn missing_host(req: &http::Request<Incoming>) -> Option<ulo_http::Response> {
    ulo_http::__private::missing_host(req.version(), req.headers())
}

/// The request as `ulo-http` reads it: parts, body, the connection as the accept loop reported
/// it with the request's own HTTP version, and the upgrade future hyper stores on a request that
/// asks for one (an HTTP/1.1 `Upgrade`), `None` on any other.
pub(crate) fn request(req: http::Request<Incoming>, conn: &ConnInfo) -> Request {
    let (mut head, body) = req.into_parts();
    let upgrade = head.extensions.remove::<hyper::upgrade::OnUpgrade>().map(|pending| {
        OnUpgrade::new(async move {
            let upgraded = pending.await?;
            Ok::<_, BoxError>(Upgraded::from_futures(FuturesIo::new(upgraded)))
        })
    });
    let mut info = conn.clone();
    info.version = head.version;
    Request { head, body: HttpBody::new(body), conn: info, upgrade }
}
