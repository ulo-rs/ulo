//! Broadcast across processes through a real Redis: two app instances, each its own process with
//! its own `WsModule` over the Redis adapter, a room with a member connected to each, and a room
//! broadcast from either instance reaching the member on the other. Redis runs in a container.
//!
//! Two further tests hold that `listen` returns only once Redis has confirmed the instance's
//! subscription: a broadcast published straight after `listen`, with no wait, reaches a member of
//! the same process, also when Redis is slow to confirm the subscription.
//!
//! The second instance is this test binary run again as a child process, through the ignored test
//! `peer_instance`, which the parent starts with the Redis URL in `ULO_WS_REDIS_PEER`. The child
//! prints the address its server bound and serves until its stdin closes.

#![cfg(feature = "integration")]

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, Dep, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc_conformance::relay::{reachable, unshadowed};
use ulo_rpc_conformance::{report, startup_failed};
use ulo_ws::{BroadcastError, ConnectCx, ConnectRefused, OnConnect, Payload, Rooms, WsModule};
use ulo_ws_redis::Redis;

/// The environment variable carrying the Redis URL to the child process.
const PEER: &str = "ULO_WS_REDIS_PEER";

/// The line the child prints its server's address on.
const ADDR_LINE: &str = "ulo-ws-redis peer listening on ";

/// The room every connection joins.
const ROOM: &str = "members";

/// How long any one wait lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long the parent waits for the child to start: the child process starts its own runtime, app
/// and Redis subscription.
const BOOT: Duration = Duration::from_secs(60);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[injectable]
struct Members;

#[routes]
#[ulo_ws::gateway(path = "/members", port = own)]
impl Members {
    /// Answered once the connection's connect phase, its join included, has finished.
    #[ulo_ws::message("ready")]
    fn ready(&self) {}

    /// To every member of the room, on every instance.
    #[ulo_ws::message("shout")]
    async fn shout(&self, text: Payload<String>, rooms: Dep<Rooms>) -> Result<(), BroadcastError> {
        rooms.to_room(ROOM).emit("shouted", &text.0).await
    }
}

impl OnConnect for Members {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        cx.conn().join(ROOM).await;
        Ok(())
    }
}

struct Root {
    url: String,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Redis::url(&self.url)));
        m.controller::<Members>();
    }
}

