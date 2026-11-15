//! Support for the code `ulo-http-macros` generates, and for `ulo-ws`'s refused handshake. Not
//! part of the public API: names and shapes here change with their users.

use std::marker::PhantomData;

use serde::de::DeserializeOwned;

pub use ulo_transport as transport;

use http::{HeaderMap, StatusCode};
use ulo_transport::CallError;

use crate::extract::{Host, Path};
use crate::handler::HttpHandler;
use crate::render;

/// The status and the headers HTTP answers `error` with: the status `WithHttpStatus` set when it is
/// 400 to 599, otherwise the kind's, and the headers that status calls for, `WWW-Authenticate`
/// with the error's own challenge or `default_challenge` on a 401, `Retry-After` from a
/// `RetryAfter` detail on a 429 or a 503. A WebSocket handshake refused under
/// `refuse = handshake` is answered with them.
pub fn error_head(error: &CallError, default_challenge: &str) -> (StatusCode, HeaderMap) {
    let status = render::status_for(error);
    let mut headers = HeaderMap::new();
    render::status_headers(error, status, default_challenge, &mut headers);
    (status, headers)
}

/// The seal of [`HttpCarried`](crate::HttpCarried): implemented for `Http` here and for
/// `ulo_grpc::Grpc` in `ulo-grpc`, and by no other transport.
pub trait Carried {}

/// `(&&PathProbe::<P>::new()).__ulo_path(handler)`: `handler.reads_path::<T>()` for a `Path<T>`
/// parameter, `handler` unchanged for any other, ranked by autoref at the concrete parameter type,
/// so an alias of `Path` is checked as one. Every probe method here is named `__ulo_*`, so it does
/// not meet a user trait's method of a common name at the call (`ulo::__private`).
pub struct PathProbe<P>(PhantomData<fn() -> P>);

impl<P> PathProbe<P> {
    pub fn new() -> Self {
        PathProbe(PhantomData)
    }
}

impl<P> Default for PathProbe<P> {
    fn default() -> Self {
        PathProbe::new()
    }
}

pub trait ViaPath {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler;
}

impl<T: DeserializeOwned + 'static> ViaPath for &PathProbe<Path<T>> {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler {
        handler.reads_path::<T>()
    }
}

pub trait NotPath {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler;
}

impl<P> NotPath for PathProbe<P> {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler {
        handler
    }
}

/// `(&&HostProbe::<P>::new()).__ulo_host(handler)`: `handler.reads_host::<T>()` for a `Host<T>`
/// or `Option<Host<T>>` parameter, `handler` unchanged for any other, ranked by autoref as
/// `PathProbe` is.
pub struct HostProbe<P>(PhantomData<fn() -> P>);

impl<P> HostProbe<P> {
    pub fn new() -> Self {
        HostProbe(PhantomData)
    }
}

impl<P> Default for HostProbe<P> {
    fn default() -> Self {
        HostProbe::new()
    }
}

pub trait ViaHost {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler;
}

impl<T: 'static> ViaHost for &HostProbe<Host<T>> {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler {
        handler.reads_host::<T>()
    }
}

impl<T: 'static> ViaHost for &HostProbe<Option<Host<T>>> {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler {
        handler.reads_host::<T>()
    }
}

pub trait NotHost {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler;
}

impl<P> NotHost for HostProbe<P> {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler {
        handler
    }
}
