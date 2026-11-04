use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use bytes::Bytes;
use futures_util::FutureExt;
use futures_util::future::{Either, select};
use tokio::net::UdpSocket;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture, Permanent, Runtime, Timer};
use ulo_runtime_tokio::Tokio;
use ulo_net::{Endpoint, EndpointSpec};
use ulo_rpc::link::{Inbound, UNARY_ONLY};
use ulo_rpc::{
    Ack, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link, NoTimer, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::__private::ordered;
use ulo_transport::{Details, ErrorKind};

/// The largest UDP payload over IPv4: 65,535 less the IP and UDP headers. One frame is one
/// datagram, envelope included.
const MAX_DATAGRAM: u64 = 65_507;

/// Deliveries queued for the server; a full queue holds the receiving loop back, and the socket's
/// own buffer then drops what it cannot hold, as UDP does.
const DELIVERY_QUEUE: usize = 1024;

/// Reply frames read ahead of the client; a full queue holds the reply socket's reader back.
const REPLY_QUEUE: usize = 64;

/// Datagrams queued for one socket's writer ahead of the next send's wait: each send is queued at its call and waits until it is
/// among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// How long the server's writer keeps the socket after the link drops, to send what was queued
/// before it (transports DESIGN §5.2, "A cut `close`"), measured with the app's `Timer`.
const CUT_GRACE: Duration = Duration::from_secs(2);

/// The UDP link: a server binds its endpoint, a client sends to it.
///
/// A server maps each sender's call ids to ids of its own, so two senders never share one, and
/// writes the sender's id back on its reply. No datagram is retried: a lost request or reply is
/// the caller's `Timeout`. The inbound stream ends once the draining server holds no call, and the
/// socket answers a request arriving afterwards `err` of kind `unavailable` itself, as the server
/// answers one during the drain, until `close`.
///
/// A client holds no connection, so `close` ends what stands for one: each socket the client side
/// bound stops receiving and is released, its reply lane ends, and the calls waiting on it fail
/// `Unavailable`. A call made afterwards binds a new socket.
///
/// Each socket has one writer task, fed in order by every send on it, so the datagrams one
/// socket sends leave it in the order they were sent.
///
/// A server's link dropped without `close`, as it is when its app drops before the close step,
/// stops its receive task, and its writer once it has sent the datagrams already queued or two
/// seconds have passed on the app's `Timer`, so its socket closes and the address can be bound
/// again. A reply queued as the app drops is sent, among them the `unavailable` a `close` cut by
/// a spent `shutdown_timeout` queues in its one poll for each call the server still holds.
///
/// The link's sockets and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`. So does an endpoint that is
/// not an IP address and port: the link resolves no host name.
pub struct Udp {
    pub(crate) endpoint: EndpointSpec,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    /// Set by `prepare`.
    pub(crate) prepared: Option<SocketAddr>,
    /// The app's clock, taken in `prepare`, which bounds the server writer's last sends.
    timer: Option<Arc<dyn Timer>>,
    pub(crate) state: Arc<State>,
    /// The server's receive task; set while the server listens. It is held here and not in
    /// `State`, which the task holds itself, so the link's drop reaches it.
    receiving: Mutex<Option<Owned>>,
    /// The server socket's writer, which `close` leaves running to send its refusals.
    writing: Mutex<Option<Flushed>>,
    /// The server's calls in flight; set while the server listens.
    calls: Mutex<Option<Arc<Mutex<Ids>>>>,
}

/// A task only the link holds, aborted when dropped: a tokio `JoinHandle` detaches on drop.
/// `close` awaits the receive task through it, and the abort then finds the task ended.
struct Owned(JoinHandle<()>);

impl Drop for Owned {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The server's writer task, which the link alone stops: dropped, it tells the writer to send
/// what is queued and end.
struct Flushed(Option<oneshot::Sender<()>>);

impl Drop for Flushed {
    fn drop(&mut self) {
        if let Some(stop) = self.0.take() {
            let _ = stop.send(());
        }
    }
}

pub(crate) struct State {
    bound: Mutex<Vec<BoundAddr>>,
    /// The server's socket, read once more by `close` after the receive task has stopped.
    socket: Mutex<Option<Arc<UdpSocket>>>,
    /// The server socket's writer queue, which `close` refuses what the socket still holds through.
    writes: Mutex<Option<ordered::Sender<Datagram>>>,
    next_id: AtomicU64,
    /// Raised by `drain`.
    draining: watch::Sender<bool>,
    /// Calls in flight: ids mapped and not yet forgotten.
    held: watch::Sender<usize>,
    /// Advanced by `close`; a client socket's reply lane ends when it changes.
    client_epoch: watch::Sender<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Udp {
    /// The link on `endpoint`: text parsed as an IP address and port in `usable`, where a client
    /// takes the link, and in a server's `prepare`, or a `SocketAddr`. An inherited endpoint is
    /// refused there too, since the activation adopts listening TCP sockets alone.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Udp {
            endpoint: endpoint.into(),
            codec: Codec::Json,
            runtime: Tokio::try_current(),
            prepared: None,
            timer: None,
            receiving: Mutex::new(None),
            writing: Mutex::new(None),
            calls: Mutex::new(None),
            state: Arc::new(State {
                bound: Mutex::new(Vec::new()),
                socket: Mutex::new(None),
                writes: Mutex::new(None),
                next_id: AtomicU64::new(1),
                draining: watch::Sender::new(false),
                held: watch::Sender::new(0),
                client_epoch: watch::Sender::new(0),
            }),
        }
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// The tokio runtime the link's sockets and tasks run on, in place of the one current where
    /// it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }

    /// The endpoint as written on both sides: `resolve` would turn an address `ulo dev --listen`
    /// holds as a TCP socket into that socket, which a datagram socket cannot be. Text is parsed as
    /// an IP address and port, and a host name is refused: the link resolves none. Each refusal is
    /// permanent, so `usable` reports it where the client takes the link.
    fn address(&self) -> Result<SocketAddr, BoxError> {
        let endpoint = self
            .endpoint
            .resolve_as_written()
            .map_err(|error| format!("the UDP link takes IP addresses only and resolves no host name: {error}"))?;
        match endpoint {
            Endpoint::Addr(addr) => Ok(addr),
            endpoint => Err(format!(
                "{endpoint}: the UDP link takes an address; the activation adopts listening TCP sockets alone"
            )
            .into()),
        }
    }
}

const NO_RUNTIME: &str = "the UDP link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Udp {
    const NAME: &'static str = "udp";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
            .binary(self.codec.binary())
            .max_frame(Some(MAX_DATAGRAM))
            .ordering(ulo_rpc::Ordering::Unordered)
            .shapes(UNARY_ONLY)
            .miss_signal(true)
            // `close` answers what its socket holds; a datagram arriving after the socket closes
            // reaches no one. A connected client socket may learn of it through an ICMP port
            // unreachable, which is often filtered, so the link does not rely on it.
            .confirms_drain(false)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime()?.require_io()?;
        self.address().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        // `listen` binds on the link's runtime: one that has shut down, or has no IO driver, is
        // refused here, before the configuration and before any server binds. The shutdown is
        // asked first, since a runtime that has shut down answers nothing about its driver.
        let runtime = self.runtime()?;
        runtime.run(async {}).await.map_err(Permanent::new)?;
        runtime.require_io().map_err(Permanent::new)?;
        self.prepared = Some(self.address()?);
        self.timer = app.timer().cloned();
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = patterns;
        let Some(addr) = self.prepared else {
            return Err("the UDP link listens once `prepare` has resolved its endpoint".into());
        };
        if lock(&self.receiving).is_some() {
            return Err("the UDP link is already listening".into());
        }
        let Some(timer) = self.timer.clone() else {
            return Err(NoTimer { link: "UDP" }.into());
        };
        let runtime = self.runtime()?;
        let socket = Arc::new(runtime.run(UdpSocket::bind(addr)).await.map_err(Permanent::new)??);
        *lock(&self.state.bound) = vec![BoundAddr::new("rpc", socket.local_addr()?)];
        *lock(&self.state.socket) = Some(Arc::clone(&socket));
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        let (dropped, stop) = oneshot::channel::<()>();
        drop(runtime.handle().spawn(write_server_datagrams(Arc::clone(&socket), queued, stop, timer)));
        *lock(&self.writing) = Some(Flushed(Some(dropped)));
        *lock(&self.state.writes) = Some(writes.clone());
        let ids = Arc::new(Mutex::new(Ids::new(Arc::clone(&self.state))));
        *lock(&self.calls) = Some(Arc::clone(&ids));
        let (deliveries, inbound) = mpsc::channel(DELIVERY_QUEUE);
        let receiving = runtime.handle().spawn(receive(socket, self.codec, Arc::clone(&self.state), ids, deliveries, writes));
        *lock(&self.receiving) = Some(Owned(receiving));
        let inbound = futures_util::stream::unfold(inbound, |mut inbound| async move {
            let delivery = inbound.recv().await?;
            Some((delivery, inbound))
        });
        Ok(Box::pin(inbound))
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let addr = self.address()?;
        let runtime = self.runtime()?;
        let local: SocketAddr = if addr.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
        let socket = runtime
            .run(async move {
                let socket = UdpSocket::bind(local).await?;
                socket.connect(addr).await?;
                Ok::<_, io::Error>(socket)
            })
            .await??;
        // The reply lane holds the socket; the writer reaches it through a `Weak`, so the socket is
        // released once the lane ends, at `close` or when the OS reports the server unreachable.
        let socket = Arc::new(socket);
        let codec = self.codec;
        let epoch = self.state.client_epoch.subscribe();
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        runtime.handle().spawn(write_datagrams(Arc::downgrade(&socket), queued, closed(epoch.clone())));
        // Queued at the call, so datagrams leave in the order their sends were called.
        let send = Box::new(move |_pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
            let queued = encode(codec, &frame).map(|bytes| write(&writes, bytes, None));
            Box::pin(async move { queued?.await })
        });
        let (frames, replies) = mpsc::channel(REPLY_QUEUE);
        runtime.handle().spawn(read_replies(socket, epoch, codec, frames));
        let replies = futures_util::stream::unfold(replies, |mut replies| async move {
            let frame = replies.recv().await?;
            Some((frame, replies))
        });
        Ok(Outbound { send, replies: Box::pin(replies) })
    }

    /// UDP has no signal for a sender: the server answers each new call `unavailable` itself, and
    /// the socket keeps receiving so a `cancel` still reaches a call in flight.
    async fn drain(&self) {
        self.state.draining.send_replace(true);
    }

    /// A server's `close` first answers every call it holds `err` of kind `unavailable`, queued
    /// before its first await, so a `close` a spent shutdown cap polls once sends them; a call's
    /// own answer arriving afterwards is refused, and so is a request the receive task decodes
    /// before it stops, answered the same way rather than delivered. It then stops the receive
    /// task, reads what the socket's buffer already holds and answers each request found there the
    /// same way, so a datagram that arrived before the close is answered; one arriving after it is
    /// not.
    async fn close(&self) -> Result<(), BoxError> {
        let calls = lock(&self.calls).take();
        let writes = lock(&self.state.writes).clone();
        if let (Some(calls), Some(writes)) = (calls, writes) {
            answer_held(&calls, &writes, self.codec);
        }
        #[cfg(test)]
        {
            let bound = lock(&self.state.bound).first().map(|bound| bound.addr);
            tests::gate::after_sweep(bound).await;
        }
        self.state.client_epoch.send_modify(|epoch| *epoch += 1);
        let receiving = lock(&self.receiving).take();
        if let Some(mut receiving) = receiving {
            receiving.0.abort();
            let _ = (&mut receiving.0).await;
        }
        let socket = lock(&self.state.socket).take();
        let writes = lock(&self.state.writes).take();
        if let (Some(socket), Some(writes)) = (socket, writes) {
            let codec = self.codec;
            self.runtime()?.run(async move { final_read(&socket, codec, &writes).await }).await?;
        }
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        lock(&self.state.bound).clone()
    }
}

