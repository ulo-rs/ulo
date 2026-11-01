//! SQLite is synchronous, so a slow query is a thread held for as long as it runs. The test runs
//! the app and a ticker on one thread; the ticker keeps ticking only if the query runs elsewhere.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ex_37_db_diesel::{builder, config};
use ex_support::{get, start};
use ulo_config::{Config, Env, Values};

/// Rows the recursive query counts: enough for SQLite to hold a thread for a few hundred
/// milliseconds on a development machine, which a blocked ticker could not hide.
const ROWS: i64 = 8_000_000;
const TICK: Duration = Duration::from_millis(5);
/// The query must take at least this long, or the test proves nothing about a blocked thread.
const SLOW: Duration = Duration::from_millis(150);

fn settings() -> Config {
    config(Env::from_map::<&str, &str>([]), Values::new().set("database.max_size", 2))
}

#[tokio::test]
async fn a_query_answers_through_the_injected_pool() {
    let server = start(builder(settings())).await;
    let reply = get(server.addr, "/count/1000").await;
    assert_eq!((reply.status, reply.body.as_str()), (200, "1000"));
    server.stop().await.unwrap();
}

/// `#[tokio::test]` is a one-thread runtime, so the server, the pool's tasks and the ticker all
/// share a thread. The ticker's longest gap must be a fraction of the query's duration; a query
/// polled on that thread would make the gap the whole duration.
#[tokio::test]
async fn a_slow_query_does_not_stop_a_ticker_on_the_thread_the_app_runs_on() {
    let server = start(builder(settings())).await;
    assert_eq!(get(server.addr, "/count/1").await.body, "1", "the first query opens the connection, which is not what is measured");

    let stop = Arc::new(AtomicBool::new(false));
    let ticker = tokio::spawn({
        let stop = Arc::clone(&stop);
        async move {
            let (mut longest, mut ticks, mut last) = (Duration::ZERO, 0u32, Instant::now());
            while !stop.load(Ordering::Acquire) {
                tokio::time::sleep(TICK).await;
                let now = Instant::now();
                longest = longest.max(now - last);
                last = now;
                ticks += 1;
            }
            (longest, ticks)
        }
    });

    let began = Instant::now();
    let reply = get(server.addr, &format!("/count/{ROWS}")).await;
    let took = began.elapsed();
    stop.store(true, Ordering::Release);
    let (longest, ticks) = tokio::time::timeout(Duration::from_secs(10), ticker).await.expect("the ticker ended").unwrap();

    assert_eq!((reply.status, reply.body.as_str()), (200, ROWS.to_string().as_str()));
    if std::env::var_os("EX_REPORT_TICKS").is_some() {
        eprintln!("query {took:?}, longest tick gap {longest:?}, {ticks} ticks");
    }
    assert!(took >= SLOW, "the query took {took:?}: too quick to show a blocked thread; raise ROWS");
    assert!(longest < took / 4, "the thread went {longest:?} without polling the ticker during a {took:?} query ({ticks} ticks)");
    server.stop().await.unwrap();
}