/// One instance serving on an ephemeral port until stopped.
struct Instance {
    addr: SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Instance {
    async fn start(url: &str) -> Instance {
        let app: App<Serving> = App::builder(Root { url: url.to_owned() })
            .runtime(ulo_runtime_tokio::Tokio::current())
            .drain_timeout(Duration::from_secs(2))
            .wire()
            .unwrap_or_else(|error| panic!("the instance did not wire: {}", report(&error)))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the instance did not connect: {}", report(&error)))
            .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the instance did not listen: {}", report(&error)));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Instance { addr, handle, serving }
    }

    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

/// The child process, killed if the test ends without stopping it.
struct Peer {
    child: Child,
    addr: SocketAddr,
}

impl Peer {
    /// This binary run again as `peer_instance`, once it reports its server's address.
    async fn start(url: &str) -> Peer {
        let mut child = Command::new(std::env::current_exe().expect("the test binary's path"))
            .args(["peer_instance", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env(PEER, url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|error| panic!("the peer process did not start: {error}"));
        let stdout = child.stdout.take().expect("the peer's stdout is piped");
        let (found, address) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            let mut found = Some(found);
            for line in lines.by_ref().map_while(Result::ok) {
                // libtest writes `test peer_instance ... ` ahead of it on the same line.
                if let Some((_, addr)) = line.split_once(ADDR_LINE)
                    && let Some(found) = found.take()
                {
                    let _ = found.send(addr.trim().to_owned());
                }
            }
        });
        let peer_addr = match tokio::time::timeout(BOOT, address).await {
            Ok(Ok(addr)) => addr.parse().unwrap_or_else(|error| panic!("the peer printed `{addr}`, not an address: {error}")),
            Ok(Err(_)) => {
                let _ = child.kill();
                panic!("the peer process ended before it reported its address");
            }
            Err(_) => {
                let _ = child.kill();
                panic!("the peer process did not report its address within {BOOT:?}");
            }
        };
        Peer { child, addr: peer_addr }
    }

    /// Closes the child's stdin, on which it stops its app and exits, and waits for it.
    async fn stop(mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                assert!(status.success(), "the peer process exited with {status}");
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the peer process did not exit within {PATIENCE:?} of its stdin closing");
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start_redis(name: &str) -> (ContainerAsync<GenericImage>, String) {
    const PORT: u16 = 6379;
    let (container, addrs) = unshadowed("the Redis container", || async {
        let container = GenericImage::new("redis", "7-alpine")
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .with_container_name(name)
            .start()
            .await
            .unwrap_or_else(|error| startup_failed!("the Redis container did not start: {}", report(&error)));
        let port = container
            .get_host_port_ipv4(PORT)
            .await
            .unwrap_or_else(|error| startup_failed!("the Redis port is not mapped: {}", report(&error)));
        (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
    })
    .await;
    reachable(addrs[0], Duration::from_secs(10)).await;
    (container, format!("redis://{}", addrs[0]))
}

/// A member of the room on the instance at `addr`, its join done.
async fn member(addr: SocketAddr) -> Socket {
    let (mut socket, _) = tokio::time::timeout(PATIENCE, tokio_tungstenite::connect_async(format!("ws://{addr}/members")))
        .await
        .unwrap_or_else(|_| panic!("connecting to {addr} did not finish within {PATIENCE:?}"))
        .unwrap_or_else(|error| panic!("connecting to {addr} failed: {error}"));
    send(&mut socket, json!({ "event": "ready", "id": 0 })).await;
    assert_eq!(next(&mut socket).await, json!({ "id": 0, "complete": true }), "the member's `ready`");
    socket
}

async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::text(message.to_string())).await.unwrap_or_else(|error| panic!("sending {message} failed: {error}"));
}

/// The next text frame as JSON, within `PATIENCE`.
async fn next(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .unwrap_or_else(|_| panic!("no frame arrived within {PATIENCE:?}"))
            .unwrap_or_else(|| panic!("the connection ended"))
            .unwrap_or_else(|error| panic!("reading a frame failed: {error}"));
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap_or_else(|error| panic!("a frame is not JSON: {error}: {text}"));
        }
    }
}

/// The data of the next `shouted` broadcast `socket` receives within `within`, skipping the
/// answers to its own calls; `None` when none arrives.
async fn shouted(socket: &mut Socket, within: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next()).await.ok()??.ok()?;
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(&text).ok()?;
        if value["event"] == "shouted" {
            return value["data"].as_str().map(str::to_owned);
        }
    }
}

/// A room broadcast sent from either instance reaches the room's member on the other process.
///
/// The child prints its address after `listen`, which returns once its subscription is live, so
/// one broadcast in each direction is enough.
#[tokio::test(flavor = "multi_thread")]
async fn a_room_broadcast_reaches_the_member_on_the_other_process() {
    let (_redis, url) = start_redis("ulo-conf-r24-ws-redis-peers").await;
    let local = Instance::start(&url).await;
    let peer = Peer::start(&url).await;
    let mut here = member(local.addr).await;
    let mut there = member(peer.addr).await;

    send(&mut here, json!({ "event": "shout", "data": "from here", "id": 1 })).await;
    let reached = shouted(&mut there, PATIENCE).await;
    assert_eq!(reached.as_deref(), Some("from here"), "the member on the other process received the broadcast");

    send(&mut there, json!({ "event": "shout", "data": "from there", "id": 1 })).await;
    // This process's member also receives its own broadcast, which comes first.
    assert_eq!(shouted(&mut here, PATIENCE).await.as_deref(), Some("from here"), "this process's member received its own broadcast");
    assert_eq!(shouted(&mut here, PATIENCE).await.as_deref(), Some("from there"), "the member on this process received the other's");

    let _ = here.close(None).await;
    let _ = there.close(None).await;
    peer.stop().await;
    local.stop().await;
}

/// The second process: an instance on the Redis URL in `ULO_WS_REDIS_PEER`, which prints its
/// address and serves until its stdin closes. Run by the test above; with the variable unset it
/// does nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "the second process of `a_room_broadcast_reaches_the_member_on_the_other_process`"]
async fn peer_instance() {
    let Ok(url) = std::env::var(PEER) else { return };
    let instance = Instance::start(&url).await;
    println!("{ADDR_LINE}{}", instance.addr);
    let closed = tokio::task::spawn_blocking(|| {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
    });
    let _ = tokio::time::timeout(Duration::from_secs(300), closed).await;
    instance.stop().await;
}