/// Resolves once the client epoch moves past the one `epoch` was subscribed at, or the link is
/// gone.
async fn closed(mut epoch: watch::Receiver<u64>) {
    let _ = epoch.changed().await;
}

/// Reads the client socket's reply datagrams into `frames` until the link closes, the reply lane
/// is dropped, or the OS reports the server unreachable; the socket is released as this returns.
async fn read_replies(socket: Arc<UdpSocket>, epoch: watch::Receiver<u64>, codec: Codec, frames: mpsc::Sender<Frame>) {
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    loop {
        let received = tokio::select! {
            received = socket.recv(&mut buffer) => received,
            () = closed(epoch.clone()) => return,
            () = frames.closed() => return,
        };
        // An error here is the OS reporting the server unreachable: the reply lane ends, and the
        // next call binds a new socket.
        let len = match received {
            Ok(len) => len,
            Err(error) => {
                tracing::debug!(%error, "the UDP link's reply lane ended");
                return;
            }
        };
        match codec.decode_frame(&buffer[..len]) {
            Ok(frame) => {
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
            Err(error) => tracing::debug!(%error, "a reply datagram did not decode and was dropped"),
        }
    }
}

/// Every datagram the socket receives, as deliveries, until the link closes. A datagram that does
/// not decode carries no id to answer and is dropped. Once the link is draining and holds no call,
/// the inbound stream ends, and a request arriving afterwards is refused here: nothing tells a UDP
/// caller to stop sending, and the stream ending only at `close` would hold the drain to its
/// deadline. This loop alone sends deliveries and decides the end between datagrams, so a
/// `cancel` releasing the last call is queued before the stream ends.
async fn receive(
    socket: Arc<UdpSocket>,
    codec: Codec,
    state: Arc<State>,
    ids: Arc<Mutex<Ids>>,
    deliveries: mpsc::Sender<Delivery>,
    writes: ordered::Sender<Datagram>,
) {
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    let mut deliveries = Some(deliveries);
    loop {
        let received = tokio::select! {
            biased;
            received = socket.recv_from(&mut buffer) => received,
            () = idle(&state), if deliveries.is_some() => {
                deliveries = None;
                continue;
            }
        };
        let (len, peer) = match received {
            Ok(received) => received,
            // A sender's ICMP refusal surfaces here on some platforms; the socket stays usable.
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused) => {
                tracing::debug!(%error, "the UDP link's socket reported a sender's refusal");
                continue;
            }
            // Any other failure ends the inbound stream, which before the drain stops the server
            // and starts the shutdown.
            Err(error) => {
                tracing::error!(%error, "the UDP link's socket failed");
                return;
            }
        };
        let frame = match codec.decode_frame(&buffer[..len]) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!(%error, %peer, "a datagram did not decode and was dropped");
                continue;
            }
        };
        let Some(sender) = deliveries.as_ref() else {
            refuse(&writes, codec, peer, &frame).await;
            continue;
        };
        #[cfg(test)]
        {
            let bound = lock(&state.bound).first().map(|bound| bound.addr);
            tests::stall::before_inward(bound);
        }
        let frame = match inward(frame, peer, &ids, &state.next_id) {
            Inward::Deliver(frame) => frame,
            Inward::Refuse(frame) => {
                refuse_after_close(&socket, &writes, codec, peer, &frame);
                continue;
            }
            Inward::Discard => continue,
        };
        let reply = reply_path(writes.clone(), peer, codec, Arc::clone(&ids));
        if sender.send(Delivery { frame, reply: Some(reply), ack: Ack::none() }).await.is_err() {
            return;
        }
    }
}

