//! A call's queue of frames received and not yet read, bounded by a count of items (transports
//! DESIGN §5.2, flow control): the client's queue of one call's reply frames, and the server's of
//! one streamed request's items.
//!
//! Its producer is a link's reader, which serves every call on the connection, so it never waits:
//! [`Feed::offer`] answers at once whether the item fitted, and the caller ends the one call whose
//! queue is full. The wire carries no window yet, `credit` being reserved, so ending the call is
//! the only way the bound holds.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use futures_channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures_core::Stream;
use futures_util::StreamExt;
use ulo_transport::Count;

/// The items a queue holds at `Count::Default`, on the client and the server alike.
pub(crate) const DEFAULT_MAX_QUEUED_ITEMS: u32 = 1024;

/// `count` as a queue's limit: [`DEFAULT_MAX_QUEUED_ITEMS`] for `Default`, `n` for `Max(n)`, and
/// `None`, no limit, for `Unlimited`.
pub(crate) fn limit_of(count: Count) -> Option<usize> {
    let items = match count {
        Count::Default => DEFAULT_MAX_QUEUED_ITEMS,
        Count::Max(items) => items,
        Count::Unlimited => return None,
    };
    Some(usize::try_from(items).unwrap_or(usize::MAX))
}

struct State {
    /// Items queued and not yet read, a call's last one included.
    held: AtomicUsize,
    limit: Option<usize>,
    /// Set by [`Feed::halt`]: the reader's next read ends the queue, as halted.
    halted: AtomicBool,
}

/// A queue holding at most `limit` items offered to it, `None` for no limit.
pub(crate) fn queue<T>(limit: Option<usize>) -> (Feed<T>, Drain<T>) {
    let (sender, receiver) = mpsc::unbounded();
    let state = Arc::new(State { held: AtomicUsize::new(0), limit, halted: AtomicBool::new(false) });
    (Feed { sender, state: Arc::clone(&state) }, Drain { receiver, state })
}

/// The producing end, cloned for each producer; the queue ends once every clone is dropped.
pub(crate) struct Feed<T> {
    sender: UnboundedSender<T>,
    state: Arc<State>,
}

impl<T> Clone for Feed<T> {
    fn clone(&self) -> Self {
        Feed { sender: self.sender.clone(), state: Arc::clone(&self.state) }
    }
}

