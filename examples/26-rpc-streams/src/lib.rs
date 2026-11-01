use std::fmt;
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use ulo::{App, AppBuilder, Dep, Timer, injectable, module, routes};
use ulo_rpc::{Classify, Inbound, Payload};

/// A streamed request item that did not decode.
#[derive(Debug, Classify)]
#[classify(bad_request)]
pub struct BadItem;

impl fmt::Display for BadItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a streamed item did not decode")
    }
}

impl std::error::Error for BadItem {}

/// The call shape comes from the signature: an `Inbound` parameter is a streamed request, a stream
/// return a streamed reply, both together a duplex call.
#[injectable]
pub struct Streams;

#[routes]
impl Streams {
    #[ulo_rpc::message("stream.up")]
    async fn up(&self, n: Payload<u32>) -> impl Stream<Item = Result<u32, BadItem>> {
        stream::iter((1..=n.0).map(Ok))
    }

    #[ulo_rpc::message("stream.sum")]
    async fn sum(&self, mut items: Inbound<i64>) -> Result<i64, BadItem> {
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item.map_err(|_| BadItem)?;
        }
        Ok(total)
    }

    #[ulo_rpc::message("stream.double")]
    async fn double(&self, items: Inbound<i64>) -> impl Stream<Item = Result<i64, BadItem>> {
        items.map(|item| item.map(|n| n * 2).map_err(|_| BadItem))
    }

    /// One item, then silence for as long as the call lives.
    #[ulo_rpc::message("stream.stall")]
    async fn stall(&self) -> impl Stream<Item = Result<u32, BadItem>> {
        stream::once(async { Ok(1) }).chain(stream::pending())
    }

    /// An item every ten milliseconds, with no end.
    #[ulo_rpc::message("stream.ticks")]
    async fn ticks(&self, timer: Dep<dyn Timer>) -> impl Stream<Item = Result<u64, BadItem>> {
        stream::unfold((0u64, timer), |(n, timer)| async move {
            timer.sleep(Duration::from_millis(10)).await;
            Some((Ok(n), (n + 1, timer)))
        })
    }
}

#[module(controllers = [Streams])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