/// Every datagram the socket's receive buffer holds, read without waiting until it is empty; each
/// request or streamed request is answered `err` of kind `unavailable`. The read goes through a
/// duplicate of the socket, not tokio's: tokio tries a read only once its reactor has seen the
/// socket readable, and a datagram that arrived since its last turn would be left unread.
async fn final_read(socket: &UdpSocket, codec: Codec, writes: &ordered::Sender<Datagram>) {
    let duplicate = match socket2::SockRef::from(socket).try_clone().and_then(|duplicate| {
        duplicate.set_nonblocking(true)?;
        Ok(std::net::UdpSocket::from(duplicate))
    }) {
        Ok(duplicate) => duplicate,
        Err(error) => {
            tracing::warn!(%error, "the UDP link could not read its socket at close; requests still in its buffer go unanswered");
            return;
        }
    };
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    let mut refused = 0usize;
    loop {
        let (len, peer) = match duplicate.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused) => continue,
            Err(error) => {
                tracing::debug!(%error, "the UDP link's read at close failed");
                break;
            }
        };
        let Ok(frame) = codec.decode_frame(&buffer[..len]) else { continue };
        if matches!(frame, Frame::Req { .. } | Frame::Open { .. }) {
            refused += 1;
        }
        refuse(writes, codec, peer, &frame).await;
    }
    if refused > 0 {
        tracing::debug!(refused, "the UDP link answered requests left in its socket at close");
    }
}

/// Resolves once the link is draining and holds no call.
async fn idle(state: &State) {
    let mut draining = state.draining.subscribe();
    let mut held = state.held.subscribe();
    let _ = draining.wait_for(|draining| *draining).await;
    let _ = held.wait_for(|held| *held == 0).await;
}

/// Answers a request or streamed request the server will not take, under the sender's own id;
/// anything else names no call in flight and is dropped.
async fn refuse(writes: &ordered::Sender<Datagram>, codec: Codec, peer: SocketAddr, frame: &Frame) {
    let (Frame::Req { id, .. } | Frame::Open { id, .. }) = *frame else { return };
    let refused = match encode(codec, &unavailable(id)) {
        Ok(bytes) => write(writes, bytes, Some(peer)).await,
        Err(error) => Err(error),
    };
    if let Err(error) = refused {
        tracing::debug!(%error, %peer, "the UDP link could not refuse a request it no longer takes");
    }
}

/// Answers a request or streamed request that arrived once `close` had answered the calls held,
/// sent on the socket in this poll rather than through the writer: after a cut `close` the link's
/// drop can end the writer before this task, still mid-poll on another worker, reaches the queue.
/// The socket is non-blocking, so the send never waits.
fn refuse_after_close(socket: &UdpSocket, writes: &ordered::Sender<Datagram>, codec: Codec, peer: SocketAddr, frame: &Frame) {
    let (Frame::Req { id, .. } | Frame::Open { id, .. }) = *frame else { return };
    let bytes = match encode(codec, &unavailable(id)) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::debug!(%error, %peer, "the UDP link could not refuse a request it no longer takes");
            return;
        }
    };
    let sent = socket2::SockRef::from(socket).send_to(&bytes, &peer.into());
    queue_unsent(sent, writes, bytes, peer);
}

