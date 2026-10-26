//! The RPC conformance suite over the MQTT v5 link, each scenario against a Mosquitto broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts. Mosquitto
//! 2.x supports MQTT v5 shared subscriptions; the image's `/mosquitto-no-auth.conf` listens on
//! every interface and allows anonymous clients, which the default configuration does not.
//!
//! Beside the suite, ten tests drive the link through `Link` directly, for orderings inside it
//! that no scenario reaches on its own, for the flow-control window it announces, for how long
//! its connection outlives `close` and for what it writes around its DISCONNECT. Three more
//! drop the link without `close` and count, through a relay, the connections Mosquitto still
//! holds: each side's link, and a server's link holding a call nobody answers.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{FutureExt, StreamExt};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::{BoundAddr, Runtime};
use ulo_rpc::__private::ClientProbe;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::link::Inbound;
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern};
use ulo_rpc_mqtt::Mqtt;
use ulo_transport::{Detail, ErrorKind, SHUTDOWN_MESSAGE};

/// The runtime a client hands its link: the test's tokio runtime, which a tokio link ignores.
fn client_runtime() -> Arc<dyn Runtime> {
    Arc::new(ulo_runtime_tokio::Tokio::current())
}

const PORT: u16 = 1883;

struct Mosquitto {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for Mosquitto {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = Mqtt;

