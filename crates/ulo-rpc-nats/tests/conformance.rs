//! The RPC conformance suite over the NATS link, each scenario against a NATS server in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts. Beside it, five
//! tests drive the link through `Link` directly, for orderings of a held call's answers against a
//! `close`, cut short or under way, that no scenario reaches on every run.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{FutureExt, StreamExt};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::BoundAddr;
use ulo_rpc::__private::ClientProbe;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::{CallHeaders, Data, ErrorBody, Frame, Link, Pattern};
use ulo_rpc_nats::Nats;
use ulo_transport::{Details, ErrorKind, SHUTDOWN_MESSAGE};

const PORT: u16 = 4222;

struct NatsServer {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for NatsServer {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = Nats;

    fn probe(link: &Nats) -> Option<&ClientProbe> {
        Some(link.probe())
    }

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the NATS container", || async {
            let container = GenericImage::new("nats", "2.10.14")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the NATS container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the NATS port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        NatsServer { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Nats {
        Nats::url(format!("nats://{}", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Nats {
        Nats::url(format!("nats://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(NatsServer; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});

/// How long the test waits for a frame the client's link should receive.
const PATIENCE: Duration = Duration::from_secs(5);

/// A server's `close` polled once and dropped answers `unavailable` only the calls no terminal
/// frame was queued for, a stream's after its items, and writes nothing more to a call already
/// answered.
#[tokio::test]
async fn a_close_cut_short_answers_only_the_calls_still_unanswered() {
    const HELD: &str = "link.unanswered";
    let broker = NatsServer::start().await;
    let server = broker.link().group("cut_unanswered");
    let mut inbound = server.listen(&[Pattern::from(HELD)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;

    let mut paths = std::collections::HashMap::new();
    for caller in [1u64, 2] {
        let data = Data::new(Bytes::from(caller.to_string()));
        let request = Frame::Req { id: caller, pattern: HELD.to_owned(), headers: CallHeaders::new(), data };
        (outbound.send)(Pattern::from(HELD), request, None).await.expect("the request is published");
    }
    for _ in 0..2 {
        let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().expect("the request reaches the server");
        let (Frame::Req { id, data, .. }, Some(path)) = (delivery.frame, delivery.reply) else { panic!("the server received something other than a request") };
        paths.insert(data.as_bytes().to_vec(), (id, path));
    }
    let (answered, answered_path) = paths.remove(&b"1"[..]).expect("the first call");
    let (streaming, streaming_path) = paths.remove(&b"2"[..]).expect("the second call");
    answered_path.send(Frame::Res { id: answered, data: Data::new(Bytes::from_static(b"7")) }).await.expect("the answer is queued");
    streaming_path.send(Frame::Item { id: streaming, data: Data::new(Bytes::from_static(b"8")) }).await.expect("the item is queued");

    assert!(server.close().now_or_never().is_none(), "the close ended in its first poll");
    drop((answered_path, streaming_path, inbound, server));

    let mut read = Vec::new();
    while let Some(frame) = tokio::time::timeout(Duration::from_millis(500), replies.next()).await.ok().flatten() {
        read.push(frame);
    }
    let unavailable = ErrorBody::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE, Details::new());
    let first: Vec<&Frame> = read.iter().filter(|frame| frame.id() == Some(1)).collect();
    let second: Vec<&Frame> = read.iter().filter(|frame| frame.id() == Some(2)).collect();
    assert_eq!(first, [&Frame::Res { id: 1, data: Data::new(Bytes::from_static(b"7")) }], "what the caller read for the call answered before the close");
    assert_eq!(
        second,
        [&Frame::Item { id: 2, data: Data::new(Bytes::from_static(b"8")) }, &Frame::Err { id: 2, error: unavailable }],
        "what the caller read for the stream the close cut"
    );
    assert_eq!(read.len(), 3, "the caller read frames for no call it made: {read:?}");
    client.close().await.expect("the client's link closes");
}

/// A server's `close` polled once and dropped, as a spent shutdown cap leaves it, queues the
/// `unavailable` with the shutdown phrase for the call it holds, which the caller reads, and the
/// call's own answer, sent after that `close`, is refused rather than queued after it. The test
/// runs on one thread, so the `close` and the late answer cannot interleave otherwise.
#[tokio::test]
async fn a_close_cut_short_still_answers_the_call_it_held() {
    const HELD: &str = "link.held";
    let broker = NatsServer::start().await;
    let server = broker.link().group("cut_close");
    let mut inbound = server.listen(&[Pattern::from(HELD)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;

    let request = Frame::Req { id: 1, pattern: HELD.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(HELD), request, None).await.expect("the request is published");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().expect("the request reaches the server");
    let (Frame::Req { id, .. }, Some(path)) = (delivery.frame, delivery.reply) else { panic!("the server received something other than a request") };

    assert!(server.close().now_or_never().is_none(), "the close ended in its first poll");
    let late = path.send(Frame::Res { id, data: Data::new(Bytes::from_static(b"7")) }).await;
    drop((path, inbound, server));

    let answer = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
    assert_eq!(
        answer,
        Some(Frame::Err { id: 1, error: ErrorBody::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE, Details::new()) }),
        "what the caller read for the call the cut close held"
    );
    assert_eq!(
        late.expect_err("the call's answer sent after the close was taken").to_string(),
        "the NATS link has closed; the reply was not sent"
    );
    let after = tokio::time::timeout(Duration::from_millis(500), replies.next()).await.ok().flatten();
    assert_eq!(after, None, "the caller read a second answer");
    client.close().await.expect("the client's link closes");
}

/// A streamed reply longer than async-nats's 2,048-command channel arrives whole and in order
/// though its frames are sent back to back on one thread, as the core sends a stream that is
/// always ready: a frame finding the channel full waits for room rather than ending the call. The
/// sends yield the thread as tokio's cooperative budget runs out, not only once the channel is
/// full: a sender holding it for 2,048 frames at a time starves the caller's link, on the same
/// thread here, until async-nats drops what its subscription has no room for.
#[tokio::test]
async fn a_streamed_reply_longer_than_the_channel_arrives_whole() {
    const LONG: &str = "link.long";
    const ITEMS: u32 = 100_000;
    let broker = NatsServer::start().await;
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

/// I1: a streamed reply whose next item waits on async-nats's full command channel when the
/// server's `close` runs. The item never reaches the channel; the caller reads every item queued
/// before the close, then exactly one terminal frame, the `unavailable` with the shutdown phrase,
/// which waited for room itself. The server reaches NATS through a relay that stops reading what
/// the server writes until `close` is under way, so the channel fills.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_waiting_on_a_full_channel_when_close_runs_ends_with_close_s_one_answer() {
    const BLOCKED: &str = "link.blocked";
    let broker = NatsServer::start().await;
    let relay = Pausing::start(broker.server).await;
    let server = Arc::new(Nats::url(format!("nats://{}", relay.addr)).group("blocked_stream"));
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
    assert!(last >= 2_048, "the stream blocked after {last} items, short of async-nats's channel");

    let closing = tokio::spawn({
        let server = Arc::clone(&server);
        async move { server.close().await.map_err(|error| error.to_string()) }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
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
    assert_eq!(
        terminal,
        Some(Frame::Err { id: 1, error: ErrorBody::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE, Details::new()) }),
        "the terminal frame the caller read after {received} items"
    );
    assert_eq!(received, last, "the items the caller read, against those queued before the close");
    let after = tokio::time::timeout(Duration::from_secs(1), replies.next()).await.ok().flatten();
    assert_eq!(after, None, "the caller read a frame after the terminal one");
    tokio::time::timeout(Duration::from_secs(10), closing).await.expect("the close did not end once the server read again").expect("the close's task").expect("the close");
    let refused = tokio::time::timeout(PATIENCE, sending).await.expect("the waiting send did not end").expect("the sending task");
    assert_eq!(refused, "the NATS link has closed; the reply was not sent", "how the item waiting at the close ended");
    client.close().await.expect("the client's link closes");
}

/// I1: a stream's end that waited on async-nats's full command channel ends its call once queued,
/// so a `close` afterwards writes the call nothing more: the caller reads the items and the end,
/// and no `unavailable` after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_frame_queued_after_waiting_ends_its_call() {
    const ENDED: &str = "link.ended";
    let broker = NatsServer::start().await;
    let relay = Pausing::start(broker.server).await;
    let server = Nats::url(format!("nats://{}", relay.addr)).group("waited_end");
    let mut inbound = server.listen(&[Pattern::from(ENDED)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;
    let request = Frame::Req { id: 1, pattern: ENDED.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(ENDED), request, None).await.expect("the request is published");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().expect("the request reaches the server");
    let (Frame::Req { id, .. }, Some(path)) = (delivery.frame, delivery.reply) else { panic!("the server received something other than a request") };

    relay.pause();
    let mut sent = 0u64;
    let blocked = loop {
        let mut send = path.send(Frame::Item { id, data: Data::new(Bytes::from(sent.to_string())) });
        if tokio::time::timeout(Duration::from_millis(500), &mut send).await.is_err() {
            break send;
        }
        sent += 1;
    };
    let mut ending = path.send(Frame::End { id });
    assert!((&mut ending).now_or_never().is_none(), "the end found room in a full channel");
    relay.resume();
    blocked.await.expect("the item that waited is queued");
    ending.await.expect("the end that waited is queued");
    server.close().await.expect("the server's link closes");

    let mut received = 0u64;
    let terminal = loop {
        match tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten() {
            Some(Frame::Item { id: 1, .. }) => received += 1,
            other => break other,
        }
    };
    assert_eq!((received, terminal), (sent + 1, Some(Frame::End { id: 1 })), "the items the caller read, and the frame after them");
    let after = tokio::time::timeout(Duration::from_secs(1), replies.next()).await.ok().flatten();
    assert_eq!(after, None, "the caller read a frame after the stream's end");
    client.close().await.expect("the client's link closes");
}

/// A relay to `target` for one connection that can stop reading what the inner side writes, so
/// the inner side's socket stops taking writes once the buffers between fill.
struct Pausing {
    addr: SocketAddr,
    paused: tokio::sync::watch::Sender<bool>,
}

impl Pausing {
    async fn start(target: SocketAddr) -> Pausing {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("a loopback port");
        let addr = listener.local_addr().expect("the relay's address");
        let (paused, mut gate) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let Ok((inner, _)) = listener.accept().await else { return };
            let Ok(outer) = tokio::net::TcpStream::connect(target).await else { return };
            let (mut inner_read, mut inner_write) = inner.into_split();
            let (mut outer_read, mut outer_write) = outer.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut outer_read, &mut inner_write).await;
            });
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                if gate.wait_for(|paused| !*paused).await.is_err() {
                    return;
                }
                match inner_read.read(&mut buffer).await {
                    Ok(0) | Err(_) => {
                        let _ = outer_write.shutdown().await;
                        return;
                    }
                    Ok(read) => {
                        if outer_write.write_all(&buffer[..read]).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Pausing { addr, paused }
    }

    fn pause(&self) {
        self.paused.send_replace(true);
    }

    fn resume(&self) {
        self.paused.send_replace(false);
    }
}

/// The runtime a client hands its link: the test's tokio runtime, which a tokio link ignores.
fn client_runtime() -> std::sync::Arc<dyn ulo::Runtime> {
    std::sync::Arc::new(ulo_runtime_tokio::Tokio::current())
}