/// Hands the writer a refusal the socket's send buffer had no room for, to send once it has. A
/// writer a cut `close` has already ended drops it, as it drops any datagram queued after its end.
fn queue_unsent(sent: io::Result<usize>, writes: &ordered::Sender<Datagram>, bytes: Bytes, peer: SocketAddr) {
    match sent {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => drop(write(writes, bytes, Some(peer))),
        Err(error) => tracing::debug!(%error, %peer, "the UDP link could not refuse a request it no longer takes"),
    }
}

/// `err` of kind `unavailable` with the shutdown phrase, under `id`.
fn unavailable(id: u64) -> Frame {
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE, Details::new()) }
}

/// `close`'s answer to every call still held, queued on the server socket's writer in one hold of
/// the call table's lock, which each reply path takes to queue its frame: a call's own answer is
/// queued before these or refused. The writer sends what is queued when the link drops, so a
/// `close` cut after this sends them, where a call's task rendering the drain's `unavailable` on
/// another worker may queue it after the writer has ended, and the answer is lost (F586).
fn answer_held(calls: &Mutex<Ids>, writes: &ordered::Sender<Datagram>, codec: Codec) {
    let mut ids = lock(calls);
    for (peer, caller) in ids.forget_all() {
        match encode(codec, &unavailable(caller)) {
            // Queued at the call; nothing waits for the send, which a cut `close` would not see.
            Ok(bytes) => drop(write(writes, bytes, Some(peer))),
            Err(error) => tracing::debug!(%error, %peer, "the UDP link could not answer a call it held at close"),
        }
    }
}

/// What the receive loop does with a sender's frame.
#[derive(Debug)]
enum Inward {
    /// Delivered to the server, its id mapped to the server's.
    Deliver(Frame),
    /// A request or streamed request that arrived once `close` had answered the calls held, to be
    /// answered `unavailable` under the sender's id rather than opened in a table nothing answers:
    /// the receive task can still be mid-poll on another worker when `close` sweeps the table.
    Refuse(Frame),
    /// One the server takes no part in, or that names no call in flight.
    Discard,
}

/// A sender's frame with its id mapped to the server's, checked against the sweep in the same hold
/// of the call table's lock that opens the call.
fn inward(frame: Frame, peer: SocketAddr, ids: &Mutex<Ids>, next: &AtomicU64) -> Inward {
    let mut ids = lock(ids);
    if ids.closed && matches!(frame, Frame::Req { .. } | Frame::Open { .. }) {
        return Inward::Refuse(frame);
    }
    let mapped = match frame {
        Frame::Req { id, pattern, headers, data } => Some(Frame::Req { id: ids.open(peer, id, next), pattern, headers, data }),
        Frame::Open { id, pattern, headers } => Some(Frame::Open { id: ids.open(peer, id, next), pattern, headers }),
        Frame::Evt { pattern, headers, data } => Some(Frame::Evt { pattern, headers, data }),
        Frame::In { id, data } => ids.server(peer, id).map(|id| Frame::In { id, data }),
        Frame::InEnd { id } => ids.server(peer, id).map(|id| Frame::InEnd { id }),
        Frame::Cancel { id } => ids.cancel(peer, id).map(|id| Frame::Cancel { id }),
        _ => None,
    };
    mapped.map_or(Inward::Discard, Inward::Deliver)
}

/// The calls in flight: each sender's id and the server's, both ways, counted in `State::held`.
struct Ids {
    by_caller: HashMap<(SocketAddr, u64), u64>,
    by_server: HashMap<u64, (SocketAddr, u64)>,
    /// Set by `forget_all`: a request arriving afterwards is refused, not opened.
    closed: bool,
    state: Arc<State>,
}

impl Ids {
    fn new(state: Arc<State>) -> Self {
        Ids { by_caller: HashMap::new(), by_server: HashMap::new(), closed: false, state }
    }

    /// Every call held, as its sender's address and id, each forgotten, so a reply to it is
    /// refused, and the table closed, so a request arriving afterwards is refused too.
    fn forget_all(&mut self) -> Vec<(SocketAddr, u64)> {
        self.closed = true;
        self.by_caller.clear();
        let held = self.by_server.drain().map(|(_, caller)| caller).collect();
        self.counted();
        held
    }

    fn counted(&self) {
        self.state.held.send_replace(self.by_server.len());
    }

    fn open(&mut self, peer: SocketAddr, caller: u64, next: &AtomicU64) -> u64 {
        let server = next.fetch_add(1, Ordering::Relaxed);
        if let Some(stale) = self.by_caller.insert((peer, caller), server) {
            self.by_server.remove(&stale);
        }
        self.by_server.insert(server, (peer, caller));
        self.counted();
        server
    }

    fn server(&self, peer: SocketAddr, caller: u64) -> Option<u64> {
        self.by_caller.get(&(peer, caller)).copied()
    }

    fn cancel(&mut self, peer: SocketAddr, caller: u64) -> Option<u64> {
        let server = self.by_caller.remove(&(peer, caller))?;
        self.by_server.remove(&server);
        self.counted();
        Some(server)
    }

    fn caller(&self, server: u64) -> Option<u64> {
        self.by_server.get(&server).map(|(_, caller)| *caller)
    }

    fn finish(&mut self, server: u64) {
        if let Some(caller) = self.by_server.remove(&server) {
            self.by_caller.remove(&caller);
        }
        self.counted();
    }
}

/// Replies to one sender, under its own id, through the server socket's writer; the mapping is
/// forgotten once a reply ends the call.
fn reply_path(writes: ordered::Sender<Datagram>, peer: SocketAddr, codec: Codec, ids: Arc<Mutex<Ids>>) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        // Queued at the call, so replies leave in the order their sends were called.
        let queued = queue_reply(&writes, peer, codec, &ids, frame);
        Box::pin(async move { queued?.await })
    })
    .peer(peer)
}

