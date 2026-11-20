//! The framing layer as RFC 6455 states it, read off the wire byte for byte: a Close echoed, the
//! three faults §7.4.1 gives 1002 for, nothing answered after a Close, a message reassembled
//! around a Ping, and nothing written after the server's own Close.
//!
//! Each scenario writes frames the suite's WebSocket client would refuse to send or would answer
//! itself, so it writes and reads bytes on the upgraded stream, and none passes on silence: each
//! first reads the frame or the departure that shows the server acted.

use serde_json::{Value, json};
use ulo_ws::DisconnectReason;

use super::Served;
use crate::Host;
use crate::app::LEAVING;
use crate::client::{RawFrame, client_frame, close_body, shown};

/// §5.5.1: "If an endpoint receives a Close frame and did not previously send a Close frame, the
/// endpoint MUST send a Close frame in response", typically echoing the status code; the server
/// echoes the code and the reason, and then ends the connection.
pub async fn close_echo<H: Host>() {
    let app = Served::<H>::start().await;
    let mut raw = app.raw("/echo").await;
    raw.write(&client_frame(true, RawFrame::CLOSE, &close_body(1000, "bye"), true)).await;
    let frames = raw.frames_to_end("the server's answer to a Close").await;
    let echo = RawFrame { fin: true, opcode: RawFrame::CLOSE, payload: close_body(1000, "bye") };
    assert_eq!(frames, vec![echo], "the frames before the end: {:?}", shown(&frames));
    let ends = app.probe.departures.at_least(app.timer(), 1, "the closed connection's `on_disconnect`").await;
    assert_eq!(ends, vec![("/echo".to_owned(), DisconnectReason::ClientClose { code: 1000, reason: "bye".to_owned() })]);
    app.stop().await;
}

/// §5.5.2: an endpoint answers a Ping with a Pong "unless it already received a Close frame". A
/// Ping sent after the client's Close gets the Close echoed and nothing else.
pub async fn no_pong_after_close<H: Host>() {
    let app = Served::<H>::start().await;
    let mut raw = app.raw("/echo").await;
    let mut written = client_frame(true, RawFrame::CLOSE, &close_body(1000, ""), true);
    written.extend(client_frame(true, RawFrame::PING, b"still there?", true));
    raw.write_unless_ended(&written).await;
    let frames = raw.frames_to_end("the server's answer to a Close and a Ping").await;
    assert_eq!(frames.first().and_then(RawFrame::close_code), Some(1000), "the Close echoed first: {:?}", shown(&frames));
    assert!(frames.iter().all(|frame| frame.opcode != RawFrame::PONG), "a Pong after the Close: {:?}", shown(&frames));
    assert_eq!(frames.len(), 1, "only the echo: {:?}", shown(&frames));
    app.stop().await;
}

/// The server's answer to `fault`: a Close with 1002 and the end of the connection, and the
/// connection's departure as a protocol error.
async fn closes_with_1002<H: Host>(fault: &[u8], what: &str) {
    let app = Served::<H>::start().await;
    let mut raw = app.raw("/echo").await;
    raw.write(fault).await;
    let close = raw.next_frame(&format!("the server's answer to {what}")).await;
    assert_eq!(close.as_ref().and_then(RawFrame::close_code), Some(1002), "{what}: answered {close:?}");
    raw.write_unless_ended(&client_frame(true, RawFrame::CLOSE, &close_body(1002, ""), true)).await;
    let after = raw.frames_to_end(&format!("the end of the connection after {what}")).await;
    assert!(after.is_empty(), "{what}: frames after the Close: {:?}", shown(&after));
    let ends = app.probe.departures.at_least(app.timer(), 1, "the faulted connection's `on_disconnect`").await;
    assert_eq!(ends, vec![("/echo".to_owned(), DisconnectReason::ProtocolError)], "{what}");
    app.stop().await;
}

/// §5.2: "If an unknown opcode is received, the receiving endpoint MUST _Fail the WebSocket
/// Connection_"; 0x3 to 0x7 are reserved for further data frames.
pub async fn reserved_opcode<H: Host>() {
    closes_with_1002::<H>(&client_frame(true, 0x3, b"reserved", true), "a frame with the reserved opcode 0x3").await;
}

