//! What `#[sse]` applies to a handler's value.
//!
//! An SSE handler answers with one of three shapes: a stream of [`SseEvent`](crate::http::SseEvent),
//! a stream of `Result<SseEvent, E>`, or a `Result` of either when the setup before the stream can
//! fail. [`into_sse`] is the one call `#[sse]` emits for all three, so the attribute reads no
//! return type to decide what a handler meant.
//!
//! One trait cannot cover a stream and a `Result` of one: coherence has to allow for an upstream
//! `impl Stream for Result<_, _>` and refuses the pair. The marker parameter moves that
//! disjointness from coherence, which reasons about every possible type, to inference, which sees
//! the concrete one a handler returns. `Result` implements no `Stream` today, and a type that did
//! both would make [`into_sse`] ambiguous at the call rather than conflicting at the impl.

#![doc(hidden)]

use std::marker::PhantomData;

use futures::Stream;

use crate::dispatch::{Http, IntoOutput};
use crate::http::{HttpError, Sse, SseItem};

/// Picks the stream impl below.
pub struct ViaStream;

/// Picks the `Result` impl below, carrying the marker its `Ok` type resolved to.
pub struct ViaResult<M>(PhantomData<M>);

/// A value an `#[sse]` handler may answer with, resolved through marker `M`.
#[diagnostic::on_unimplemented(
    message = "an `#[sse]` handler must answer with a stream of events",
    label = "not a stream of `SseEvent`, of `Result<SseEvent, E>`, or a `Result` of either",
    note = "for setup that can fail before streaming starts, return `Result<impl Stream<Item = SseEvent>, E>`"
)]
pub trait IntoSse<M> {
    type Output: IntoOutput<Http>;

    fn into_sse(self) -> Self::Output;
}

impl<S> IntoSse<ViaStream> for S
where
    S: Stream + Send + 'static,
    S::Item: SseItem,
{
    type Output = Sse<S>;

    fn into_sse(self) -> Sse<S> {
        Sse::new(self)
    }
}

impl<V, E, M> IntoSse<ViaResult<M>> for Result<V, E>
where
    V: IntoSse<M>,
    E: Into<HttpError>,
{
    type Output = Result<V::Output, E>;

    fn into_sse(self) -> Self::Output {
        self.map(IntoSse::into_sse)
    }
}

/// The call `#[sse]` emits around a handler's value.
pub fn into_sse<M, T: IntoSse<M>>(value: T) -> T::Output {
    value.into_sse()
}
