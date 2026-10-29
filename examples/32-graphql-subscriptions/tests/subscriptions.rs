use std::time::Duration;

use ex_32_graphql_subscriptions::{Ticker, builder};
use ex_support::graphql::{next_data, ready, subscribe};
use ex_support::ws::{close_frame, hang_up, next, send};
use ex_support::{Served, advancing, start_on};
use serde_json::json;
use ulo::{Dep, Signal};
use ulo_conformance_support::ScriptedTimer;

/// Waits until `n` subscriptions listen on the ticker, so a tick published next reaches them.
async fn listening(ticker: &Dep<Ticker>, n: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while ticker.listeners() != n {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the subscription started listening");
}

async fn serving() -> (Served, ScriptedTimer, Dep<Ticker>) {
    let clock = ScriptedTimer::new();
    let server = start_on(builder(), clock.clone()).await;
    let ticker: Dep<Ticker> = server.handle.get().await.expect("the ticker is bound");
    (server, clock, ticker)
}

#[tokio::test]
async fn ticks_arrive_in_the_order_they_are_published_and_complete_ends_the_operation() {
    let (server, _clock, ticker) = serving().await;
    let mut socket = ready(server.addr, "/graphql/ws").await;
    subscribe(&mut socket, "t", "subscription { ticks }").await;
    listening(&ticker, 1).await;

    for tick in [1, 2, 3] {
        ticker.publish(tick);
    }
    for tick in [1, 2, 3] {
        assert_eq!(next_data(&mut socket, "t").await, json!({ "ticks": tick }));
    }

    send(&mut socket, json!({ "type": "complete", "id": "t" })).await;
    listening(&ticker, 0).await;
    hang_up(socket).await;
    server.stop().await.unwrap();
}

#[tokio::test]
async fn two_subscriptions_on_one_connection_each_read_every_tick_under_their_own_id() {
    let (server, _clock, ticker) = serving().await;
    let mut socket = ready(server.addr, "/graphql/ws").await;
    subscribe(&mut socket, "a", "subscription { ticks }").await;
    subscribe(&mut socket, "b", "subscription { ticks }").await;
    listening(&ticker, 2).await;

    ticker.publish(7);
    let mut seen = vec![next(&mut socket).await, next(&mut socket).await];
    seen.sort_by_key(|m| m["id"].as_str().map(str::to_owned));
    let payload = json!({ "data": { "ticks": 7 } });
    assert_eq!(seen, [json!({ "type": "next", "id": "a", "payload": payload }), json!({ "type": "next", "id": "b", "payload": payload })]);

    hang_up(socket).await;
    server.stop().await.unwrap();
}

/// The drain ends a running operation with `complete`, then closes the socket with 1001 and the
/// shutdown phrase.
#[tokio::test]
async fn the_drain_completes_the_operation_and_closes_the_socket_with_1001() {
    let (server, clock, ticker) = serving().await;
    let mut socket = ready(server.addr, "/graphql/ws").await;
    subscribe(&mut socket, "t", "subscription { ticks }").await;
    listening(&ticker, 1).await;
    ticker.publish(1);
    assert_eq!(next_data(&mut socket, "t").await, json!({ "ticks": 1 }));

    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    server.handle.draining().await;

    assert_eq!(next(&mut socket).await, json!({ "type": "complete", "id": "t" }));
    assert_eq!(close_frame(&mut socket).await, Some((1001, "the server is shutting down".to_owned())));
    hang_up(socket).await;

    let report = advancing(&clock, Duration::from_secs(5), closing).await.unwrap().unwrap();
    assert_eq!(report.signal.name(), "deploy");
    assert_eq!((report.abandoned, report.terminal_skipped), (0, 0));
    assert_eq!(clock.elapsed(), Duration::ZERO, "the drain ended with its last connection, not at the window");
}
