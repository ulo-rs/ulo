//! How many items an answer carries, over whatever the transport carries them as.

use futures::stream::BoxStream;

/// Nothing, one item, or a stream of them.
///
/// The same three states on every transport that has them, over an item type each names for
/// itself: `RpcData` on RPC, `WsMessage` on WebSocket. What varies beside the count — a status and
/// headers on HTTP, metadata on gRPC — is the answer's envelope and sits outside this (ADR-0049).
///
/// `E` is what an item fails with mid-stream, and it is `Infallible` where the wire has no channel
/// to report one. A WebSocket frame is a frame: an error to a client is another message the gateway
/// shapes, not a failed item, and `Result<WsMessage, Infallible>` is niche-optimised to
/// `WsMessage`'s own layout, so saying so costs nothing.
pub enum Cardinality<T, E> {
    /// Nothing goes back. An `#[event_pattern]` handler's answer, and a WebSocket handler's when
    /// the frame it read needs none.
    Empty,
    /// One item.
    One(T),
    /// Items until the stream ends, each framed to the caller as it arrives. An `Err` item ends it.
    Many(BoxStream<'static, Result<T, E>>),
}

impl<T: Send + 'static> Cardinality<T, std::convert::Infallible> {
    /// A stream of items that cannot fail, for a wire with no channel to report one.
    ///
    /// [`Many`](Cardinality::Many) holds `Result` items whatever the transport, so this is what a
    /// WebSocket handler writes instead of mapping every item into an `Ok` the wire has no way to
    /// contradict.
    pub fn stream(items: impl futures::Stream<Item = T> + Send + 'static) -> Self {
        use futures::StreamExt as _;
        Cardinality::Many(items.map(Ok).boxed())
    }
}

impl<T: std::fmt::Debug, E> std::fmt::Debug for Cardinality<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cardinality::Empty => write!(f, "Empty"),
            Cardinality::One(item) => write!(f, "One({item:?})"),
            Cardinality::Many(_) => write!(f, "Many(..)"),
        }
    }
}

impl<T, E> From<T> for Cardinality<T, E> {
    fn from(item: T) -> Self {
        Cardinality::One(item)
    }
}

impl<T, E> From<Option<T>> for Cardinality<T, E> {
    fn from(item: Option<T>) -> Self {
        match item {
            Some(item) => Cardinality::One(item),
            None => Cardinality::Empty,
        }
    }
}
