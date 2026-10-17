//! Scenarios: a client called from a thread no runtime runs, and a link's connect and bind
//! polled on a thread that runs every other task too.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use ulo::{Bound, Runtime};
use ulo_rpc::{Link, RpcClient, RpcError};

use crate::Broker;
use crate::cases::app::{ADD, Add, COUNT, EVENT, Fixture, SUM, Sum, client, eventually, joined, ready, server, streams};
use crate::rt::{runtime, sleep, spawn, timeout};

/// Each call's own timeout.
const WAIT: Duration = Duration::from_secs(10);

/// How long the thread may take for every call it makes.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the thread's calls answered.
struct Answers {
    sum: Result<Sum, RpcError>,
    emitted: Result<(), RpcError>,
    /// `None` on a link whose shapes exclude the streamed ones.
    counted: Option<Result<Vec<Result<u32, RpcError>>, RpcError>>,
    summed: Option<Result<i64, RpcError>>,
}

/// A client built with `RpcClient::new` and called from a plain `std::thread`, which starts with no
/// runtime current, driven by `futures::executor::block_on`: a unary call is answered, an event
/// reaches its handler, and on a link carrying the streamed shapes a streamed reply and a streamed
/// request are answered. The link is built where the scenario runs, so it holds the scenario's
/// runtime and runs its I/O there; the client spawns on that runtime too, held by its handle. The
/// client is dropped on the thread.
///
/// A link whose futures need a tokio runtime current where they are polled fails here: the thread
/// panics, and the scenario reports the panic.
pub async fn plain_thread<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let link = fixture.broker.client_link(&fixture.server.addresses);
    let carries_streams = streams(&link.capabilities());
    let runtime: Arc<dyn Runtime> = runtime::<B>();

    let thread = std::thread::spawn(move || {
        let client = RpcClient::new(link, runtime).expect("the scenario's link holds the scenario's runtime");
        let answers = futures_executor::block_on(async {
            let sum = client.request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(WAIT).await;
            let emitted = client.emit(EVENT, &11u32).await;
            let (counted, summed) = if carries_streams {
                let counted = match client.stream::<_, u32>(COUNT, &3u32).idle_timeout(Bound::After(WAIT)).await {
                    Ok(replies) => Ok(replies.collect::<Vec<_>>().await),
                    Err(error) => Err(error),
                };
                let summed = client.send_stream::<_, i64>(SUM, stream::iter(vec![1i64, 2, 3, 4])).timeout(WAIT).await;
                (Some(counted), Some(summed))
            } else {
                (None, None)
            };
            Answers { sum, emitted, counted, summed }
        });
        drop(client);
        answers
    });
    // Joined once it has finished, so the join does not block a thread the scenario's runtime runs.
    let finished = timeout::<B, _>(PATIENCE, async {
        while !thread.is_finished() {
            sleep::<B>(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(finished.is_some(), "the client's thread did not finish within {PATIENCE:?}");
    let answers = thread.join().unwrap_or_else(|panic| panic!("the client's thread panicked: {}", message(&*panic)));

    assert_eq!(answers.sum.expect("the unary call from the thread is answered"), Sum { sum: 5 });
    answers.emitted.expect("the event from the thread is published");
    let settle = fixture.broker.budget().settle;
    let probe = fixture.server.probe.clone();
    assert!(eventually::<B>(settle, || probe.last_event() == Some(11)).await, "the event from the thread did not reach its handler within {settle:?}");
    if let Some(counted) = answers.counted {
        let items: Vec<u32> =
            counted.expect("the stream from the thread opens").into_iter().map(|item| item.expect("every item arrives")).collect();
        assert_eq!(items, vec![1, 2, 3]);
    }
    if let Some(summed) = answers.summed {
        assert_eq!(summed.expect("the streamed request from the thread is answered"), 10);
    }
    fixture.stop().await;
}

fn message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "a payload that is not text".to_owned())
}

/// How many fresh clients [`connecting_does_not_stall_the_thread`] connects, each making a new
/// connection and, on a broker, its own reply lane.
const CONNECTS: usize = 3;

/// What the ticker saw: the longest gap between two of its ticks, what the scenario was doing when
/// that gap ended, how many ticks it made, and every thread that polled it. A window shorter than
/// one tick still measures a stall inside it: the ticker's next poll comes after the stall, and the
/// gap that ends there includes it.
struct Ticks {
    longest: Duration,
    during: &'static str,
    count: usize,
    threads: Vec<ThreadId>,
}

/// Connecting doesn't stall the thread. Run through `Harness::block_on_one_thread`, so the
/// scenario, the server, the clients and every task they spawn are polled on one thread: a ticker
/// task sleeping a tenth of the bound between ticks records the longest gap between two of them
/// while a server binds and three fresh clients each connect and are answered. The gap stays
/// under the broker's [`Budget::stall`](crate::Budget::stall), 50 ms unless it sets another, and
/// every tick ran on the scenario's own thread, so a harness polling the ticker elsewhere fails
/// rather than hiding a stall. A link doing blocking work on the thread polling its connect or its
/// `listen` stalls the ticker with it: a trust-store read, or a librdkafka handle whose drop joins a
/// thread.
pub async fn connecting_does_not_stall_the_thread<B: Broker>() {
    let broker = B::start().await;
    let bound = broker.budget().stall;
    let doing: Arc<Mutex<&'static str>> = Arc::new(Mutex::new("starting the ticker"));
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = spawn::<B, _, _>({
        let (doing, stop) = (Arc::clone(&doing), Arc::clone(&stop));
        async move {
            let mut ticks = Ticks { longest: Duration::ZERO, during: "", count: 0, threads: Vec::new() };
            let mut last = Instant::now();
            while !stop.load(Ordering::Acquire) {
                sleep::<B>(bound / 10).await;
                let now = Instant::now();
                if now - last > ticks.longest {
                    ticks.longest = now - last;
                    ticks.during = *doing.lock().unwrap_or_else(PoisonError::into_inner);
                }
                last = now;
                ticks.count += 1;
                let polled_on = thread::current().id();
                if !ticks.threads.contains(&polled_on) {
                    ticks.threads.push(polled_on);
                }
            }
            ticks
        }
    });
    let doing = |what: &'static str| *doing.lock().unwrap_or_else(PoisonError::into_inner) = what;

    doing("binding the server");
    let server = server(&broker).await;
    let mut clients = Vec::with_capacity(CONNECTS);
    for _ in 0..CONNECTS {
        doing("connecting a client");
        let client = client(&broker, &[&server]).await;
        ready(&broker, &client.rpc, &[&server]).await;
        clients.push(client);
    }
    doing("stopping the ticker");
    stop.store(true, Ordering::Release);
    let ticks = joined(ticker.await, "the ticker");
    for client in clients {
        client.close().await;
    }
    server.stop().await;

    assert_eq!(
        ticks.threads,
        vec![thread::current().id()],
        "the ticker was polled on a thread other than the scenario's: the harness's `block_on_one_thread` runs tasks elsewhere"
    );
    assert!(ticks.count >= 1, "the ticker never ticked while the server bound and {CONNECTS} clients connected");
    assert!(
        ticks.longest < bound,
        "the scenario's thread went {:.1} ms without polling the ticker, which sleeps {:.1} ms between ticks, while {}; the bound is {:.1} ms",
        ticks.longest.as_secs_f64() * 1e3,
        (bound / 10).as_secs_f64() * 1e3,
        ticks.during,
        bound.as_secs_f64() * 1e3
    );
}
