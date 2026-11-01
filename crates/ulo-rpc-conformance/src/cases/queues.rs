//! Scenarios: the per-call queues, `max_queued_items` on the client and on the server. On a link
//! whose `shapes` exclude the streamed ones, each asserts the startup refusal instead: a call
//! answered by one reply holds one frame, which no limit ends.

use std::time::Duration;

use futures_util::{StreamExt, stream};
use ulo::{Bound, CancelReason};
use ulo_transport::{Count, ErrorKind};

use crate::Broker;
use crate::cases::app::{Fixture, IGNORE, Mounts, TICKS, eventually, failed, refused_at_startup, streams, within};

const WAIT: Duration = Duration::from_secs(10);

/// The limit both scenarios set, small enough that the ticking stream reaches it in a fraction of
/// a second.
const LIMIT: u32 = 8;

/// A reply stream its caller does not read ends once the client holds `LIMIT` items: the
/// producer is cancelled `ClientCancelled` while nothing has been read, and the caller then reads
/// the `LIMIT` items held and `Unavailable` with the reason `overflow`.
pub async fn reply_overflow<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let rpc = fixture.rpc().clone().max_queued_items(Count::Max(LIMIT)).expect("a limit of eight");
    let ticks = rpc.stream::<_, u64>(TICKS, &()).idle_timeout(Bound::After(WAIT)).await.expect("the stream opens");
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(eventually::<B>(settle.max(Duration::from_secs(5)), || probe.stopped()).await, "the producer kept running past the overflow");
    assert_eq!(probe.cancelled(), Some(Some(CancelReason::ClientCancelled)), "how the producer ended");
    let items: Vec<_> = within::<B, _>(WAIT, "the overflowed stream", ticks.collect()).await;
    let held: Vec<u64> = items.iter().map_while(|item| item.as_ref().ok().copied()).collect();
    assert_eq!(held, (0..u64::from(LIMIT)).collect::<Vec<_>>(), "the items held at the overflow: {items:?}");
    assert_eq!(items.len(), LIMIT as usize + 1, "the stream did not end at its overflow: {items:?}");
    let error = items.into_iter().last().and_then(Result::err).expect("the stream ends with the overflow");
    assert_eq!((error.kind(), error.reason()), (ErrorKind::Unavailable, Some("overflow")), "{error}");
    fixture.stop().await;
}

/// A streamed request whose handler reads none of it ends once the server holds `LIMIT` items:
/// the caller is answered `Unavailable` with the reason `overflow`.
pub async fn request_overflow<B: Broker>() {
    let fixture = Fixture::<B>::start_queued(Count::Max(LIMIT)).await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let items = stream::iter(0..i64::from(LIMIT) * 4);
    let answer = fixture.rpc().send_stream::<_, i64>(IGNORE, items).timeout(WAIT).await;
    let error = failed(answer, ErrorKind::Unavailable);
    assert_eq!(error.reason(), Some("overflow"), "{error}");
    fixture.stop().await;
}
