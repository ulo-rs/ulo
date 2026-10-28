//! `ScriptedTimer` against sleeps polled by hand: what `live`, `pending_at` and `refused` answer at
//! each step.

use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use ulo::Timer;
use ulo_conformance_support::ScriptedTimer;

const SHORT: Duration = Duration::from_secs(1);
const LONG: Duration = Duration::from_secs(5);

fn poll(sleep: &mut (impl Future<Output = ()> + Unpin)) -> Poll<()> {
    std::pin::Pin::new(sleep).poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn a_sleep_not_yet_polled_has_no_pending_time() {
    let clock = ScriptedTimer::new();
    let _sleep = clock.sleep(SHORT);
    assert_eq!(clock.pending_at(SHORT), None);
}

#[test]
fn a_pending_poll_records_the_clock_time_it_saw() {
    let clock = ScriptedTimer::new();
    let mut sleep = clock.sleep(LONG);
    assert!(poll(&mut sleep).is_pending());
    assert_eq!(clock.pending_at(LONG), Some(Duration::ZERO));

    clock.advance(Duration::from_secs(2));
    assert_ne!(clock.pending_at(LONG), Some(clock.elapsed()), "the sleep has not yet seen the advance");
    assert!(poll(&mut sleep).is_pending());
    assert_eq!(clock.pending_at(LONG), Some(Duration::from_secs(2)));
    assert_eq!(clock.pending_at(LONG), Some(clock.elapsed()));
}

#[test]
fn the_poll_that_ends_a_sleep_leaves_its_last_pending_time() {
    let clock = ScriptedTimer::new();
    let mut sleep = clock.sleep(SHORT);
    clock.advance(SHORT - Duration::from_millis(1));
    assert!(poll(&mut sleep).is_pending());
    clock.advance(Duration::from_millis(1));
    assert_eq!(poll(&mut sleep), Poll::Ready(()));
    assert_eq!(clock.pending_at(SHORT), Some(SHORT - Duration::from_millis(1)));
    assert_ne!(clock.pending_at(SHORT), Some(clock.elapsed()));
}

#[test]
fn pending_times_are_kept_per_sleep_length() {
    let clock = ScriptedTimer::new();
    let mut short = clock.sleep(SHORT);
    let mut long = clock.sleep(LONG);
    assert!(poll(&mut short).is_pending());
    clock.advance(Duration::from_millis(500));
    assert!(poll(&mut long).is_pending());
    assert_eq!(clock.pending_at(SHORT), Some(Duration::ZERO));
    assert_eq!(clock.pending_at(LONG), Some(Duration::from_millis(500)));
    assert_eq!(clock.pending_at(Duration::from_secs(9)), None);
}

#[test]
fn live_counts_sleeps_until_they_drop() {
    let clock = ScriptedTimer::new();
    let first = clock.sleep(SHORT);
    let second = clock.sleep(SHORT);
    assert_eq!(clock.live(SHORT), 2);
    drop(first);
    assert_eq!(clock.live(SHORT), 1);
    drop(second);
    assert_eq!(clock.live(SHORT), 0);
}

/// A refused length is answered at once, `try_sleep` with the refusal, and counted; another length
/// keeps its script.
#[test]
fn a_refused_length_ends_at_once_and_is_counted() {
    let clock = ScriptedTimer::new();
    clock.refuse(SHORT);
    assert_eq!(poll(&mut clock.sleep(SHORT)), Poll::Ready(()));
    let refused = std::pin::Pin::new(&mut clock.try_sleep(SHORT)).poll(&mut Context::from_waker(Waker::noop()));
    assert_eq!(refused, Poll::Ready(Err(ulo::Refused)));
    assert_eq!(clock.refused(SHORT), 2);
    assert_eq!(clock.live(SHORT), 0, "a refused sleep is not live");
    assert!(poll(&mut clock.sleep(LONG)).is_pending(), "a length not refused ended");
    assert_eq!(clock.refused(LONG), 0);
}
