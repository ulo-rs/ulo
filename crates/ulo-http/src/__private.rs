//! Support for the code `ulo-http-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::marker::PhantomData;

use serde::de::DeserializeOwned;

pub use ulo_transport as transport;

use crate::extract::{Host, Path};
use crate::handler::HttpHandler;

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
