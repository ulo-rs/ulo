//! Scenarios: cancel, and how a streamed request read on a task of its own ends when its call
//! ends before the caller's last item.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::future::{Either, join, select};
use futures_util::{StreamExt, stream};
use serde::Serialize;
use ulo::{Bound, CancelReason};
use ulo_transport::{Classify, Count, ErrorKind, ExtractError};

use crate::Broker;
use crate::cases::app::{Fixture, Mounts, PARK, PARK_AND_ANSWER, TICKS, eventually, failed, refused_at_startup, streams, within};

const WAIT: Duration = Duration::from_secs(10);

/// `cancel` mid-stream fires `ClientCancelled` and stops the producer.
pub async fn mid_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false, every_instance: false }).await;
        return fixture.stop().await;
    }
    let settle = fixture.broker.budget().settle;
    let mut ticks = fixture.rpc().stream::<_, u64>(TICKS, &()).idle_timeout(Bound::After(Duration::from_secs(10))).await.expect("the stream opens");
    for _ in 0..2 {
        within::<B, _>(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields").expect("a tick arrives");
    }
    drop(ticks);
    let probe = fixture.server.probe.clone();
    assert!(eventually::<B>(settle.max(Duration::from_secs(2)), || probe.stopped()).await, "the producer kept running after `cancel`");
    assert_eq!(probe.cancelled(), Some(Some(CancelReason::ClientCancelled)));
    fixture.stop().await;
}

/// A streamed request its handler leaves for a task of its own ends there `Cancelled` when its
/// caller cancels the call before its last item: the caller left, not the server. The caller sends
/// one item and never `in_end`, and drops the call once the handler has the request.
pub async fn parked_request_after_cancel<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false, every_instance: false }).await;
        return fixture.stop().await;
    }
    let probe = fixture.server.probe.clone();
    let items = stream::iter([1i64]).chain(stream::pending());
    let call = async { fixture.rpc().send_stream::<_, i64>(PARK, items).timeout(WAIT).await };
    let parked = async {
        loop {
            if let Some(parked) = probe.take_parked() {
                return parked;
            }
            crate::rt::sleep::<B>(Duration::from_millis(20)).await;
        }
    };
    let parked = match within::<B, _>(WAIT, "the handler's taking the request", select(Box::pin(call), Box::pin(parked))).await {
        Either::Right((parked, call)) => {
            drop(call);
            parked
        }
        Either::Left((answer, _)) => panic!("the call was answered before its handler had the request: {answer:?}"),
    };
    let read: Vec<_> = within::<B, _>(WAIT, "the parked request's end", parked.collect()).await;
    let last = read.last().and_then(|item| item.as_ref().err()).unwrap_or_else(|| panic!("the request ended without a failure: {read:?}"));
    assert_eq!(last.classify(), ErrorKind::Cancelled, "the request's last item: {last}");
    fixture.stop().await;
}

/// A streamed request its handler leaves for a task of its own and answers before the caller's
/// last item ends there `CallEnded`, `Unavailable`: the server ended the call.
pub async fn parked_request_after_the_answer<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false, every_instance: false }).await;
        return fixture.stop().await;
    }
    let items = stream::iter([1i64]).chain(stream::pending());
    let answer = fixture.rpc().send_stream::<_, i64>(PARK_AND_ANSWER, items).timeout(WAIT).await;
    assert_eq!(answer.expect("the handler answers"), 0);
    let parked = fixture.server.probe.take_parked().expect("the handler left its request before it answered");
    let read: Vec<_> = within::<B, _>(WAIT, "the parked request's end", parked.collect()).await;
    let last = read.last().and_then(|item| item.as_ref().err()).unwrap_or_else(|| panic!("the request ended without a failure: {read:?}"));
    assert_eq!(last.classify(), ErrorKind::Unavailable, "the request's last item: {last}");
    fixture.stop().await;
}

/// The frame bound of the server's link in [`parked_request_over_the_frame_bound`], below the
/// client's and above the answer a TCP server writes to a frame over it.
const FRAME_BOUND: u64 = 64 * 1024;

/// An item of a streamed request: the number the parked handler reads, or text too large for the
/// server's link.
#[derive(Serialize)]
#[serde(untagged)]
enum Item {
    Number(i64),
    Text(String),
}

/// A streamed request its handler leaves for a task of its own ends there `TooLarge`, naming the
/// link's bound, when the server's link refuses one of its items for its size, and the call is
/// answered `bad_request` with `payload_too_large`: the server ended the call, not the caller. The
/// server's link refuses frames over 64 KiB, which the client's carries; the caller sends
/// the large item once the request's first item has been read, and never `in_end`.
pub async fn parked_request_over_the_frame_bound<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false, every_instance: false }).await;
        return fixture.stop().await;
    }
    fixture.stop().await;
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| {
        broker.link_with_max_frame(FRAME_BOUND).unwrap_or_else(|| {
            panic!("the server's link sets no frame bound below its client's (`Broker::link_with_max_frame`); declare this scenario not applicable")
        })
    })
    .await;
    let probe = fixture.server.probe.clone();
    let release = Arc::new(AtomicBool::new(false));
    let released = Arc::clone(&release);
    let large = stream::once(async move {
        while !released.load(Ordering::SeqCst) {
            crate::rt::sleep::<B>(Duration::from_millis(20)).await;
        }
        Item::Text("x".repeat(2 * usize::try_from(FRAME_BOUND).unwrap_or(usize::MAX / 2)))
    });
    let items = stream::iter([Item::Number(1)]).chain(large).chain(stream::pending());
    let call = async { fixture.rpc().send_stream::<_, i64>(PARK, items).timeout(WAIT).await };
    let read = async {
        let mut parked = loop {
            if let Some(parked) = probe.take_parked() {
                break parked;
            }
            crate::rt::sleep::<B>(Duration::from_millis(20)).await;
        };
        let first = parked.next().await;
        release.store(true, Ordering::SeqCst);
        let rest: Vec<_> = parked.collect().await;
        (first, rest)
    };
    let (answer, (first, rest)) = within::<B, _>(WAIT, "the call's answer and the parked request's end", join(call, read)).await;
    assert!(matches!(first, Some(Ok(1))), "the request's first item: {first:?}");
    match rest.as_slice() {
        [Err(error @ ExtractError::TooLarge { limit, .. })] => {
            assert_eq!(*limit, FRAME_BOUND, "the bound the request's end names: {error}");
            assert_eq!(error.classify(), ErrorKind::BadRequest, "{error}");
        }
        other => panic!("the request after its first item, which ends `TooLarge` alone: {other:?}"),
    }
    let error = failed(answer, ErrorKind::BadRequest);
    assert_eq!(error.reason(), Some("payload_too_large"), "{error:?}");
    fixture.stop().await;
}