/// `frame` under the caller's id, queued now in one hold of the call table's lock; the mapping is
/// forgotten once a reply ends the call, or once `close` has answered the calls held.
fn queue_reply(
    writes: &ordered::Sender<Datagram>,
    peer: SocketAddr,
    codec: Codec,
    ids: &Mutex<Ids>,
    frame: Frame,
) -> Result<impl Future<Output = Result<(), BoxError>> + Send + 'static, BoxError> {
    let Some(server) = frame.id() else {
        return Err(BoxError::from("a reply frame carries no id"));
    };
    let mut ids = lock(ids);
    let Some(caller) = ids.caller(server) else {
        return Err("the call is no longer in flight".into());
    };
    let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
    let bytes = encode(codec, &with_id(frame, caller))?;
    let written = write(writes, bytes, Some(peer));
    if ends {
        ids.finish(server);
    }
    Ok(written)
}

/// One datagram for a socket's writer: to `to` on the server's socket, to the connected address on
/// a client's, the writer's outcome answered on `answer`.
struct Datagram {
    bytes: Bytes,
    to: Option<SocketAddr>,
    answer: oneshot::Sender<Result<(), BoxError>>,
}

/// Queues `bytes` on a socket's writer at the call and answers a wait for it to be sent. The
/// queue keeps the order of the calls, so one socket's datagrams leave it in that order.
fn write(writes: &ordered::Sender<Datagram>, bytes: Bytes, to: Option<SocketAddr>) -> impl Future<Output = Result<(), BoxError>> + Send + 'static {
    let (answer, answered) = oneshot::channel();
    let room = writes.send(Datagram { bytes, to, answer });
    async move {
        room.await.map_err(|_| BoxError::from("the UDP link's socket is closed"))?;
        answered.await.map_err(|_| BoxError::from("the UDP link's socket is closed"))?
    }
}

/// The socket a writer sends on: held by the server's writer, reached through a `Weak` by a
/// client's, whose reply lane owns the socket.
trait Sending: Send + 'static {
    fn socket(&self) -> Option<Arc<UdpSocket>>;
}

impl Sending for Arc<UdpSocket> {
    fn socket(&self) -> Option<Arc<UdpSocket>> {
        Some(Arc::clone(self))
    }
}

impl Sending for Weak<UdpSocket> {
    fn socket(&self) -> Option<Arc<UdpSocket>> {
        self.upgrade()
    }
}

/// Sends each queued datagram in turn until the queue's senders are gone or `stop` resolves.
async fn write_datagrams(socket: impl Sending, mut queued: ordered::Receiver<Datagram>, stop: impl Future<Output = ()>) {
    let mut stop = std::pin::pin!(stop);
    loop {
        let datagram = tokio::select! {
            datagram = queued.recv() => match datagram {
                Some(datagram) => datagram,
                None => return,
            },
            () = &mut stop => return,
        };
        send_datagram(&socket, datagram).await;
    }
}

/// The server socket's writer: each queued datagram in turn until the link drops, then every
/// datagram the queue holds at that moment, so a reply queued as the link drops is sent though the
/// writer is first polled after it. The link's drop can close the queue before it signals
/// `dropped`, the receive task's abort releasing the last sender, so a closed queue ends the
/// writer the same way: through the bounded drain, which finds it empty.
async fn write_server_datagrams(socket: Arc<UdpSocket>, mut queued: ordered::Receiver<Datagram>, dropped: oneshot::Receiver<()>, timer: Arc<dyn Timer>) {
    let mut dropped = std::pin::pin!(dropped);
    loop {
        let datagram = tokio::select! {
            biased;
            _ = &mut dropped => break,
            datagram = queued.recv() => match datagram {
                Some(datagram) => datagram,
                None => break,
            },
        };
        send_datagram(&socket, datagram).await;
    }
    drain_queued(&mut queued, &*timer, CUT_GRACE, |datagram| send_datagram(&socket, datagram)).await;
}

/// Sends what `queued` holds now, giving up once `grace` has passed on `timer`, or at once when
/// `timer` refuses it, its runtime shut down: a send waiting for
/// room in a socket whose buffer never drains would otherwise hold the writer, and the socket with
/// it, indefinitely.
async fn drain_queued<F, Fut>(queued: &mut ordered::Receiver<Datagram>, timer: &dyn Timer, grace: Duration, mut send: F)
where
    F: FnMut(Datagram) -> Fut,
    Fut: Future<Output = ()>,
{
    let drain = async {
        while let Some(Some(datagram)) = queued.recv().now_or_never() {
            send(datagram).await;
        }
    };
    match select(std::pin::pin!(drain), timer.try_sleep(grace)).await {
        Either::Left(_) => {}
        Either::Right((Ok(()), _)) => tracing::debug!("the UDP link let go of its socket `CUT_GRACE` after its drop with writes still queued"),
        // Timed on no other clock: the grace is the app's, and its runtime is gone.
        Either::Right((Err(refused), _)) => tracing::debug!(%refused, "the runtime timing the UDP link's grace refused it; the socket is let go without one"),
    }
}

async fn send_datagram(socket: &impl Sending, Datagram { bytes, to, answer }: Datagram) {
    let sent = match socket.socket() {
        Some(socket) => match to {
            Some(peer) => socket.send_to(&bytes, peer).await,
            None => socket.send(&bytes).await,
        }
        .map(drop)
        .map_err(BoxError::from),
        None => Err("the UDP link's client socket is closed".into()),
    };
    let _ = answer.send(sent);
}

fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        other => other,
    }
}