    fn probe(link: &Mqtt) -> Option<&ClientProbe> {
        Some(link.probe())
    }

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the Mosquitto container", || async {
            let container = GenericImage::new("eclipse-mosquitto", "2.0.18")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stderr(" running"))
                .with_cmd(["mosquitto", "-c", "/mosquitto-no-auth.conf"])
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the Mosquitto container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the MQTT port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        Mosquitto { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Mqtt {
        Mqtt::url(format!("mqtt://{}", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Mqtt {
        Mqtt::url(format!("mqtt://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(Mosquitto; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});

/// How long a test waits for a frame the server's link should deliver.
const PATIENCE: Duration = Duration::from_secs(5);

async fn next_frame(inbound: &mut Inbound) -> Option<Frame> {
    tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().map(|delivery| delivery.frame)
}

/// A streamed reply longer than the server's 65,536-request channel arrives whole and in order
/// though its frames are sent back to back on one thread, as the core sends a stream that is
/// always ready: a frame finding the channel full waits for room rather than ending the call.
#[tokio::test]
async fn a_streamed_reply_longer_than_the_channel_arrives_whole() {
    const LONG: &str = "link.long";
    const ITEMS: u32 = 70_000;
    let broker = Mosquitto::start().await;
    let server = broker.link().group("long_stream");
    let mut inbound = server.listen(&[Pattern::from(LONG)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;

    let request = Frame::Req { id: 1, pattern: LONG.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(LONG), request, None).await.expect("the request is published");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().expect("the request reaches the server");
    let (Frame::Req { id, .. }, Some(path)) = (delivery.frame, delivery.reply) else { panic!("the server received something other than a request") };
    let sending = tokio::spawn(async move {
        for n in 0..ITEMS {
            path.send(Frame::Item { id, data: Data::new(Bytes::from(n.to_string())) }).await.unwrap_or_else(|error| panic!("item {n} was not sent: {error}"));
        }
        path.send(Frame::End { id }).await.expect("the end is sent");
    });

    let mut received = 0u32;
    loop {
        match tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten() {
            Some(Frame::Item { id: 1, data }) => {
                assert_eq!(data.as_bytes(), received.to_string().as_bytes(), "item {received} arrived out of place");
                received += 1;
            }
            Some(Frame::End { id: 1 }) => break,
            other => panic!("after {received} of {ITEMS} items the caller read {other:?}"),
        }
    }
    assert_eq!(received, ITEMS, "the items the caller read before the end");
    sending.await.expect("the sends end");
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// I1: a streamed reply whose next item waits on a full request channel when the server's `close`
/// runs. The item never reaches the channel; the caller reads every item queued before the close,
/// then exactly one terminal frame, the `unavailable` with the shutdown phrase, which waited for
/// room itself; and the broker, reached through a relay that parses what the server writes, reads
/// a DISCONNECT and no PUBLISH after it. The relay stops reading what the server writes until
/// `close` has run for longer than `CUT_GRACE`, so the broker acknowledges nothing and the channel
/// fills, and the connection is still there when it reads again, the grace running from `close`'s
/// end.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_waiting_on_a_full_channel_when_close_runs_ends_with_close_s_one_answer() {
    const BLOCKED: &str = "link.blocked";
    let broker = Mosquitto::start().await;
    let relay = Parsing::start(broker.server).await;
    let server = Arc::new(Mqtt::url(format!("mqtt://{}", relay.addr)).group("blocked_stream"));
    let mut inbound = server.listen(&[Pattern::from(BLOCKED)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;
    let request = Frame::Req { id: 1, pattern: BLOCKED.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(BLOCKED), request, None).await.expect("the request is published");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().expect("the request reaches the server");
    let (Frame::Req { id, .. }, Some(path)) = (delivery.frame, delivery.reply) else { panic!("the server received something other than a request") };

    relay.pause();
    let sent = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&sent);
    let sending = tokio::spawn(async move {
        loop {
            let n = counted.load(Ordering::SeqCst);
            if let Err(error) = path.send(Frame::Item { id, data: Data::new(Bytes::from(n.to_string())) }).await {
                return error.to_string();
            }
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let mut last = u64::MAX;
    let blocked = std::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = sent.load(Ordering::SeqCst);
        if now == last {
            break;
        }
        last = now;
        assert!(blocked.elapsed() < Duration::from_secs(60), "the stream did not block within 60 s, {now} items sent");
    }
    assert!(last >= 65_536, "the stream blocked after {last} items, short of the channel's capacity");

    let closing = tokio::spawn({
        let server = Arc::clone(&server);
        async move { server.close().await.map_err(|error| error.to_string()) }
    });
    tokio::time::sleep(CUT_GRACE + Duration::from_millis(500)).await;
    assert!(!closing.is_finished(), "the close ended though its answer had no room");
    relay.resume();

    let mut received = 0u64;
    let terminal = loop {
        match tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten() {
            Some(Frame::Item { id: 1, data }) => {
                assert_eq!(data.as_bytes(), received.to_string().as_bytes(), "item {received} arrived out of place");
                received += 1;
            }
            other => break other,
        }
    };
    let unavailable = |frame: &Frame| matches!(frame, Frame::Err { id: 1, error } if error.kind == ErrorKind::Unavailable && error.message == SHUTDOWN_MESSAGE && error.details.is_empty());
    assert!(terminal.as_ref().is_some_and(unavailable), "after {received} items the caller read {terminal:?}, not the close's `unavailable`");
    assert_eq!(received, last, "the items the caller read, against those queued before the close");
    let after = tokio::time::timeout(Duration::from_secs(1), replies.next()).await.ok().flatten();
    assert_eq!(after, None, "the caller read a frame after the terminal one");
    tokio::time::timeout(Duration::from_secs(10), closing).await.expect("the close did not end once the broker read again").expect("the close's task").expect("the close");
    let refused = tokio::time::timeout(PATIENCE, sending).await.expect("the waiting send did not end").expect("the sending task");
    assert_eq!(refused, "the MQTT link has closed; the reply was not sent", "how the item waiting at the close ended");

    let written = relay.written();
    let disconnect = written.iter().position(|&kind| kind == DISCONNECT).unwrap_or_else(|| panic!("the broker read no DISCONNECT: {:?}", tally(&written)));
    assert!(!written[disconnect..].contains(&PUBLISH), "the broker read a PUBLISH after the DISCONNECT: {:?}", tally(&written[disconnect..]));
    assert_eq!(written.iter().filter(|&&kind| kind == PUBLISH).count() as u64, last + 1, "the PUBLISHes the broker read: every item and the one answer");
    client.close().await.expect("the client's link closes");
}

/// A server's link polled past its DISCONNECT reads what the broker sends until it closes the
/// connection, and acknowledges none of it: MQTT allows no packet after the DISCONNECT. The relay
/// holds the DISCONNECT from the broker while requests are published to the server's filter, so
/// the broker delivers them; the broker then reads the DISCONNECT and nothing after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_acknowledges_nothing_after_its_disconnect() {
    const LATE: &str = "link.late";
    let broker = Mosquitto::start().await;
    let relay = Parsing::start(broker.server).await;
    let server = Mqtt::url(format!("mqtt://{}", relay.addr)).group("after_disconnect");
    let _inbound = server.listen(&[Pattern::from(LATE)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    relay.pause();
    server.close().await.expect("the server's link closes");
    for id in 1..=3 {
        let request = Frame::Req { id, pattern: LATE.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
        (outbound.send)(Pattern::from(LATE), request, None).await.expect("the request is published");
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    relay.resume();
    let resumed = std::time::Instant::now();
    let written = loop {
        let written = relay.written();
        if let Some(disconnect) = written.iter().position(|&kind| kind == DISCONNECT) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            break relay.written().split_off(disconnect + 1);
        }
        assert!(resumed.elapsed() < PATIENCE, "the broker read no DISCONNECT: {:?}", tally(&written));
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(tally(&written), Vec::new(), "the packets the server wrote after its DISCONNECT");
    client.close().await.expect("the client's link closes");
}

/// The MQTT control packet types the parsing relay names.
const PUBLISH: u8 = 3;
const DISCONNECT: u8 = 14;

/// Each packet type and how many times it occurs, in order of first occurrence.
fn tally(kinds: &[u8]) -> Vec<(u8, usize)> {
    let mut counts: Vec<(u8, usize)> = Vec::new();
    for &kind in kinds {
        match counts.iter_mut().find(|(seen, _)| *seen == kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((kind, 1)),
        }
    }
    counts
}

/// A TCP relay to `target` for one connection that records the type of every MQTT packet the
/// inner side writes, as the broker reads them, and that can stop forwarding what the inner side
/// writes until resumed, reading nothing more meanwhile.
struct Parsing {
    addr: SocketAddr,
    paused: tokio::sync::watch::Sender<bool>,
    kinds: Arc<Mutex<Vec<u8>>>,
}

impl Parsing {
    async fn start(target: SocketAddr) -> Parsing {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("a loopback port");
        let addr = listener.local_addr().expect("the relay's address");
        let (paused, mut gate) = tokio::sync::watch::channel(false);
        let kinds: Arc<Mutex<Vec<u8>>> = Arc::default();
        let recorded = Arc::clone(&kinds);
        tokio::spawn(async move {
            let Ok((inner, _)) = listener.accept().await else { return };
            let Ok(outer) = tokio::net::TcpStream::connect(target).await else { return };
            let (mut inner_read, mut inner_write) = inner.into_split();
            let (mut outer_read, mut outer_write) = outer.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut outer_read, &mut inner_write).await;
            });
            let mut buffer = vec![0u8; 16 * 1024];
            let mut pending: Vec<u8> = Vec::new();
            loop {
                if gate.wait_for(|paused| !*paused).await.is_err() {
                    return;
                }
                let read = match inner_read.read(&mut buffer).await {
                    Ok(0) | Err(_) => {
                        let _ = outer_write.shutdown().await;
                        return;
                    }
                    Ok(read) => read,
                };
                // A read under way when the relay was paused is held until it resumes.
                if gate.wait_for(|paused| !*paused).await.is_err() {
                    return;
                }
                pending.extend_from_slice(&buffer[..read]);
                while let Some(length) = packet_length(&pending) {
                    recorded.lock().unwrap_or_else(PoisonError::into_inner).push(pending[0] >> 4);
                    pending.drain(..length);
                }
                if outer_write.write_all(&buffer[..read]).await.is_err() {
                    return;
                }
            }
        });
        Parsing { addr, paused, kinds }
    }

    fn pause(&self) {
        self.paused.send_replace(true);
    }

    fn resume(&self) {
        self.paused.send_replace(false);
    }

    /// The type of every packet the inner side wrote that the relay has read, in order.
    fn written(&self) -> Vec<u8> {
        self.kinds.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The length of the whole MQTT packet at the front of `bytes`, once `bytes` holds all of it: the
/// fixed header's type byte, its variable-length Remaining Length, then that many bytes.
fn packet_length(bytes: &[u8]) -> Option<usize> {
    let mut remaining = 0usize;
    for (index, &byte) in bytes.iter().enumerate().skip(1).take(4) {
        remaining |= usize::from(byte & 0x7f) << (7 * (index - 1));
        if byte & 0x80 == 0 {
            let length = index + 1 + remaining;
            return (bytes.len() >= length).then_some(length);
        }
    }
    None
}

/// The client side's `close` writes what was queued before it. rumqttc's `publish` only queues a
/// request for the event loop, and on a current-thread runtime nothing between the queueing and
/// `close` lets the loop run, so a `close` that ended the loop without awaiting it would lose the
/// event every time.
#[tokio::test]
async fn client_close_writes_what_was_queued_before_it() {
    const EVENT: &str = "link.event";
    let broker = Mosquitto::start().await;
    let server = broker.link().group("client_close");
    let mut inbound = server.listen(&[Pattern::from(EVENT)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");

    let event = Frame::Evt { pattern: EVENT.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"7")) };
    let mut send = (outbound.send)(Pattern::from(EVENT), event, None);
    // One poll queues the publish; the rest of the send waits for the event loop to write it.
    assert!((&mut send).now_or_never().is_none(), "the event loop wrote the publish before it could run");
    client.close().await.expect("the client's link closes");
    drop(send);

    match next_frame(&mut inbound).await {
        Some(Frame::Evt { pattern, data, .. }) => {
            assert_eq!(pattern, EVENT);
            assert_eq!(data.as_bytes(), b"7");
        }
        other => panic!("the event queued before the client's close did not reach the server, got: {other:?}"),
    }
    server.close().await.expect("the server's link closes");
}

/// A client side's `close` cut in its first poll still writes its DISCONNECT: it is queued before
/// the event loop's turn on the calling thread, so the turn writes it. On a current-thread runtime
/// the loop runs only in that turn, and the cut aborts it.
#[tokio::test]
async fn a_client_close_cut_in_its_first_poll_writes_its_disconnect() {
    let broker = Mosquitto::start().await;
    let relay = Parsing::start(broker.server).await;
    let client = Mqtt::url(format!("mqtt://{}", relay.addr));
    let _outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    assert!(client.close().now_or_never().is_none(), "the client's close ended in its first poll");
    let wrote = std::time::Instant::now();
    while !relay.written().contains(&DISCONNECT) {
        assert!(wrote.elapsed() < PATIENCE, "the broker read no DISCONNECT from a close cut in its first poll: {:?}", tally(&relay.written()));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A draining server delivers the `cancel` that releases the last call it holds before its
/// inbound stream ends. The link releases a call on its `cancel` before delivering the frame, and
/// the drain ends the inbound stream once no call is held, so the two have to be ordered.
#[tokio::test(flavor = "multi_thread")]
async fn drain_delivers_the_cancel_that_releases_the_last_call() {
    const HELD: &str = "link.held";
    let broker = Mosquitto::start().await;
    let server = broker.link().group("drain_cancel");
    let mut inbound = server.listen(&[Pattern::from(HELD)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");

    let request = Frame::Req { id: 1, pattern: HELD.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(HELD), request, None).await.expect("the request is published");
    let held = match next_frame(&mut inbound).await {
        Some(Frame::Req { id, .. }) => id,
        other => panic!("the request did not reach the server, got: {other:?}"),
    };

    server.drain().await;
    (outbound.send)(Pattern::from(HELD), Frame::Cancel { id: 1 }, None).await.expect("the cancel is published");
    assert_eq!(
        next_frame(&mut inbound).await,
        Some(Frame::Cancel { id: held }),
        "the cancel of the call the draining server held was not delivered",
    );
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// A server's drain returns only once the broker has stopped routing to it: a request published
/// after it returns finds no subscriber, and the broker's PUBACK 0x10 reaches the caller as
/// `unavailable` with `reason: "no_destination"`. A drain returning while its UNSUBSCRIBE waited
/// to be written or processed would leave the request routed to the draining server.
#[tokio::test(flavor = "multi_thread")]
async fn drain_returns_once_the_broker_stops_routing_to_the_server() {
    const DRAINED: &str = "link.drained";
    let broker = Mosquitto::start().await;
    let server = broker.link().group("drain_confirmed");
    let mut inbound = server.listen(&[Pattern::from(DRAINED)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;

    tokio::time::timeout(PATIENCE, server.drain()).await.expect("the drain returns once the broker has confirmed it");
    let request = Frame::Req { id: 1, pattern: DRAINED.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(DRAINED), request, None).await.expect("the request is published");

    let reply = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
    let no_destination = |error: &ulo_rpc::ErrorBody| {
        error.kind == ErrorKind::Unavailable
            && error.details.iter().any(|detail| matches!(detail, Detail::ErrorInfo { reason, .. } if reason == "no_destination"))
    };
    assert!(
        matches!(&reply, Some(Frame::Err { id: 1, error }) if no_destination(error)),
        "a request published once the drain returned was not answered `no_destination`, got: {reply:?}",
    );
    assert!(inbound.next().now_or_never().is_none_or(|delivery| delivery.is_none()), "the drained server received the request");
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// A drain dropped before the broker confirmed its UNSUBSCRIBEs, as the core drops it at the
/// drain's deadline, logs the filters the broker had not confirmed. On a current-thread runtime
/// the event loop cannot run between the drain's first poll and its drop, so nothing is confirmed.
#[tokio::test]
async fn a_drain_dropped_before_the_broker_confirms_logs_the_filters() {
    const UNCONFIRMED: &str = "link.unconfirmed";
    capture();
    let broker = Mosquitto::start().await;
    let server = broker.link().group("drain_unconfirmed");
    let _inbound = server.listen(&[Pattern::from(UNCONFIRMED)]).await.expect("the server's link listens");

    assert!(server.drain().now_or_never().is_none(), "the drain returned before the event loop could write its UNSUBSCRIBE");

    let warnings = capture().warnings(thread::current().id());
    let filter = format!("$share/drain_unconfirmed/{UNCONFIRMED}");
    assert!(
        warnings.iter().any(|line| {
            line.starts_with("the drain's deadline passed before the MQTT broker confirmed the unsubscribe") && line.contains(&filter)
        }),
        "no `warn` naming the unconfirmed filter `{filter}`: {warnings:?}",
    );
    server.close().await.expect("the server's link closes");
}

/// The broker puts every request routed to a server instance in flight at once, holding none back
/// for flow control: the link announces a Receive Maximum of 65,535 in its CONNECT, and Mosquitto
/// otherwise applies its own `max_inflight_messages`, 20 unset. The server reaches the broker
/// through a relay that, once armed, forwards nothing the server writes, so no PUBACK reaches the
/// broker and every request it sends stays unacknowledged.
#[tokio::test(flavor = "multi_thread")]
async fn the_broker_holds_back_no_request_for_flow_control() {
    const FLOODED: &str = "link.flooded";
    const SENT: usize = 50;
    let broker = Mosquitto::start().await;
    let relay = Muting::start(broker.server).await;
    let server = Mqtt::url(format!("mqtt://{}", relay.addr)).group("flow_control");
    let mut inbound = server.listen(&[Pattern::from(FLOODED)]).await.expect("the server's link listens");
    relay.mute();
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");

    for id in 1..=SENT as u64 {
        let request = Frame::Req { id, pattern: FLOODED.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
        (outbound.send)(Pattern::from(FLOODED), request, None).await.expect("the request is published");
    }
    let mut received = 0;
    while received < SENT {
        match next_frame(&mut inbound).await {
            Some(Frame::Req { .. }) => received += 1,
            other => panic!("{received} of {SENT} requests reached the server unacknowledged; then: {other:?}"),
        }
    }
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// How long the server's link keeps its connection after `close` to read until the broker
/// closes it, transports DESIGN §5.2's `CUT_GRACE`.
const CUT_GRACE: Duration = Duration::from_secs(2);

/// A server's link keeps its connection past `close` only until the broker closes it or for
/// `CUT_GRACE`: through a relay that forwards nothing the server writes once muted, the broker
/// never reads the DISCONNECT and never closes the connection, and the link drops it at the grace,
/// not at its keep-alive's missed PINGRESP ten seconds on.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_s_connection_ends_at_the_grace_when_the_broker_does_not_close_it() {
    let broker = Mosquitto::start().await;
    let relay = Muting::start(broker.server).await;
    let server = Mqtt::url(format!("mqtt://{}", relay.addr)).group("grace");
    let _inbound = server.listen(&[Pattern::from("link.grace")]).await.expect("the server's link listens");
    relay.mute();
    server.close().await.expect("the server's link closes");
    let closed = std::time::Instant::now();
    let ended = loop {
        if let Some(ended) = relay.ended() {
            break ended;
        }
        assert!(closed.elapsed() < Duration::from_secs(10), "the server's link still held its connection 10 s after `close`");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let kept = ended.saturating_duration_since(closed);
    assert!(
        kept >= CUT_GRACE - Duration::from_millis(100) && kept < CUT_GRACE + Duration::from_secs(1),
        "the server's link dropped its connection {kept:?} after `close`, where the grace is {CUT_GRACE:?}"
    );
}

/// A TCP relay to `target` for one connection that, once muted, forwards what the target sends
/// and nothing the other side writes.
struct Muting {
    addr: SocketAddr,
    muted: Arc<std::sync::atomic::AtomicBool>,
    /// When the other side's writing half ended.
    ended: Arc<Mutex<Option<std::time::Instant>>>,
}

impl Muting {
    async fn start(target: SocketAddr) -> Muting {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("a loopback port");
        let addr = listener.local_addr().expect("the relay's address");
        let muted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let outward = Arc::clone(&muted);
        let ended: Arc<Mutex<Option<std::time::Instant>>> = Arc::default();
        let recorded = Arc::clone(&ended);
        tokio::spawn(async move {
            let Ok((inner, _)) = listener.accept().await else { return };
            let Ok(outer) = tokio::net::TcpStream::connect(target).await else { return };
            let (mut inner_read, mut inner_write) = inner.into_split();
            let (mut outer_read, mut outer_write) = outer.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut outer_read, &mut inner_write).await;
            });
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                let read = inner_read.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    *recorded.lock().unwrap_or_else(PoisonError::into_inner) = Some(std::time::Instant::now());
                    return;
                }
                if !outward.load(Ordering::Acquire) && outer_write.write_all(&buffer[..read]).await.is_err() {
                    return;
                }
            }
        });
        Muting { addr, muted, ended }
    }

    fn ended(&self) -> Option<std::time::Instant> {
        *self.ended.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn mute(&self) {
        self.muted.store(true, Ordering::Release);
    }
}

/// The process's subscriber, installed by the first test that reads it. A thread-local one would
/// not do: tracing caches whether a callsite is enabled from the first thread to reach it, and a
/// test without the subscriber reaching the `warn` first would disable it for the one with it.
fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone()).expect("no other subscriber is installed");
        capture
    })
}

/// Every `warn` and `error` event, as its message followed by `name=value` for each other field,
/// with the thread it was emitted on.
#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<(ThreadId, String)>>>,
    next_span: Arc<AtomicU64>,
}

impl Capture {
    fn warnings(&self, on: ThreadId) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().filter(|(thread, _)| *thread == on).map(|(_, line)| line.clone()).collect()
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_span() || *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &TraceEvent<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        let line = format!("{}{}", line.message, line.fields);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push((thread::current().id(), line));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

/// How long a dropped link's connections have to close at the broker.
const RELEASE: Duration = Duration::from_secs(10);

/// Waits until `relay` carries no connection, the broker having closed each of its ends in turn,
/// failing the test with `what` and the count left after [`RELEASE`].
async fn released(relay: &Relay, what: &str) {
    let dropped = std::time::Instant::now();
    loop {
        let open = relay.open().await;
        if open == 0 {
            return;
        }
        assert!(dropped.elapsed() < RELEASE, "{what}: {open} connections still open at the broker {RELEASE:?} after the link's drop");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// F428: a server's link dropped without `close`, as it is when its app drops before the close
/// step, closes its connection: the event loop owns it, and a dropped `JoinHandle` leaves the loop
/// running, subscribed and reconnecting. The link reaches Mosquitto through a relay of its own,
/// which counts the connections the broker still holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_without_close_closes_its_connection() {
    let broker = Mosquitto::start().await;
    let relay = Relay::start(broker.server).await;
    let server = Mqtt::url(format!("mqtt://{}", relay.addr())).group("dropped");
    let inbound = server.listen(&[Pattern::from("link/dropped")]).await.expect("the server's link listens");
    assert!(relay.open().await > 0, "the listening server's link holds no connection through its relay");
    drop(inbound);
    drop(server);
    released(&relay, "the server's link dropped without `close`").await;
}

/// F428: a client's link dropped without `close`, with the outbound it answered, as a dropped
/// `RpcClient` whose spawned close never ran drops them, closes its connection: its event loop
/// ends only at `close` or when the connection fails.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_link_dropped_without_close_closes_its_connection() {
    let broker = Mosquitto::start().await;
    let client = broker.client_link(&[]);
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    assert!(broker.relay.open().await > 0, "the connected client's link holds no connection through its relay");
    drop(outbound);
    drop(client);
    released(&broker.relay, "the client's link dropped without `close`").await;
}

/// F429: a server's link dropped holding a call whose delivery nobody answers closes its
/// connection, undrained and drained. A held call's reply path holds the event loop's request
/// channel and the table the call is held in, and the table holds the path; the event loop owns
/// the connection and the link's drop aborts it, so the connection closes whatever the table
/// holds, and the link's unit tests check that the table goes too. The request comes from a
/// client reaching the broker through the suite's relay, so the server's relay counts the
/// server's connection alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_holding_a_call_closes_its_connection() {
    let broker = Mosquitto::start().await;
    let client = broker.client_link(&[]);
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    for (id, drained) in [(1, false), (2, true)] {
        let pattern = if drained { "link/held/drained" } else { "link/held" };
        let relay = Relay::start(broker.server).await;
        let server = Mqtt::url(format!("mqtt://{}", relay.addr())).group(if drained { "held_drained" } else { "held" });
        let mut inbound = server.listen(&[Pattern::from(pattern)]).await.expect("the server's link listens");
        let request = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
        (outbound.send)(Pattern::from(pattern), request, None).await.expect("the request is published");
        let delivery = tokio::time::timeout(PATIENCE, inbound.next())
            .await
            .unwrap_or_else(|_| panic!("the request did not reach the server within {PATIENCE:?}"))
            .expect("the server's inbound stream ended");
        assert!(matches!(delivery.frame, Frame::Req { .. }), "the server received something other than the request: {:?}", delivery.frame);
        if drained {
            server.drain().await;
        }
        drop(delivery);
        drop(inbound);
        drop(server);
        let what = if drained { "the drained server's link dropped holding a call" } else { "the server's link dropped holding a call" };
        released(&relay, what).await;
    }
    client.close().await.expect("the client's link closes");
}
