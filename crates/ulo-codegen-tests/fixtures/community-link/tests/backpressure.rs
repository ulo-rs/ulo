//! The link's queues hold a sender back once full and release it as the other side reads, which
//! is what `ulo_transport::sync::ordered` gives a link and an unbounded queue does not.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use community_link::{Hub, Loopback};
use futures_util::task::noop_waker_ref;
use futures_util::{FutureExt, StreamExt};
use ulo::{BoxFuture, Refused, Runtime, Spawn, TaskHandle, Timer};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern};

/// The runtime `connect` is handed, which this link never uses.
struct Unused;

impl Timer for Unused {
    fn sleep(&self, _: Duration) -> BoxFuture<'static, ()> {
        unreachable!("the link takes no timer")
    }

    fn now(&self) -> Instant {
        unreachable!("the link takes no timer")
    }

    fn try_sleep(&self, _: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        unreachable!("the link takes no timer")
    }
}

impl Spawn for Unused {
    fn spawn(&self, _: BoxFuture<'static, ()>) -> TaskHandle {
        unreachable!("the link spawns no task")
    }

    fn try_spawn(&self, _: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        unreachable!("the link spawns no task")
    }
}

#[test]
fn a_server_that_reads_nothing_holds_the_sender_back_until_it_reads() {
    let hub = Hub::new();
    let server = Loopback::on(&hub);
    let pattern = Pattern::new("p");
    let mut inbound = server.listen(std::slice::from_ref(&pattern)).now_or_never().unwrap().unwrap();
    let runtime: Arc<dyn Runtime> = Arc::new(Unused);
    let client = Loopback::on(&hub).connect(&runtime).now_or_never().unwrap().unwrap();
    let mut cx = Context::from_waker(noop_waker_ref());

    let event = || Frame::Evt { pattern: "p".to_owned(), headers: CallHeaders::new(), data: Data::new(Vec::new()) };
    let mut held = None;
    for sent in 0..100_000 {
        let mut send = (client.send)(pattern.clone(), event(), None);
        match send.poll_unpin(&mut cx) {
            Poll::Ready(result) => result.unwrap(),
            Poll::Pending => {
                held = Some((sent, send));
                break;
            }
        }
    }
    let (sent, mut send) = held.expect("no send waited for room: the server's queue is unbounded");
    assert!(sent > 0, "the first send waited for room");
    assert!(send.poll_unpin(&mut cx).is_pending());
    assert!(inbound.next().now_or_never().flatten().is_some());
    assert!(matches!(send.poll_unpin(&mut cx), Poll::Ready(Ok(()))), "a read did not make room");
}
