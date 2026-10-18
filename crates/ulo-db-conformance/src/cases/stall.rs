use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use crate::Host;
use crate::cases::{Root, builder, closed, connected, joined, lock, sleep, spawn, store};

/// How many apps the scenario connects in turn, each building a client and pinging it.
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

/// Connecting does not stall a one-thread runtime, run through `Harness::block_on_one_thread` and
/// held to [`Budget::stall`](crate::Budget::stall). Planted break: a client created on the worker
/// outside `spawn_with`.
///
/// The scenario, the store's client and every task either spawns are polled on the scenario's own
/// thread. A ticker task sleeping a tenth of the bound between ticks records the longest gap between
/// two of them while three apps in turn wire and connect, each building the store's client through
/// `Store::create` and confirming it with a ping, and then close. The gap stays under the host's
/// stall bound, and every tick ran on the scenario's thread, so a harness polling the ticker
/// elsewhere fails rather than hiding a stall. A store doing blocking work on the thread polling its
/// creation or its ping stalls the ticker with it: a trust-store read, a DNS lookup, a synchronous
/// connect.
pub async fn connecting_does_not_stall_the_thread<H: Host>() {
    let host = H::start().await;
    let bound = host.budget().stall;
    let doing: Arc<Mutex<&'static str>> = Arc::new(Mutex::new("starting the ticker"));
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = spawn::<H, _, _>({
        let (doing, stop) = (Arc::clone(&doing), Arc::clone(&stop));
        async move {
            let mut ticks = Ticks { longest: Duration::ZERO, during: "", count: 0, threads: Vec::new() };
            let mut last = Instant::now();
            while !stop.load(Ordering::Acquire) {
                sleep::<H>(bound / 10).await;
                let now = Instant::now();
                if now - last > ticks.longest {
                    ticks.longest = now - last;
                    ticks.during = *lock(&doing);
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
    let doing = |what: &'static str| *lock(&doing) = what;

    for _ in 0..CONNECTS {
        doing("connecting an app");
        let app = connected(builder::<H>(Root::new().import(store::<H>(host.options()))), "the app").await;
        doing("closing an app");
        closed(app, "the app").await;
    }
    doing("stopping the ticker");
    stop.store(true, Ordering::Release);
    let ticks = joined(ticker.await, "the ticker");

    assert_eq!(
        ticks.threads,
        vec![thread::current().id()],
        "the ticker was polled on a thread other than the scenario's: the harness's `block_on_one_thread` runs tasks elsewhere"
    );
    assert!(ticks.count >= 1, "the ticker never ticked while {CONNECTS} apps connected");
    assert!(
        ticks.longest < bound,
        "the scenario's thread went {:.1} ms without polling the ticker, which sleeps {:.1} ms between ticks, while {}; the bound is {:.1} ms",
        ticks.longest.as_secs_f64() * 1e3,
        (bound / 10).as_secs_f64() * 1e3,
        ticks.during,
        bound.as_secs_f64() * 1e3
    );
}