/// §5.1: "The server MUST close the connection upon receiving a frame that is not masked", with
/// 1002 (§7.4.1).
pub async fn unmasked_frame<H: Host>() {
    let message = json!({ "event": "echo", "id": 1, "data": "unmasked" }).to_string();
    closes_with_1002::<H>(&client_frame(true, RawFrame::TEXT, message.as_bytes(), false), "an unmasked text frame").await;
}

/// §5.5: "All control frames MUST have a payload length of 125 bytes or less".
pub async fn oversized_ping<H: Host>() {
    closes_with_1002::<H>(&client_frame(true, RawFrame::PING, &[b'p'; 126], true), "a Ping carrying 126 bytes").await;
}

/// §5.4: a control frame "MAY be injected in the middle of a fragmented message". The Ping is
/// answered with its Pong and the message's two fragments are read as one message.
pub async fn fragments_around_a_ping<H: Host>() {
    let app = Served::<H>::start().await;
    let mut raw = app.raw("/echo").await;
    let message = json!({ "event": "echo", "id": 9, "data": "whole" }).to_string();
    let (first, rest) = message.as_bytes().split_at(10);
    let mut written = client_frame(false, RawFrame::TEXT, first, true);
    written.extend(client_frame(true, RawFrame::PING, b"mid", true));
    written.extend(client_frame(true, RawFrame::CONTINUATION, rest, true));
    raw.write(&written).await;

    let mut seen = Vec::new();
    while seen.len() < 2 {
        match raw.next_frame("the Pong and the message's answer").await {
            Some(frame) => seen.push(frame),
            None => panic!("the connection ended after {:?}", shown(&seen)),
        }
    }
    let pong = RawFrame { fin: true, opcode: RawFrame::PONG, payload: b"mid".to_vec() };
    assert!(seen.contains(&pong), "no Pong carrying the Ping's payload: {:?}", shown(&seen));
    let answer = seen.iter().find(|frame| frame.opcode == RawFrame::TEXT).unwrap_or_else(|| panic!("no answer: {:?}", shown(&seen)));
    let answer: Value = serde_json::from_slice(&answer.payload).unwrap_or_else(|error| panic!("the answer is not JSON: {error}"));
    assert_eq!(answer, json!({ "id": 9, "data": "whole" }), "the reassembled message's answer");
    raw.write(&client_frame(true, RawFrame::CLOSE, &close_body(1000, ""), true)).await;
    let rest = raw.frames_to_end("the end of the connection").await;
    assert_eq!(rest.iter().map(|frame| frame.opcode).collect::<Vec<_>>(), [RawFrame::CLOSE], "{:?}", shown(&rest));
    app.stop().await;
}

/// §5.5.1: "After sending a Close frame, the endpoint MUST NOT send any more data frames." A
/// handler that closes its connection, then sends a frame and answers, has neither written.
pub async fn no_data_after_close<H: Host>() {
    let app = Served::<H>::start().await;
    let mut raw = app.raw("/echo").await;
    let message = json!({ "event": "leave", "id": 1 }).to_string();
    raw.write(&client_frame(true, RawFrame::TEXT, message.as_bytes(), true)).await;
    let close = raw.next_frame("the handler's Close").await;
    let expected = RawFrame { fin: true, opcode: RawFrame::CLOSE, payload: close_body(1000, LEAVING) };
    assert_eq!(close.as_ref(), Some(&expected), "the first frame after the message: {:?}", close.as_ref().map(|frame| shown(std::slice::from_ref(frame))));
    raw.write_unless_ended(&client_frame(true, RawFrame::CLOSE, &close_body(1000, ""), true)).await;
    let after = raw.frames_to_end("the end of the connection after its Close").await;
    assert!(after.is_empty(), "frames after the server's Close: {:?}", shown(&after));
    let ends = app.probe.departures.at_least(app.timer(), 1, "the closed connection's `on_disconnect`").await;
    assert_eq!(ends, vec![("/echo".to_owned(), DisconnectReason::ServerClose { code: 1000 })]);
    app.stop().await;
}