/// One datagram's bytes, refused with `FrameTooLarge` over 65,507 before anything is sent.
fn encode(codec: Codec, frame: &Frame) -> Result<Bytes, BoxError> {
    let bytes = codec.encode_frame(frame)?;
    let size = bytes.len() as u64;
    if size > MAX_DATAGRAM {
        return Err(Box::new(FrameTooLarge { size, limit: MAX_DATAGRAM }));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::task::{Context, Poll, Waker};
    use std::time::Instant;

    use futures_util::StreamExt;

    use super::*;

    fn datagram() -> Datagram {
        let (answer, _) = oneshot::channel();
        Datagram { bytes: Bytes::new(), to: None, answer }
    }

    /// A clock that moves only when the test advances it, recording the length of each sleep asked
    /// of it. A sleep ends once the clock has advanced past it; the test polls by hand, so no waker
    /// is kept.
    struct Scripted {
        base: Instant,
        at: Arc<Mutex<Duration>>,
        asked: Mutex<Vec<Duration>>,
    }

    impl Scripted {
        fn new() -> Scripted {
            Scripted { base: Instant::now(), at: Arc::new(Mutex::new(Duration::ZERO)), asked: Mutex::new(Vec::new()) }
        }

        fn advance(&self, by: Duration) {
            *lock(&self.at) += by;
        }
    }

    impl Timer for Scripted {
        fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
            lock(&self.asked).push(d);
            let (at, until) = (Arc::clone(&self.at), *lock(&self.at) + d);
            Box::pin(std::future::poll_fn(move |_| if *lock(&at) >= until { Poll::Ready(()) } else { Poll::Pending }))
        }

        fn now(&self) -> Instant {
            self.base + *lock(&self.at)
        }

        fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
            let sleep = Timer::sleep(self, d);
            Box::pin(async move {
                sleep.await;
                Ok(())
            })
        }
    }

    /// The drain ends once the app's clock passes the grace when the first send never completes,
    /// with datagrams still queued, and not a moment sooner.
    #[test]
    fn drain_ends_at_the_grace_when_a_send_never_completes() {
        let (writes, mut queued) = ordered::channel(4);
        for _ in 0..3 {
            drop(writes.send(datagram()));
        }
        let (clock, grace) = (Scripted::new(), Duration::from_millis(200));
        let started = AtomicBool::new(false);
        let mut drain = Box::pin(drain_queued(&mut queued, &clock, grace, |_| {
            started.store(true, Ordering::Relaxed);
            std::future::pending::<()>()
        }));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(drain.as_mut().poll(&mut cx).is_pending(), "the drain ended before its clock moved");
        assert!(started.load(Ordering::Relaxed), "the drain did not start sending");
        clock.advance(grace - Duration::from_millis(1));
        assert!(drain.as_mut().poll(&mut cx).is_pending(), "the drain ended short of its grace");
        clock.advance(Duration::from_millis(1));
        assert!(drain.as_mut().poll(&mut cx).is_ready(), "the drain was still sending once its grace had passed");
        drop(drain);
        assert_eq!(*lock(&clock.asked), [grace], "the sleeps the drain asked of the app's clock");
        assert!(queued.recv().now_or_never().flatten().is_some(), "the drain took every datagram though its first send never completed");
    }

    /// A call's own answer racing `close`'s answer to the calls held, on two threads: once
    /// `answer_held` returns, the call's one terminal frame is already queued, and the call's own
    /// answer was either that frame or is refused. What is queued later would reach an ended
    /// writer after a cut `close`.
    #[test]
    fn a_reply_racing_the_close_is_queued_before_it_or_refused() {
        let state = Udp::new("127.0.0.1:0").state;
        let peer: SocketAddr = (Ipv4Addr::LOCALHOST, 9).into();
        let (mut own_first, mut refused) = (0, 0);
        for round in 0..100_000 {
            let (writes, mut queued) = ordered::channel(WRITE_QUEUE);
            let ids = Mutex::new(Ids::new(Arc::clone(&state)));
            let server = lock(&ids).open(peer, 7, &state.next_id);
            let start = std::sync::Barrier::new(2);
            let taken = |queued: &mut ordered::Receiver<Datagram>| {
                let mut frames = Vec::new();
                while let Some(Some(datagram)) = queued.recv().now_or_never() {
                    frames.push(Codec::Json.decode_frame(&datagram.bytes).expect("the frame decodes"));
                }
                frames
            };
            let reply = || {
                start.wait();
                queue_reply(&writes, peer, Codec::Json, &ids, Frame::Res { id: server, data: ulo_rpc::Data::new(Bytes::from_static(b"1")) }).map(drop)
            };
            // The thread spawned reaches the barrier last and usually runs first, so the two sides
            // swap threads each round and both orders run.
            let (at_close, own) = std::thread::scope(|scope| {
                if round % 2 == 0 {
                    let own = scope.spawn(reply);
                    start.wait();
                    answer_held(&ids, &writes, Codec::Json);
                    let at_close = taken(&mut queued);
                    (at_close, own.join().expect("the reply thread"))
                } else {
                    let close = scope.spawn(|| {
                        start.wait();
                        answer_held(&ids, &writes, Codec::Json);
                        taken(&mut queued)
                    });
                    let own = reply();
                    (close.join().expect("the closing thread"), own)
                }
            });
            let after = taken(&mut queued);
            assert!(after.is_empty(), "round {round}: queued after `answer_held` returned: {after:?}");
            assert_eq!(at_close.len(), 1, "round {round}: the frames queued for the call: {at_close:?}");
            match (&at_close[0], own) {
                (Frame::Res { id: 7, .. }, Ok(())) => own_first += 1,
                (Frame::Err { id: 7, .. }, Err(_)) => refused += 1,
                (frame, own) => panic!("round {round}: {frame:?} queued with the call's own answer {own:?}"),
            }
        }
        assert!(own_first > 0 && refused > 0, "one order never ran: {own_first} answered first, {refused} refused");
    }

    fn request(id: u64) -> Frame {
        Frame::Req { id, pattern: LATE.to_owned(), headers: ulo_rpc::CallHeaders::new(), data: ulo_rpc::Data::new(Bytes::from_static(b"1")) }
    }

    const LATE: &str = "udp.late";

    /// A request racing `close`'s sweep on two threads is either opened before the sweep and
    /// answered by it, or refused; the swept table is left holding no call.
    #[test]
    fn a_request_racing_the_sweep_is_answered_by_it_or_refused() {
        let state = Udp::new("127.0.0.1:0").state;
        let peer: SocketAddr = (Ipv4Addr::LOCALHOST, 9).into();
        let (mut opened_first, mut refused) = (0, 0);
        for round in 0..100_000 {
            let (writes, mut queued) = ordered::channel(WRITE_QUEUE);
            let ids = Mutex::new(Ids::new(Arc::clone(&state)));
            let start = std::sync::Barrier::new(2);
            let arrive = || {
                start.wait();
                inward(request(7), peer, &ids, &state.next_id)
            };
            let sweep = || {
                start.wait();
                answer_held(&ids, &writes, Codec::Json);
            };
            // As in the reply race above, the two sides swap threads each round so both orders run.
            let arrived = std::thread::scope(|scope| {
                if round % 2 == 0 {
                    let arrived = scope.spawn(arrive);
                    sweep();
                    arrived.join().expect("the receiving thread")
                } else {
                    let swept = scope.spawn(sweep);
                    let arrived = arrive();
                    swept.join().expect("the closing thread");
                    arrived
                }
            });
            let mut at_close = Vec::new();
            while let Some(Some(datagram)) = queued.recv().now_or_never() {
                at_close.push(Codec::Json.decode_frame(&datagram.bytes).expect("the frame decodes"));
            }
            assert!(lock(&ids).by_server.is_empty(), "round {round}: the swept table holds a call; the request was {arrived:?}");
            match (at_close.as_slice(), &arrived) {
                ([answer], Inward::Deliver(Frame::Req { .. })) if *answer == unavailable(7) => opened_first += 1,
                ([], Inward::Refuse(frame)) if *frame == request(7) => refused += 1,
                (at_close, arrived) => panic!("round {round}: the sweep queued {at_close:?} for a request that was {arrived:?}"),
            }
        }
        assert!(opened_first > 0 && refused > 0, "one order never ran: {opened_first} opened first, {refused} refused");
    }

    /// After the sweep a streamed request is refused as a request is, under the sender's id and
    /// opened nowhere; an event is still delivered, and a frame naming a call the sweep forgot is
    /// dropped, so nothing answers that call a second time.
    #[test]
    fn after_the_sweep_a_streamed_request_is_refused_and_an_event_delivered() {
        let state = Udp::new("127.0.0.1:0").state;
        let peer: SocketAddr = (Ipv4Addr::LOCALHOST, 9).into();
        let (writes, _queued) = ordered::channel(WRITE_QUEUE);
        let ids = Mutex::new(Ids::new(Arc::clone(&state)));
        let held = inward(request(7), peer, &ids, &state.next_id);
        assert!(matches!(held, Inward::Deliver(Frame::Req { .. })), "the request before the sweep was {held:?}");
        answer_held(&ids, &writes, Codec::Json);
        let open = Frame::Open { id: 8, pattern: LATE.to_owned(), headers: ulo_rpc::CallHeaders::new() };
        match inward(open.clone(), peer, &ids, &state.next_id) {
            Inward::Refuse(frame) => assert_eq!(frame, open, "the streamed request refused after the sweep"),
            other => panic!("a streamed request after the sweep was {other:?}"),
        }
        assert!(lock(&ids).by_server.is_empty(), "the swept table holds a call");
        let event = Frame::Evt { pattern: LATE.to_owned(), headers: ulo_rpc::CallHeaders::new(), data: ulo_rpc::Data::new(Bytes::from_static(b"1")) };
        match inward(event.clone(), peer, &ids, &state.next_id) {
            Inward::Deliver(frame) => assert_eq!(frame, event, "the event delivered after the sweep"),
            other => panic!("an event after the sweep was {other:?}"),
        }
        let forgotten = [Frame::In { id: 7, data: ulo_rpc::Data::new(Bytes::from_static(b"1")) }, Frame::InEnd { id: 7 }, Frame::Cancel { id: 7 }];
        for frame in forgotten {
            let inward = inward(frame.clone(), peer, &ids, &state.next_id);
            assert!(matches!(inward, Inward::Discard), "{frame:?} naming the call the sweep answered was {inward:?}");
        }
    }

    pub(super) mod gate {
        use super::*;

        type Stop = (SocketAddr, oneshot::Sender<()>, oneshot::Receiver<()>);

        static AFTER_SWEEP: Mutex<Vec<Stop>> = Mutex::new(Vec::new());

        /// Stops the next `close` of the server bound at `addr` once it has answered the calls
        /// held, before it stops the receive task: the receiver answers once the close has stopped
        /// there, and the sender releases it.
        pub(crate) fn stop(addr: SocketAddr) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
            let (arrived, stopped) = oneshot::channel();
            let (release, released) = oneshot::channel();
            lock(&AFTER_SWEEP).push((addr, arrived, released));
            (stopped, release)
        }

        pub(crate) async fn after_sweep(bound: Option<SocketAddr>) {
            let stop = {
                let mut stops = lock(&AFTER_SWEEP);
                let at = stops.iter().position(|(addr, _, _)| Some(*addr) == bound);
                at.map(|at| stops.swap_remove(at))
            };
            if let Some((_, arrived, released)) = stop {
                let _ = arrived.send(());
                let _ = released.await;
            }
        }
    }

    struct Root;

    impl ulo::Module for Root {
        fn identity(&self) -> ulo::ModuleIdentity {
            ulo::ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, m: &mut ulo::ModuleDef<'_>) {
            let _ = m;
        }
    }

    /// The receive task still running once `close` has swept the call table, as it is when it is
    /// mid-poll on another worker, is held there by the gate: the request it decodes then is
    /// answered `unavailable` with the shutdown phrase under the sender's id, and never delivered.
    async fn a_request_decoded_after_the_sweep_is_refused_and_not_delivered() {
        const PATIENCE: Duration = Duration::from_secs(5);
        let app = ulo::App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
        let mut udp = Udp::new("127.0.0.1:0");
        udp.prepare(&app.handle()).await.expect("the link prepares");
        let inbound = udp.listen(&[Pattern::from(LATE)]).await.expect("the link listens");
        let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");
        let caller = UdpSocket::bind("127.0.0.1:0").await.expect("the caller's socket");
        let (swept, release) = gate::stop(addr);
        let late = async {
            tokio::time::timeout(PATIENCE, swept).await.expect("the close did not stop after its sweep").expect("the gate");
            let bytes = Codec::Json.encode_frame(&request(7)).expect("the request encodes");
            caller.send_to(&bytes, addr).await.expect("the request is sent");
            let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
            let answer = tokio::time::timeout(PATIENCE, caller.recv_from(&mut buffer)).await;
            let _ = release.send(());
            let (len, _) = answer.expect("no answer to the request decoded after the sweep").expect("the answer is received");
            Codec::Json.decode_frame(&buffer[..len]).expect("the answer decodes")
        };
        let (closed, answer) = tokio::join!(udp.close(), late);
        closed.expect("the link closes");
        assert_eq!(answer, unavailable(7), "what the caller read for the request decoded after the sweep");
        let delivered: Vec<Frame> =
            tokio::time::timeout(PATIENCE, inbound.map(|delivery| delivery.frame).collect()).await.expect("the inbound stream did not end with the close");
        assert!(delivered.is_empty(), "the server was delivered frames after the sweep: {delivered:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_request_decoded_after_the_sweep_is_refused_on_one_thread() {
        a_request_decoded_after_the_sweep_is_refused_and_not_delivered().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_request_decoded_after_the_sweep_is_refused_on_a_worker_pool() {
        a_request_decoded_after_the_sweep_is_refused_and_not_delivered().await;
    }

    pub(super) mod stall {
        use super::*;

        type Stall = (SocketAddr, std::sync::mpsc::SyncSender<()>, std::sync::mpsc::Receiver<()>);

        static BEFORE_INWARD: Mutex<Vec<Stall>> = Mutex::new(Vec::new());

        /// How long a stall holds its worker unreleased before it fails the receive task, so a
        /// test that never releases it fails rather than hangs.
        const LIMIT: Duration = Duration::from_secs(10);

        /// Blocks the worker of the receive task of the server bound at `addr` the next time it
        /// has decoded a datagram, as a preemption mid-poll would: the receiver answers once it
        /// has stopped there, and the sender releases it. Nothing the stalled task holds is locked
        /// meanwhile, so the test can close and drop the link.
        pub(crate) fn arm(addr: SocketAddr) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::SyncSender<()>) {
            let (arrived, stopped) = std::sync::mpsc::sync_channel(1);
            let (release, released) = std::sync::mpsc::sync_channel(1);
            lock(&BEFORE_INWARD).push((addr, arrived, released));
            (stopped, release)
        }

        pub(crate) fn before_inward(bound: Option<SocketAddr>) {
            let stall = {
                let mut stalls = lock(&BEFORE_INWARD);
                let at = stalls.iter().position(|(addr, _, _)| Some(*addr) == bound);
                at.map(|at| stalls.swap_remove(at))
            };
            if let Some((_, arrived, released)) = stall {
                let _ = arrived.send(());
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = released.recv_timeout(LIMIT) {
                    panic!("the receive task's stall was not released within {LIMIT:?}");
                }
            }
        }
    }

    /// A request the receive task decodes while a `close` cut in its first poll lets the link drop
    /// and its writer end is still answered `unavailable` under the sender's id. The receive task
    /// is stalled on its worker after decoding the request, the close is polled once and dropped
    /// with the link, and the task is released only once the writer has let go of the socket.
    /// One worker is stalled, so this needs a pool: on one thread the abort ends the task before
    /// it runs again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_request_decoded_as_a_cut_close_drops_the_link_is_refused() {
        const PATIENCE: Duration = Duration::from_secs(5);
        let app = ulo::App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
        let mut udp = Udp::new("127.0.0.1:0");
        udp.prepare(&app.handle()).await.expect("the link prepares");
        let _inbound = udp.listen(&[Pattern::from(LATE)]).await.expect("the link listens");
        let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");
        let socket = lock(&udp.state.socket).clone().expect("the server's socket");
        let caller = UdpSocket::bind("127.0.0.1:0").await.expect("the caller's socket");
        let (stopped, release) = stall::arm(addr);
        let bytes = Codec::Json.encode_frame(&request(7)).expect("the request encodes");
        caller.send_to(&bytes, addr).await.expect("the request is sent");
        let stopped = tokio::task::spawn_blocking(move || stopped.recv_timeout(PATIENCE));
        stopped.await.expect("the wait for the stall").expect("the receive task did not stop after decoding the request");

        // Held by the link's state, the writer, the stalled receive task and this test.
        let held_by_writer = Arc::strong_count(&socket);
        assert_eq!(held_by_writer, 4, "the holders of the server's socket before the cut");
        let mut close = Box::pin(udp.close());
        assert!(close.as_mut().now_or_never().is_none(), "the close ended in its first poll");
        drop(close);
        drop(udp);
        let writer_ended = tokio::time::timeout(PATIENCE, async {
            while Arc::strong_count(&socket) >= held_by_writer {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        let _ = release.send(());
        writer_ended.expect("the writer did not let go of the socket after the link dropped");

        let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
        let (len, _) = tokio::time::timeout(PATIENCE, caller.recv_from(&mut buffer))
            .await
            .expect("no answer to the request decoded as the cut close dropped the link")
            .expect("the answer is received");
        assert_eq!(Codec::Json.decode_frame(&buffer[..len]).expect("the answer decodes"), unavailable(7));
    }

    /// A refusal the socket's send buffer had no room for is queued on the writer for the
    /// request's sender; one the socket sent, or failed for another reason, is not.
    #[test]
    fn a_refusal_the_socket_has_no_room_for_is_queued_on_the_writer() {
        let peer: SocketAddr = (Ipv4Addr::LOCALHOST, 9).into();
        let bytes = Codec::Json.encode_frame(&unavailable(7)).expect("the refusal encodes");
        let (writes, mut queued) = ordered::channel(WRITE_QUEUE);
        queue_unsent(Err(io::ErrorKind::WouldBlock.into()), &writes, bytes.clone(), peer);
        let datagram = queued.recv().now_or_never().flatten().expect("the refusal the socket had no room for was not queued");
        assert_eq!((datagram.bytes, datagram.to), (bytes.clone(), Some(peer)), "the queued refusal");
        for sent in [Ok(bytes.len()), Err(io::ErrorKind::ConnectionRefused.into())] {
            let outcome = format!("{sent:?}");
            queue_unsent(sent, &writes, bytes.clone(), peer);
            assert!(queued.recv().now_or_never().flatten().is_none(), "a refusal was queued after the socket's send answered {outcome}");
        }
    }

    #[test]
    fn drain_sends_everything_queued_when_sends_complete() {
        let (writes, mut queued) = ordered::channel(4);
        for _ in 0..3 {
            drop(writes.send(datagram()));
        }
        let clock = Scripted::new();
        let mut sent = 0;
        let drained = drain_queued(&mut queued, &clock, Duration::from_secs(5), |_| {
            sent += 1;
            std::future::ready(())
        })
        .now_or_never();
        assert!(drained.is_some(), "the drain waited with every send complete");
        assert_eq!(sent, 3);
    }
}
