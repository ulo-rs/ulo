//! Bridge between a `#[controller]` struct and its optional handler impl.
//!
//! `#[controller]` emits the factory and the `Controller` object on the struct; the object's
//! `dispatch()` calls `Self::__ulo_dispatch(&source)` at the concrete type. A handler-impl macro
//! (`#[routes]`, `#[patterns]`, `#[grpc_methods]`) emits an inherent `__ulo_dispatch` that
//! out-ranks the blanket default below and names the transport. So one struct attribute serves
//! every transport — the struct macro dispatches to the handlers, it doesn't detect them — and a
//! controller with no handler impl is valid and dispatches nothing.
//!
//! The call sits at a concrete-type site (the generated object names the struct); inherent-wins
//! resolution is a property of that site, not available through a generic `T`.

#![doc(hidden)]

use crate::dispatch::{DispatchSource, Targets};
/// Blanket "no dispatch" default, implemented for every type: an empty HTTP route list, which
/// registers nothing. A handler-impl macro shadows this with an inherent `__ulo_dispatch` of the
/// same name, which wins at the call site.
pub trait DispatchBridge {
    fn __ulo_dispatch(_source: &DispatchSource<Self>) -> Targets
    where
        Self: Sized,
    {
        Targets::Http(Vec::new())
    }
}

impl<T: ?Sized> DispatchBridge for T {}
