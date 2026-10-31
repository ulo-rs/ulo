use std::time::Duration;

use ex_18_http_streaming::{KEEP_ALIVE, Pace, builder};
use ex_support::http::{Call, call, open};
use ex_support::{advancing, start, start_on};
use tokio::time::sleep;
use ulo::{Dep, Signal};
use ulo::testing::ScriptedTimer;

#[tokio::test]
async fn events_reach_a_connected_client_one_at_a_time_as_they_are_published() {
    let server = start(builder()).await;
    let mut events = open(server.addr, Call::get("/events")).await;
    assert_eq!(events.status, 200);
    assert_eq!(events.header("content-type"), Some("text/event-stream; charset=utf-8"));
    assert_eq!(events.header("cache-control"), Some("no-cache"));

    for (n, text) in ["first", "second"].into_iter().enumerate() {
        assert_eq!(call(server.addr, Call::post(&format!("/events/{text}"))).await.body, "published");
        assert_eq!(events.next_block().await.as_deref(), Some(format!("id: {}\ndata: {text}", n + 1).as_str()));
    }
    drop(events);
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_reconnecting_client_resumes_after_the_id_it_names() {
    let server = start(builder()).await;
    for text in ["a", "b", "c"] {
        call(server.addr, Call::post(&format!("/events/{text}"))).await;
    }

    let mut resumed = open(server.addr, Call::get("/events").header("last-event-id", "2")).await;
    assert_eq!(resumed.next_block().await.as_deref(), Some("id: 3\ndata: c"), "ids 1 and 2 are not sent again");
    call(server.addr, Call::post("/events/d")).await;
    assert_eq!(resumed.next_block().await.as_deref(), Some("id: 4\ndata: d"));

    let mut fresh = open(server.addr, Call::get("/events")).await;
    assert_eq!(fresh.next_block().await.as_deref(), Some("id: 1\ndata: a"), "no Last-Event-ID starts at the beginning");
    drop((resumed, fresh));
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn an_idle_stream_writes_a_keep_alive_comment_when_the_clock_passes_the_period() {
    let clock = ScriptedTimer::new();
    let server = start_on(builder(), clock.clone()).await;
    let mut events = open(server.addr, Call::get("/events")).await;

    // The keep-alive timer is registered once the body is first polled; wait for it, then pass it.
    let armed = async {
        while clock.live(KEEP_ALIVE) == 0 {
            sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), armed).await.expect("the stream armed its keep-alive within ten seconds of real time");
    clock.advance(KEEP_ALIVE);
    assert_eq!(events.next_block().await.as_deref(), Some(": keepalive"));
    drop(events);
    advancing(&clock, KEEP_ALIVE, server.stop()).await.expect("closes clean");
}

#[tokio::test]
async fn the_drain_ends_an_open_stream_cleanly_and_close_reports_nothing_abandoned() {
    let clock = ScriptedTimer::new();
    let server = start_on(builder(), clock.clone()).await;
    call(server.addr, Call::post("/events/seen")).await;
    let mut events = open(server.addr, Call::get("/events")).await;
    assert_eq!(events.next_block().await.as_deref(), Some("id: 1\ndata: seen"));

    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    server.handle.draining().await;
    assert_eq!(events.rest().await, "", "the stream ended with its chunked terminator, no error event");

    let report = advancing(&clock, Duration::from_secs(5), closing).await.unwrap().unwrap();
    assert_eq!((report.signal.name(), report.abandoned), ("deploy", 0));
    assert_eq!(clock.elapsed(), Duration::ZERO, "the drain ended with the stream, not at the window");
}

#[tokio::test]
async fn a_streamed_body_arrives_frame_by_frame_while_the_handler_is_still_writing() {
    let server = start(builder()).await;
    let pace: Dep<Pace> = server.handle.get().await.unwrap();
    let mut export = open(server.addr, Call::get("/export")).await;
    assert_eq!(export.status, 200);
    assert_eq!(export.header("transfer-encoding"), Some("chunked"));
    assert_eq!(export.header("content-length"), None);

    assert_eq!(export.next_block().await.as_deref(), Some("row 1"));
    pace.0.add_permits(1);
    assert_eq!(export.next_block().await.as_deref(), Some("row 2"));
    assert_eq!(pace.0.available_permits(), 0, "row 2 was written only once the stream took the permit");
    pace.0.add_permits(1);
    assert_eq!(export.next_block().await.as_deref(), Some("row 3"));
    assert_eq!(pace.0.available_permits(), 0, "row 3 was written only once the stream took the permit");
    assert_eq!(export.next_block().await, None);
    server.stop().await.expect("closes clean");
}
