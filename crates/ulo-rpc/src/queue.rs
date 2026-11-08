//! A call's queue of frames received and not yet read, bounded by a count of items and by the
//! bytes they carry (transports DESIGN §5.2, flow control): the client's queue of one call's reply
//! frames, and the server's of one streamed request's items.
//!
//! Its producer is a link's reader, which serves every call on the connection, so it never waits:
//! [`Feed::offer`] answers at once whether the item fitted, and the caller ends the one call whose
//! queue is full. The wire carries no window yet, `credit` being reserved, so ending the call is
//! the only way the bounds hold.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use futures_channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures_core::Stream;
use futures_util::StreamExt;
use ulo_transport::Count;

/// The items a queue holds at `Count::Default`, on the client and the server alike.
pub(crate) const DEFAULT_MAX_QUEUED_ITEMS: u32 = 1024;

/// The bytes a queue holds unset, on the client and the server alike: 4 MiB, four frames at the
/// TCP link's 1 MiB `max_frame` default, and the item bound's worth of items of 4 KiB each, so items
/// smaller than that meet the item bound first and larger ones this.
pub(crate) const DEFAULT_MAX_QUEUED_BYTES: u64 = 4 * 1024 * 1024;

/// `count` as a queue's item limit: [`DEFAULT_MAX_QUEUED_ITEMS`] for `Default`, `n` for `Max(n)`,
/// and `None`, no limit, for `Unlimited`.
pub(crate) fn limit_of(count: Count) -> Option<usize> {
    let items = match count {
        Count::Default => DEFAULT_MAX_QUEUED_ITEMS,
        Count::Max(items) => items,
        Count::Unlimited => return None,
    };
    Some(usize::try_from(items).unwrap_or(usize::MAX))
}

/// A queue's two bounds: the items it holds, `None` for no limit, and the bytes they carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) items: Option<usize>,
    pub(crate) bytes: u64,
}

impl Limits {
    pub(crate) fn of(items: Count, bytes: u64) -> Limits {
        Limits { items: limit_of(items), bytes }
    }
}

/// The limits of `Count::Default` and [`DEFAULT_MAX_QUEUED_BYTES`].
impl Default for Limits {
    fn default() -> Self {
        Limits::of(Count::Default, DEFAULT_MAX_QUEUED_BYTES)
    }
}

/// Which bound the item past it would have passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// `limit` items waited unread.
    Items { limit: usize },
    /// The bytes waiting unread and the next item's `item` bytes would have passed `limit`. An
    /// `item` past `limit` on its own is one no queue under this bound holds.
    Bytes { limit: u64, item: u64 },
}

struct State {
    /// Items queued and not yet read, a call's last one included.
    held: AtomicUsize,
    /// The bytes those items carry.
    bytes: AtomicU64,
    limits: Limits,
    /// Set by [`Feed::halt`]: the reader's next read ends the queue, as halted.
    halted: AtomicBool,
    /// The bound a refused offer passed, recorded before the caller halts the queue.
    overflow: Mutex<Option<Overflow>>,
}