/// Relays every connection to `upstream`, and on the first connection that sends a `SUBSCRIBE`
/// holds what Redis answers after it for `delay`: the subscriber's connection and its handshake
/// are served at once, and only the confirmation is late.
async fn slow_subscribe_confirmation(upstream: SocketAddr, delay: Duration) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("the relay binds");
    let addr = listener.local_addr().expect("the relay's address");
    let delayed = Arc::new(AtomicBool::new(false));
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let delayed = Arc::clone(&delayed);
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(upstream).await else { return };
                let (mut from_client, mut to_client) = client.into_split();
                let (mut from_server, mut to_server) = server.into_split();
                // Set by this connection when it sends the first `SUBSCRIBE` any connection sends.
                let holds = Arc::new(AtomicBool::new(false));
                let marks = Arc::clone(&holds);
                let upstream_half = tokio::spawn(async move {
                    let mut chunk = vec![0u8; 16 * 1024];
                    while let Ok(n) = from_client.read(&mut chunk).await {
                        if n == 0 {
                            break;
                        }
                        if chunk[..n].windows(9).any(|w| w.eq_ignore_ascii_case(b"SUBSCRIBE")) && !delayed.swap(true, Ordering::SeqCst) {
                            marks.store(true, Ordering::SeqCst);
                        }
                        if to_server.write_all(&chunk[..n]).await.is_err() {
                            break;
                        }
                    }
                });
                let mut chunk = vec![0u8; 16 * 1024];
                while let Ok(n) = from_server.read(&mut chunk).await {
                    if n == 0 {
                        break;
                    }
                    if holds.swap(false, Ordering::SeqCst) {
                        tokio::time::sleep(delay).await;
                    }
                    if to_client.write_all(&chunk[..n]).await.is_err() {
                        break;
                    }
                }
                upstream_half.abort();
            });
        }
    });
    addr
}

/// A member's own broadcast, sent the moment its connection is up, reaches it through Redis. The
/// instance has just returned from `listen` and nothing waits for its subscription.
async fn shout_reaches_its_own_member(url: &str) {
    let instance = Instance::start(url).await;
    let mut member = member(instance.addr).await;
    send(&mut member, json!({ "event": "shout", "data": "at once", "id": 1 })).await;
    let received = shouted(&mut member, PATIENCE).await;
    assert_eq!(received.as_deref(), Some("at once"), "the member received its room's broadcast");
    let _ = member.close(None).await;
    instance.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broadcast_published_right_after_listen_reaches_a_member_of_the_same_process() {
    let (_redis, url) = start_redis("ulo-conf-r24-ws-redis-fresh").await;
    shout_reaches_its_own_member(&url).await;
}

/// Redis's answer to the subscriber's first `SUBSCRIBE` reaches it `DELAY` late, its connection
/// served at once. A `listen` that returned before the subscription was confirmed would let the
/// first broadcast pass unheard.
#[tokio::test(flavor = "multi_thread")]
async fn listen_waits_for_the_subscription_when_redis_is_slow_to_confirm_it() {
    const DELAY: Duration = Duration::from_millis(1500);
    let (_redis, url) = start_redis("ulo-conf-r24-ws-redis-slow").await;
    let upstream: SocketAddr = url.trim_start_matches("redis://").parse().expect("the container's address");
    let slow = format!("redis://{}", slow_subscribe_confirmation(upstream, DELAY).await);

    let started = Instant::now();
    let instance = Instance::start(&slow).await;
    let listened = started.elapsed();
    assert!(listened >= DELAY, "`listen` returned after {listened:?}, before Redis served the subscription at {DELAY:?}");
    let mut member = member(instance.addr).await;
    send(&mut member, json!({ "event": "shout", "data": "at once", "id": 1 })).await;
    let received = shouted(&mut member, PATIENCE).await;
    assert_eq!(received.as_deref(), Some("at once"), "the broadcast published straight after `listen` reached the member");
    let _ = member.close(None).await;
    instance.stop().await;
}