impl<T> Feed<T> {
    /// Queues `item` unless the limit's worth of items already wait unread: `false` then, and
    /// `item` is dropped. An item offered once the reader has gone is dropped too, and answers
    /// `true`, since nothing is left to bound.
    pub(crate) fn offer(&self, item: T) -> bool {
        let held = self.state.held.fetch_add(1, Ordering::AcqRel);
        if self.state.limit.is_some_and(|limit| held >= limit) {
            self.state.held.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        if self.sender.unbounded_send(item).is_err() {
            self.state.held.fetch_sub(1, Ordering::AcqRel);
        }
        true
    }

    /// Queues `item` whatever the queue holds: a call's last item, after which nothing more is
    /// queued, so the queue holds at most one item past its limit.
    pub(crate) fn finish(&self, item: T) {
        self.state.held.fetch_add(1, Ordering::AcqRel);
        if self.sender.unbounded_send(item).is_err() {
            self.state.held.fetch_sub(1, Ordering::AcqRel);
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

    /// The queue's limit, `None` for none.
    pub(crate) fn limit(&self) -> Option<usize> {
        self.state.limit
    }

    /// What tells whether this queue was halted, holding no producing end, so the queue still
    /// ends when its producers do.
    pub(crate) fn halted(&self) -> Halted {
        Halted(Arc::clone(&self.state))
    }
}

/// Whether a queue was halted.
#[derive(Clone)]
pub(crate) struct Halted(Arc<State>);

impl Halted {
    pub(crate) fn get(&self) -> bool {
        self.0.halted.load(Ordering::Acquire)
    }

    /// The queue's limit, `None` for none.
    pub(crate) fn limit(&self) -> Option<usize> {
        self.0.limit
    }
}

/// The reading end.
pub(crate) struct Drain<T> {
    receiver: UnboundedReceiver<T>,
    state: Arc<State>,
}

impl<T> Drain<T> {
    /// The limit the queue overflowed, once [`Feed::halt`] has ended it; `None` while it has not,
    /// and after a clean end.
    pub(crate) fn halted_at(&self) -> Option<usize> {
        self.state.halted.load(Ordering::Acquire).then_some(self.state.limit).flatten()
    }
}

/// Ends at the end its producers give, or at once when halted, the items unread dropped.
impl<T> Stream for Drain<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        if self.state.halted.load(Ordering::Acquire) {
            return Poll::Ready(None);
        }
        match self.receiver.poll_next_unpin(cx) {
            Poll::Ready(Some(item)) => {
                self.state.held.fetch_sub(1, Ordering::AcqRel);
                // Halted between the check above and this read: the item is one left unread.
                if self.state.halted.load(Ordering::Acquire) {
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(item))
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Wake, Waker};

    use futures_util::FutureExt;

    use super::*;

    /// The limit counts what is unread: reading one item makes room for one more, and the last
    /// item fits past the limit.
    #[test]
    fn the_limit_counts_items_unread_and_the_last_fits_past_it() {
        let (feed, mut drain) = queue::<u32>(Some(2));
        assert!(feed.offer(1) && feed.offer(2), "two items fit a limit of two");
        assert!(!feed.offer(3), "a third item fitted a limit of two");
        assert_eq!(drain.next().now_or_never(), Some(Some(1)));
        assert!(feed.offer(4), "reading one item made no room");
        feed.finish(5);
        drop(feed);
        let rest: Vec<u32> = read_ready(&mut drain);
        assert_eq!(rest, vec![2, 4, 5], "the items read after the first, the last one past the limit");
    }

    /// A halted queue ends at its next read, its unread items dropped, and tells that end from a
    /// clean one; a queue that ends cleanly tells it too.
    #[test]
    fn a_halted_queue_ends_at_once_and_tells_its_end_from_a_clean_one() {
        let (feed, mut drain) = queue::<u32>(Some(4));
        assert!(feed.offer(1) && feed.offer(2));
        let halted = feed.halted();
        feed.halt();
        assert!(halted.get());
        assert_eq!(drain.next().now_or_never(), Some(None), "a halted queue yielded an unread item, or did not end");
        assert_eq!(drain.halted_at(), Some(4), "the halted end names the limit");

        let (feed, mut drain) = queue::<u32>(Some(4));
        feed.finish(1);
        drop(feed);
        assert_eq!(read_ready(&mut drain), vec![1]);
        assert_eq!(drain.next().now_or_never(), Some(None));
        assert_eq!(drain.halted_at(), None, "a clean end read as halted");
    }

    /// A reader waiting on an empty queue is woken by the halt, whichever task it waits on, with
    /// the queue's producing end still held.
    #[test]
    fn a_halt_wakes_a_waiting_reader() {
        let (feed, mut drain) = queue::<u32>(Some(4));
        let woken = Arc::new(Woken::default());
        let waker = Waker::from(Arc::clone(&woken));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut drain).poll_next(&mut cx).is_pending());
        feed.halt();
        assert!(woken.0.load(Ordering::SeqCst), "the halt left a waiting reader asleep");
        assert_eq!(Pin::new(&mut drain).poll_next(&mut cx), Poll::Ready(None));
        assert_eq!(drain.halted_at(), Some(4));
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
        let (feed, drain) = queue::<u32>(Some(2));
        drop(drain);
        assert!((0..10).all(|item| feed.offer(item)), "a queue with no reader refused an item");
    }

    /// Every item a queue with no limit is offered fits.
    #[test]
    fn a_queue_with_no_limit_takes_every_item() {
        let (feed, _drain) = queue::<u32>(None);
        assert!((0..10_000).all(|item| feed.offer(item)));
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
    }
}