impl State {
    fn release(&self, bytes: u64) {
        self.held.fetch_sub(1, Ordering::AcqRel);
        self.bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    fn overflow(&self) -> Option<Overflow> {
        *self.overflow.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A queue holding at most `limits`' worth of the items offered to it.
pub(crate) fn queue<T>(limits: Limits) -> (Feed<T>, Drain<T>) {
    let (sender, receiver) = mpsc::unbounded();
    let state = Arc::new(State {
        held: AtomicUsize::new(0),
        bytes: AtomicU64::new(0),
        limits,
        halted: AtomicBool::new(false),
        overflow: Mutex::new(None),
    });
    (Feed { sender, state: Arc::clone(&state) }, Drain { receiver, state })
}

/// The producing end, cloned for each producer; the queue ends once every clone is dropped.
pub(crate) struct Feed<T> {
    sender: UnboundedSender<(T, u64)>,
    state: Arc<State>,
}

impl<T> Clone for Feed<T> {
    fn clone(&self) -> Self {
        Feed { sender: self.sender.clone(), state: Arc::clone(&self.state) }
    }
}

impl<T> Feed<T> {
    /// Queues `item`, which carries `bytes`, unless the item limit's worth of items already wait
    /// unread or the bytes waiting and `bytes` would pass the byte limit: the bound passed then,
    /// also recorded for [`Halted::overflow`], and `item` is dropped. An item over the byte limit
    /// on its own never fits. An item offered once the reader has gone is dropped too, and answers
    /// `Ok`, since nothing is left to bound.
    pub(crate) fn offer(&self, item: T, bytes: u64) -> Result<(), Overflow> {
        // A reader that has gone has released its counts and bounds nothing, an item over the
        // byte limit on its own included.
        if self.sender.is_closed() {
            return Ok(());
        }
        let held = self.state.held.fetch_add(1, Ordering::AcqRel);
        let carried = self.state.bytes.fetch_add(bytes, Ordering::AcqRel);
        let limits = self.state.limits;
        let passed = match limits.items {
            Some(limit) if held >= limit => Some(Overflow::Items { limit }),
            _ if carried.saturating_add(bytes) > limits.bytes => Some(Overflow::Bytes { limit: limits.bytes, item: bytes }),
            _ => None,
        };
        if let Some(passed) = passed {
            self.state.release(bytes);
            *self.state.overflow.lock().unwrap_or_else(PoisonError::into_inner) = Some(passed);
            return Err(passed);
        }
        if self.sender.unbounded_send((item, bytes)).is_err() {
            self.state.release(bytes);
        }
        Ok(())
    }

    /// Queues `item`, which carries `bytes`, whatever the queue holds: a call's last item, after
    /// which nothing more is queued, so the queue holds at most one item past its limits.
    pub(crate) fn finish(&self, item: T, bytes: u64) {
        self.state.held.fetch_add(1, Ordering::AcqRel);
        self.state.bytes.fetch_add(bytes, Ordering::AcqRel);
        if self.sender.unbounded_send((item, bytes)).is_err() {
            self.state.release(bytes);
        }
    }

    /// Ends the queue as halted: the reader's next read ends it, the items it had not read
    /// dropped, and [`Drain::halted_at`] tells that end from a clean one, so a handler reading a
    /// request cut short does not take it for a complete one. A reader waiting for an item is
    /// woken, whichever task it reads on.
    pub(crate) fn halt(&self) {
        self.state.halted.store(true, Ordering::Release);
        // Closing wakes a waiting reader; the flag is stored first, so the read that finds the
        // queue closed finds it halted.
        self.sender.close_channel();
    }

    /// Whether the reader has gone.
    pub(crate) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// What tells whether this queue was halted, holding no producing end, so the queue still
    /// ends when its producers do.
    pub(crate) fn halted(&self) -> Halted {
        Halted(Arc::clone(&self.state))
    }

    /// The items and bytes queued and not yet read.
    #[cfg(test)]
    pub(crate) fn held(&self) -> (usize, u64) {
        (self.state.held.load(Ordering::Acquire), self.state.bytes.load(Ordering::Acquire))
    }
}

/// Whether a queue was halted.
#[derive(Clone)]
pub(crate) struct Halted(Arc<State>);

impl Halted {
    pub(crate) fn get(&self) -> bool {
        self.0.halted.load(Ordering::Acquire)
    }

    /// The bound the queue's refused offer passed; `None` while every offer fitted.
    pub(crate) fn overflow(&self) -> Option<Overflow> {
        self.0.overflow()
    }
}

/// The reading end. Dropped, or ended halted, it lets go of the items it had not read, so the
/// queue's counts return to zero once nothing is left to read.
pub(crate) struct Drain<T> {
    receiver: UnboundedReceiver<(T, u64)>,
    state: Arc<State>,
}

impl<T> Drain<T> {
    /// The bound the queue overflowed, once [`Feed::halt`] has ended it; `None` while it has not,
    /// and after a clean end.
    pub(crate) fn halted_at(&self) -> Option<Overflow> {
        if self.state.halted.load(Ordering::Acquire) { self.state.overflow() } else { None }
    }

    /// Closes the queue and lets go of every item it holds unread; an offer after this is dropped
    /// at the offer.
    fn discard(&mut self) {
        self.receiver.close();
        // Once closed, the receiver answers `Pending` only while a sender that counted its item
        // before the close has yet to push it, so the wait is that push's, and the item is
        // released rather than dropped with the receiver still counted.
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            match self.receiver.poll_next_unpin(&mut cx) {
                Poll::Ready(Some((_, bytes))) => self.state.release(bytes),
                Poll::Ready(None) => break,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}

/// Ends at the end its producers give, or at once when halted, the items unread dropped.
impl<T> Stream for Drain<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        if self.state.halted.load(Ordering::Acquire) {
            self.discard();
            return Poll::Ready(None);
        }
        match self.receiver.poll_next_unpin(cx) {
            Poll::Ready(Some((item, bytes))) => {
                self.state.release(bytes);
                // Halted between the check above and this read: the item is one left unread.
                if self.state.halted.load(Ordering::Acquire) {
                    self.discard();
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for Drain<T> {
    fn drop(&mut self) {
        self.discard();
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Wake, Waker};

    use futures_util::FutureExt;

    use super::*;

    fn items(limit: usize) -> Limits {
        Limits { items: Some(limit), bytes: u64::MAX }
    }

    fn fits(feed: &Feed<u32>, item: u32, bytes: u64) -> bool {
        feed.offer(item, bytes).is_ok()
    }

    /// The limit counts what is unread: reading one item makes room for one more, and the last
    /// item fits past the limit.
    #[test]
    fn the_limit_counts_items_unread_and_the_last_fits_past_it() {
        let (feed, mut drain) = queue::<u32>(items(2));
        assert!(fits(&feed, 1, 0) && fits(&feed, 2, 0), "two items fit a limit of two");
        assert_eq!(feed.offer(3, 0), Err(Overflow::Items { limit: 2 }), "a third item fitted a limit of two");
        assert_eq!(drain.next().now_or_never(), Some(Some(1)));
        assert!(fits(&feed, 4, 0), "reading one item made no room");
        feed.finish(5, 0);
        drop(feed);
        let rest: Vec<u32> = read_ready(&mut drain);
        assert_eq!(rest, vec![2, 4, 5], "the items read after the first, the last one past the limit");
    }

    /// F584: a few large items meet the byte bound with the item bound far away, the refusal
    /// naming the bytes; reading one makes room for its bytes, and the last item fits past both.
    #[test]
    fn a_few_large_items_meet_the_byte_bound_far_below_the_item_bound() {
        let (feed, mut drain) = queue::<u32>(Limits { items: Some(1024), bytes: 100 });
        assert!(fits(&feed, 1, 40) && fits(&feed, 2, 40), "80 bytes fit a bound of 100");
        assert_eq!(feed.offer(3, 40), Err(Overflow::Bytes { limit: 100, item: 40 }), "120 bytes fitted a bound of 100");
        assert_eq!(feed.held(), (2, 80), "the refused item left its count and its bytes behind");
        assert_eq!(drain.next().now_or_never(), Some(Some(1)));
        assert!(fits(&feed, 4, 60), "reading 40 bytes made no room for 60");
        feed.finish(5, 500);
        assert_eq!(feed.held(), (3, 600), "the last item fits past the byte bound");
        drop(feed);
        assert_eq!(read_ready(&mut drain), vec![2, 4, 5]);
    }

    /// F584: many tiny items meet the item bound long before the byte bound.
    #[test]
    fn many_tiny_items_meet_the_item_bound_before_the_byte_bound() {
        let (feed, _drain) = queue::<u32>(Limits { items: Some(1024), bytes: 4 * 1024 * 1024 });
        assert!((0..1024).all(|item| fits(&feed, item, 8)), "1,024 items of eight bytes fit");
        assert_eq!(feed.offer(1024, 8), Err(Overflow::Items { limit: 1024 }), "the 1,025th item fitted");
    }

    /// F584: an item over the byte bound on its own is refused at once, on an empty queue, rather
    /// than waiting for room that never comes.
    #[test]
    fn an_item_over_the_byte_bound_on_its_own_is_refused_on_an_empty_queue() {
        let (feed, _drain) = queue::<u32>(Limits { items: Some(1024), bytes: 100 });
        assert_eq!(feed.offer(1, 101), Err(Overflow::Bytes { limit: 100, item: 101 }), "an item of 101 bytes fitted a bound of 100");
        assert_eq!(feed.held(), (0, 0));
        assert!(fits(&feed, 2, 100), "an item of exactly the bound fits an empty queue");
    }

    /// F584: the counts return to zero once the items are read, once the reader drops with items
    /// unread, as a cancelled call's does, once the producer and then the reader drop, as on a lost
    /// connection, and once a halted queue's next read ends it.
    #[test]
    fn the_counts_return_to_zero_after_a_read_a_cancel_a_loss_and_a_halt() {
        let (feed, mut drain) = queue::<u32>(Limits { items: Some(8), bytes: 1000 });
        assert!(fits(&feed, 1, 100) && fits(&feed, 2, 200));
        feed.finish(3, 300);
        assert_eq!(read_ready(&mut drain), vec![1, 2, 3]);
        assert_eq!(feed.held(), (0, 0), "after every item was read");

        let (feed, drain) = queue::<u32>(Limits { items: Some(8), bytes: 1000 });
        assert!(fits(&feed, 1, 100) && fits(&feed, 2, 200));
        drop(drain);
        assert_eq!(feed.held(), (0, 0), "after the reader dropped with items unread");
        assert!(fits(&feed, 3, 900), "an offer once the reader dropped");
        assert_eq!(feed.held(), (0, 0), "after an offer once the reader dropped");

        let (feed, drain) = queue::<u32>(Limits { items: Some(8), bytes: 1000 });
        assert!(fits(&feed, 1, 100) && fits(&feed, 2, 200));
        let watching = feed.clone();
        drop(feed);
        drop(drain);
        assert_eq!(watching.held(), (0, 0), "after the producer and then the reader dropped");

        let (feed, mut drain) = queue::<u32>(Limits { items: Some(8), bytes: 1000 });
        assert!(fits(&feed, 1, 100) && fits(&feed, 2, 200));
        feed.halt();
        assert_eq!(drain.next().now_or_never(), Some(None));
        assert_eq!(feed.held(), (0, 0), "after a halted queue's next read");
    }

    /// A halted queue ends at its next read, its unread items dropped, and tells that end from a
    /// clean one; a queue that ends cleanly tells it too.
    #[test]
    fn a_halted_queue_ends_at_once_and_tells_its_end_from_a_clean_one() {
        let (feed, mut drain) = queue::<u32>(items(2));
        assert!(fits(&feed, 1, 0) && fits(&feed, 2, 0) && !fits(&feed, 3, 0));
        let halted = feed.halted();
        feed.halt();
        assert!(halted.get());
        assert_eq!(drain.next().now_or_never(), Some(None), "a halted queue yielded an unread item, or did not end");
        assert_eq!(drain.halted_at(), Some(Overflow::Items { limit: 2 }), "the halted end names the bound");
        assert_eq!(halted.overflow(), Some(Overflow::Items { limit: 2 }));

        let (feed, mut drain) = queue::<u32>(items(4));
        feed.finish(1, 0);
        drop(feed);
        assert_eq!(read_ready(&mut drain), vec![1]);
        assert_eq!(drain.next().now_or_never(), Some(None));
        assert_eq!(drain.halted_at(), None, "a clean end read as halted");
    }

    /// A reader waiting on an empty queue is woken by the halt, whichever task it waits on, with
    /// the queue's producing end still held.
    #[test]
    fn a_halt_wakes_a_waiting_reader() {
        let (feed, mut drain) = queue::<u32>(Limits { items: Some(4), bytes: 10 });
        let woken = Arc::new(Woken::default());
        let waker = Waker::from(Arc::clone(&woken));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut drain).poll_next(&mut cx).is_pending());
        assert!(!fits(&feed, 1, 11));
        feed.halt();
        assert!(woken.0.load(Ordering::SeqCst), "the halt left a waiting reader asleep");
        assert_eq!(Pin::new(&mut drain).poll_next(&mut cx), Poll::Ready(None));
        assert_eq!(drain.halted_at(), Some(Overflow::Bytes { limit: 10, item: 11 }));
    }

    #[derive(Default)]
    struct Woken(AtomicBool);

    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// An item offered once the reader has gone is dropped and takes no room: a handler that drops
    /// its request early and goes on working is not ended by the items that follow.
    #[test]
    fn a_queue_whose_reader_has_gone_refuses_nothing() {
        let (feed, drain) = queue::<u32>(Limits { items: Some(2), bytes: 10 });
        drop(drain);
        assert!((0..10).all(|item| fits(&feed, item, 5)), "a queue with no reader refused an item");
        assert!(fits(&feed, 10, 11), "a queue with no reader refused an item over its byte bound on its own");
    }

    /// A reader dropped on one thread while a producer offers on another leaves no count behind:
    /// an item counted before the drop's close and pushed after it is released by the drop, not
    /// dropped with the receiver still counted.
    #[test]
    fn a_reader_dropped_while_a_producer_offers_leaves_the_counts_at_zero() {
        for reads in 0..2_000usize {
            let (feed, drain) = queue::<u32>(Limits { items: Some(64), bytes: 6_400 });
            let watching = feed.clone();
            let producer = std::thread::spawn(move || {
                for item in 0..200 {
                    let _ = feed.offer(item, 100);
                }
            });
            let reader = std::thread::spawn(move || {
                let mut drain = drain;
                let mut cx = Context::from_waker(Waker::noop());
                for _ in 0..reads % 50 {
                    let _ = Pin::new(&mut drain).poll_next(&mut cx);
                }
            });
            reader.join().expect("the reader");
            producer.join().expect("the producer");
            assert_eq!(watching.held(), (0, 0), "counts left after a drop beside {reads} reads");
        }
    }

    /// Every item a queue with no item limit and no byte limit is offered fits.
    #[test]
    fn a_queue_with_no_limit_takes_every_item() {
        let (feed, _drain) = queue::<u32>(Limits { items: None, bytes: u64::MAX });
        assert!((0..10_000).all(|item| fits(&feed, item, 1 << 40)));
    }

    fn read_ready(drain: &mut Drain<u32>) -> Vec<u32> {
        let mut items = Vec::new();
        while let Some(Some(item)) = drain.next().now_or_never() {
            items.push(item);
        }
        items
    }

    #[test]
    fn counts_read_as_limits() {
        assert_eq!(limit_of(Count::Default), Some(1024));
        assert_eq!(limit_of(Count::Max(3)), Some(3));
        assert_eq!(limit_of(Count::Unlimited), None);
        assert_eq!(Limits::default(), Limits { items: Some(1024), bytes: 4 * 1024 * 1024 });
    }
}
