//! Scenarios: sse.

use crate::wire::{Exchange, same_as_reference, start};
use crate::{Host, Mode};

/// An SSE stream whose item fails, written as an `error` event that ends it.
pub async fn error_event<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/sse")).await;
    assert_eq!(reply.status, 200);
    assert!(reply.header("content-type").is_some_and(|value| value.starts_with("text/event-stream")));
    let text = reply.text();
    assert!(text.contains("data: one"), "the first event is missing: {text}");
    assert!(text.contains("event: error"), "the error event is missing: {text}");
    host.stop().await;
}

/// Events reach the client as the handler produces them: the first is read while the handler is
/// still waiting on a gate only this scenario opens, after reading it. A host that holds frames
/// back until the response ends never delivers the first, and the scenario fails when the read
/// runs out of patience, not by hanging.
pub async fn delivered_as_produced<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = host.raw().await;
    let request = format!("GET {} HTTP/1.1\r\nHost: suite\r\nAccept: text/event-stream\r\n\r\n", host.target("/gated"));
    raw.write(request.as_bytes()).await;
    let first = raw
        .read_until(b"data: one\n\n", "the first event, while the handler waits on the gate")
        .await
        .expect("the connection stayed open until the first event");
    assert!(
        !String::from_utf8_lossy(&first).contains("data: two"),
        "the second event arrived before the gate opened"
    );
    host.probe.gate.open();
    let rest = raw.read_until(b"data: two\n\n", "the second event, after the gate opened").await.expect("the second event arrives");
    assert!(String::from_utf8_lossy(&rest).contains("data: two"));
    host.stop().await;
}
