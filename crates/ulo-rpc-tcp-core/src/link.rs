use std::collections::HashMap;
use std::future::{Future, pending};
use std::io;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_channel::{mpsc, oneshot};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::future::{Either, select};
use futures_util::stream::FuturesUnordered;
use futures_util::{AsyncReadExt, AsyncWriteExt, FutureExt, SinkExt, StreamExt, select_biased};
use ulo::{BoundAddr, BoxError, BoxFuture, Runtime, Spawn, TaskHandle};
use ulo_net::rustls::ServerConfig;
use ulo_net::{Activation, ActivationError, BoundListener, Endpoint, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Outbound, Pattern, ReplyPath, ReplyTo};
use ulo_transport::__private::{Watch, ordered};
use ulo_transport::{Details, ErrorKind, TaskSet};

/// The largest frame unless `max_frame` sets one: twice HTTP's default body limit, room for a
/// payload HTTP accepts unset with its envelope and its JSON encoding's growth.
pub const DEFAULT_MAX_FRAME: u64 = 4 * 1024 * 1024;

/// How long the accept loop waits after the OS refuses an accept, out of descriptors or memory,
/// before it accepts again.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// Frames queued for one connection's writer ahead of the next send's wait: each send is queued at
/// its call and waits until it is among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// Deliveries queued for the server; a full queue holds each connection's reader back.
const DELIVERY_QUEUE: usize = 1024;

/// Reply frames read ahead of the client; a full queue holds the connection's reader back.
const REPLY_QUEUE: usize = 64;

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const CLOSED: u8 = 2;

/// One direction of a connection, on `futures-io`'s traits: what a link's sockets, plain or TLS,
/// are handed to the core as.
pub type Reader = Pin<Box<dyn AsyncRead + Send>>;
pub type Writer = Pin<Box<dyn AsyncWrite + Send>>;

/// A link's listener, adopted from the socket `ulo-net` bound or inherited.
pub trait Accept: Send + 'static {
    /// The next connection. A failure is logged and retried after a pause, as the OS refusing an
    /// accept for want of descriptors or memory is.
    fn accept(&mut self) -> impl Future<Output = io::Result<Incoming>> + Send;
}

/// An accepted connection: the caller's address and the handshake that yields its two halves,
/// run on the connection's own task. A plain connection's is ready at once; a TLS connection's is
/// its TLS handshake, whose failure is logged at `debug` and drops the connection.
pub struct Incoming {
    peer: SocketAddr,
    open: BoxFuture<'static, io::Result<(Reader, Writer)>>,
}

impl Incoming {
    pub fn plain(peer: SocketAddr, reader: Reader, writer: Writer) -> Incoming {
        Incoming { peer, open: Box::pin(std::future::ready(Ok((reader, writer)))) }
    }

    pub fn handshaking<F>(peer: SocketAddr, handshake: F) -> Incoming
    where
        F: Future<Output = io::Result<(Reader, Writer)>> + Send + 'static,
    {
        Incoming { peer, open: Box::pin(handshake) }
    }
}

/// A TCP link's settings and running state. Each link holds one and adds its sockets and its
/// runtime.
///
/// Dropped without `close`, it aborts the server's accept loop and forwarding task, so the
/// listener closes, the connections the server accepted close, and the address can be bound again.
pub struct Core {
    endpoint: EndpointSpec,
    tls: Option<Tls>,
    codec: Codec,
    max_frame: Option<u64>,
    /// Set by `prepare`.
    prepared: Option<Prepared>,
    state: Arc<State>,
    /// The accept loop, which holds the listener; set while the server listens. It is held here
    /// and not in `State`, which the loop holds itself, so the link's drop reaches it.
    accept: Mutex<Option<Owned>>,
    /// The forwarding task, which `close` leaves running to hand over or refuse what is still
    /// queued, and which ends once the accept loop and every connection task have.
    forward: Mutex<Option<Owned>>,
}

/// A task only the link holds, aborted when dropped: a `TaskHandle` detaches on drop. `close`
/// awaits the accept loop through it, and the abort then finds the task ended and does nothing.
struct Owned(TaskHandle);

impl Drop for Owned {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Prepared {
    endpoint: Endpoint,
    tls: Option<Arc<ServerConfig>>,
}

/// The running state, shared with the tasks.
struct State {
    bound: Mutex<Vec<BoundAddr>>,
    /// Each open connection's writer queue, for `goaway`.
    connections: Mutex<HashMap<u64, ordered::Sender<Bytes>>>,
    phase: Watch<u8>,
    next_connection: AtomicU64,
    next_id: AtomicU64,
    /// Calls in flight across every connection: ids mapped and not yet forgotten.
    held: Watch<usize>,
    /// Advanced by `close`; a client connection ends when it changes.
    client_epoch: Watch<u64>,
    /// The writer of each client connection, which `close` waits for once it has shut the socket.
    client_writers: Mutex<FuturesUnordered<TaskHandle>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Core {
    pub fn new(endpoint: EndpointSpec) -> Core {
        Core {
            endpoint,
            tls: None,
            codec: Codec::Json,
            max_frame: None,
            prepared: None,
            accept: Mutex::new(None),
            forward: Mutex::new(None),
            state: Arc::new(State {
                bound: Mutex::new(Vec::new()),
                connections: Mutex::new(HashMap::new()),
                phase: Watch::new(RUNNING),
                next_connection: AtomicU64::new(0),
                next_id: AtomicU64::new(1),
                held: Watch::new(0),
                client_epoch: Watch::new(0),
                client_writers: Mutex::new(FuturesUnordered::new()),
            }),
        }
    }

    pub fn set_tls(&mut self, tls: Tls) {
        self.tls = Some(tls);
    }

    pub fn set_codec(&mut self, codec: Codec) {
        self.codec = codec;
    }

    pub fn set_max_frame(&mut self, bytes: u64) {
        self.max_frame = Some(bytes);
    }

    /// The largest frame read or written: `max_frame`, 4 MiB unset, bounded by the 4-byte prefix.
    pub fn limit(&self) -> u64 {
        self.max_frame.unwrap_or(DEFAULT_MAX_FRAME).min(u64::from(u32::MAX))
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
            .binary(self.codec.binary())
            .max_frame(Some(self.limit()))
            .ordering(ulo_rpc::Ordering::PerConnection)
            .miss_signal(true)
            .ordered_control(true)
    }

    /// Everything that can fail before a socket, reported together with `usable`, the link's own
    /// refusal: the endpoint, an inherited socket, the TLS configuration, a zero `max_frame`.
    pub fn prepare(&mut self, usable: Result<(), BoxError>) -> Result<(), BoxError> {
        let mut problems = Vec::new();
        if let Err(error) = usable {
            problems.push(error.to_string());
        }
        let endpoint = match self.endpoint.resolve() {
            Ok(endpoint) => Some(endpoint),
            Err(error) => {
                problems.push(error.to_string());
                None
            }
        };
        if let Some(Endpoint::Inherited(name)) = &endpoint {
            match Activation::get() {
                Ok(activation) if activation.contains(name) => {}
                Ok(_) => problems.push(ActivationError::Missing(name.clone()).to_string()),
                Err(error) => problems.push(error.to_string()),
            }
        }
        let tls = match &self.tls {
            Some(tls) => match tls.load(&[]) {
                Ok(config) => Some(config),
                Err(error) => {
                    problems.push(error.to_string());
                    None
                }
            },
            None => None,
        };
        if self.max_frame == Some(0) {
            problems.push("`.max_frame(0)` would refuse every frame".to_owned());
        }
        match (endpoint, problems.as_slice()) {
            (Some(endpoint), []) => {
                self.prepared = Some(Prepared { endpoint, tls });
                Ok(())
            }
            (_, [problem]) => Err(problem.clone().into()),
            (_, problems) => Err(format!("{} problems:\n  - {}", problems.len(), problems.join("\n  - ")).into()),
        }
    }

    /// Binds the prepared endpoint, hands the socket and the TLS configuration to `adopt`, which
    /// answers the link's listener, and accepts on it from a task on `runtime` until the drain.
    pub fn listen<A, F>(&self, runtime: Arc<dyn Runtime>, adopt: F) -> Result<Inbound, BoxError>
    where
        A: Accept,
        F: FnOnce(BoundListener, Option<Arc<ServerConfig>>) -> io::Result<A>,
    {
        let Some(prepared) = &self.prepared else {
            return Err("the TCP link listens once `prepare` has resolved its endpoint".into());
        };
        let mut accept = lock(&self.accept);
        if accept.is_some() {
            return Err("the TCP link is already listening".into());
        }
        let listener = BoundListener::bind(&prepared.endpoint)?;
        let local = listener.local_addr();
        let listener = adopt(listener, prepared.tls.clone())?;
        *lock(&self.state.bound) = vec![BoundAddr::new("rpc", local).tls(prepared.tls.is_some())];
        let (deliveries, received) = mpsc::channel(DELIVERY_QUEUE);
        let (forwarded, inbound) = mpsc::channel(DELIVERY_QUEUE);
        let server = Server { codec: self.codec, limit: self.limit(), state: Arc::clone(&self.state), runtime: Arc::clone(&runtime) };
        *accept = Some(Owned(runtime.spawn(Box::pin(server.accept(listener, deliveries)))));
        *lock(&self.forward) = Some(Owned(runtime.spawn(Box::pin(forward(Arc::clone(&self.state), received, forwarded, Arc::clone(&runtime))))));
        Ok(Box::pin(inbound))
    }

    /// The address a client connects to, as written: under `ulo dev`, resolving would turn the
    /// address of the socket the command holds into that socket, which a client cannot connect
    /// through. Text is parsed as an IP address and port, and a host name is refused: the link
    /// resolves none. The client side speaks plain TCP: a link with `tls` set is refused. Each
    /// refusal is permanent, so a link's `usable` reports it where the client takes the link.
    pub fn client_addr(&self) -> Result<SocketAddr, BoxError> {
        let endpoint = self
            .endpoint
            .resolve_as_written()
            .map_err(|error| format!("the TCP link takes IP addresses only and resolves no host name: {error}"))?;
        let Endpoint::Addr(addr) = endpoint else {
            return Err(format!("a TCP client connects to an address; {endpoint} is a socket a server listens on").into());
        };
        if self.tls.is_some() {
            return Err("the TCP link's client side speaks no TLS; `Tcp::tls` sets a server's certificate".into());
        }
        Ok(addr)
    }

    /// A client connection over the two halves of a connected socket: its writer and its reply
    /// lane's reader as tasks on `runtime`, both ending at `close`.
    pub fn connect(&self, runtime: &Arc<dyn Runtime>, reader: Reader, writer: Writer) -> Outbound {
        let state = &self.state;
        let at = state.client_epoch.read(|epoch| *epoch);
        let (queue, queued) = ordered::channel(WRITE_QUEUE);
        {
            let mut writers = lock(&state.client_writers);
            while let Some(Some(_)) = writers.next().now_or_never() {}
            writers.push(runtime.spawn(Box::pin(write_frames(writer, queued, closed(Arc::clone(state), at)))));
        }
        let (codec, limit) = (self.codec, self.limit());
        let (frames, replies) = mpsc::channel(REPLY_QUEUE);
        let (lane, dropped) = oneshot::channel::<()>();
        drop(runtime.spawn(Box::pin(read_replies(reader, Arc::clone(state), at, codec, limit, frames, lane))));
        // Queued at the call, so frames go out in the order their sends were called.
        let send = Box::new(move |_pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
            let queued = encode(codec, limit, &frame).map(|bytes| queue.send(bytes));
            Box::pin(async move { queued?.await.map_err(|_| BoxError::from("the TCP connection closed")) })
        });
        // The lane's reader ends once the stream holding `dropped` is dropped.
        let replies = replies.map(move |frame| {
            let _ = &dropped;
            frame
        });
        Outbound { send, replies: Box::pin(replies) }
    }

    /// Sends `goaway` on every connection; one registered afterwards sends its own.
    pub async fn drain(&self) {
        self.state.phase.modify(|phase| advance(phase, DRAINING));
        let Ok(goaway) = self.codec.encode_frame(&Frame::Goaway) else { return };
        let queues: Vec<ordered::Sender<Bytes>> = lock(&self.state.connections).values().cloned().collect();
        for queue in queues {
            // A connection whose queue is full is busy writing replies; its caller learns of the
            // drain from the `unavailable` its next call is answered with.
            if queue.try_send(goaway.clone()).is_err() {
                tracing::debug!("a TCP connection's write queue was full at the drain; it received no `goaway`");
            }
        }
    }

    /// Stops the accept loop and every server connection, and ends every client connection: its
    /// socket shut, its reply lane ended.
    pub async fn close(&self) {
        self.state.phase.modify(|phase| advance(phase, CLOSED));
        self.state.client_epoch.modify(|epoch| *epoch += 1);
        let accept = lock(&self.accept).take();
        if let Some(mut accept) = accept {
            let _ = (&mut accept.0).await;
        }
        // The connection tasks are gone; each writer ends, shutting its half of the socket, once
        // its queue's last sender drops, and this map holds one for every connection whose task
        // was aborted before it could remove its own.
        lock(&self.state.connections).clear();
        let mut writers = std::mem::take(&mut *lock(&self.state.client_writers));
        while writers.next().await.is_some() {}
    }

    pub fn bound(&self) -> Vec<BoundAddr> {
        lock(&self.state.bound).clone()
    }
}

fn advance(phase: &mut u8, to: u8) {
    if *phase < to {
        *phase = to;
    }
}

/// Resolves once the client epoch has moved past `at`.
async fn closed(state: Arc<State>, at: u64) {
    state.client_epoch.wait_for(|epoch| *epoch != at).await;
}

/// Resolves once the phase has reached `at`.
async fn reached(state: &State, at: u8) {
    state.phase.wait_for(|phase| *phase >= at).await;
}

/// Reads one client connection's reply frames into `frames` until the connection ends, the link
/// closes, or the reply lane is dropped.
async fn read_replies(
    mut reader: Reader,
    state: Arc<State>,
    at: u64,
    codec: Codec,
    limit: u64,
    mut frames: mpsc::Sender<Frame>,
    mut lane: oneshot::Sender<()>,
) {
    loop {
        let read = {
            let read = read_frame(&mut reader, limit).fuse();
            let ended = closed(Arc::clone(&state), at).fuse();
            let dropped = lane.cancellation().fuse();
            let (mut read, mut ended, mut dropped) = (pin!(read), pin!(ended), pin!(dropped));
            select_biased! {
                read = read => read,
                () = ended => return,
                () = dropped => return,
            }
        };
        let bytes = match read {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(%error, "the TCP connection's reply lane ended");
                return;
            }
        };
        match codec.decode_frame(&bytes) {
            Ok(frame) => {
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "a reply frame did not decode; the TCP connection is dropped");
                return;
            }
        }
    }
}

/// What every connection task of one server reads.
#[derive(Clone)]
struct Server {
    codec: Codec,
    limit: u64,
    state: Arc<State>,
    /// The link's runtime, which every connection's tasks are spawned on.
    runtime: Arc<dyn Runtime>,
}

impl Server {
    /// Accepts until the drain, then holds the connections until they end or the link closes. The
    /// inbound stream ends once this task and every connection task have dropped their senders.
    async fn accept<A: Accept>(self, mut listener: A, deliveries: mpsc::Sender<Delivery>) {
        let mut connections = TaskSet::new(Arc::clone(&self.runtime) as Arc<dyn Spawn>);
        let mut draining = pin!(reached(&self.state, DRAINING).fuse());
        loop {
            let accepted = {
                let accepted = listener.accept().fuse();
                let joined = async {
                    if connections.is_empty() {
                        pending::<()>().await;
                    }
                    connections.join_next().await;
                }
                .fuse();
                let (mut accepted, mut joined) = (pin!(accepted), pin!(joined));
                select_biased! {
                    () = draining => break,
                    accepted = accepted => Some(accepted),
                    () = joined => None,
                }
            };
            match accepted {
                Some(Ok(incoming)) => connections.spawn(self.clone().connection(incoming, deliveries.clone())),
                Some(Err(error)) => {
                    tracing::warn!(%error, "the TCP link could not accept a connection");
                    self.runtime.sleep(ACCEPT_BACKOFF).await;
                }
                None => {}
            }
        }
        drop(listener);
        drop(deliveries);
        let mut closing = pin!(reached(&self.state, CLOSED).fuse());
        loop {
            let finished = {
                let joined = connections.join_next().fuse();
                let mut joined = pin!(joined);
                select_biased! {
                    () = closing => None,
                    finished = joined => Some(finished),
                }
            };
            match finished {
                None => {
                    connections.abort_all();
                    connections.join_all().await;
                    return;
                }
                Some(None) => return,
                Some(Some(_)) => {}
            }
        }
    }

    async fn connection(self, incoming: Incoming, deliveries: mpsc::Sender<Delivery>) {
        let peer = incoming.peer;
        match incoming.open.await {
            Ok((reader, writer)) => self.serve(reader, writer, peer, deliveries).await,
            Err(error) => tracing::debug!(%error, %peer, "a TLS handshake on the TCP link failed"),
        }
    }

    /// Reads one connection's frames until it closes, then cancels its calls in flight.
    async fn serve(self, mut reader: Reader, writer: Writer, peer: SocketAddr, mut deliveries: mpsc::Sender<Delivery>) {
        let (queue, queued) = ordered::channel(WRITE_QUEUE);
        // The writer runs apart from this task: the server's calls hold reply paths into its queue
        // after the connection's reader has ended, and it ends when the last of them drops.
        drop(self.runtime.spawn(Box::pin(write_frames(writer, queued, pending()))));
        let connection = self.state.next_connection.fetch_add(1, Ordering::Relaxed);
        lock(&self.state.connections).insert(connection, queue.clone());
        // A connection accepted before the drain and registered after it, its TLS handshake
        // finishing late, missed the drain's `goaway`: it sends its own. The drain advances the
        // phase before it reads this map, so one of the two always reaches it.
        if self.state.phase.read(|phase| *phase >= DRAINING)
            && let Ok(goaway) = self.codec.encode_frame(&Frame::Goaway)
        {
            let _ = queue.try_send(goaway);
        }
        let ids = Arc::new(Mutex::new(Ids::new(Arc::clone(&self.state))));
        let path = reply_path(queue, self.codec, self.limit, Arc::clone(&ids), peer);
        loop {
            let bytes = match read_frame(&mut reader, self.limit).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(%error, %peer, "a TCP connection ended");
                    break;
                }
            };
            let frame = match self.codec.decode_frame(&bytes) {
                Ok(frame) => frame,
                Err(error) => {
                    // Without its id nothing can be answered: the connection is dropped.
                    tracing::warn!(%error, %peer, "a frame did not decode; the TCP connection is dropped");
                    break;
                }
            };
            let Some(frame) = self.inward(frame, &ids) else { continue };
            let cancels = matches!(frame, Frame::Cancel { .. });
            let delivery = Delivery { frame, reply: Some(path.clone()), ack: Ack::none() };
            let sent = deliveries.send(delivery).await;
            if cancels {
                self.state.held.modify(|held| *held = held.saturating_sub(1));
            }
            if sent.is_err() {
                break;
            }
        }
        lock(&self.state.connections).remove(&connection);
        let orphaned = lock(&ids).drain();
        let count = orphaned.len();
        for id in orphaned {
            let _ = deliveries.send(Delivery { frame: Frame::Cancel { id }, reply: None, ack: Ack::none() }).await;
        }
        // Released once their `cancel`s are queued, so the inbound stream cannot end before them.
        self.state.held.modify(|held| *held = held.saturating_sub(count));
    }

    /// A caller's frame with its id mapped to the server's; `None` for one the server takes no
    /// part in, or that names no call in flight.
    fn inward(&self, frame: Frame, ids: &Mutex<Ids>) -> Option<Frame> {
        let mut ids = lock(ids);
        Some(match frame {
            Frame::Req { id, pattern, headers, data } => {
                Frame::Req { id: ids.open(id, &self.state.next_id), pattern, headers, data }
            }
            Frame::Open { id, pattern, headers } => Frame::Open { id: ids.open(id, &self.state.next_id), pattern, headers },
            Frame::Evt { pattern, headers, data } => Frame::Evt { pattern, headers, data },
            Frame::In { id, data } => Frame::In { id: ids.server(id)?, data },
            Frame::InEnd { id } => Frame::InEnd { id: ids.server(id)? },
            Frame::Cancel { id } => Frame::Cancel { id: ids.cancel(id)? },
            _ => return None,
        })
    }
}

/// Hands the connections' deliveries to the server until the draining server holds no call, then
/// ends the inbound stream and answers what arrives afterwards itself: a request or streamed
/// request `err` of kind `unavailable`, which also forgets its id. Ending sooner would drop an
/// item or a `cancel` for a call still held; ending once every connection has closed would hold
/// the drain to its deadline while a client keeps an idle connection open. A call's count is
/// released only after its last delivery is queued here, and this reads the queue before it looks
/// at the count, so nothing queued before the count reached zero is answered by the link.
async fn forward(state: Arc<State>, mut received: mpsc::Receiver<Delivery>, forwarded: mpsc::Sender<Delivery>, runtime: Arc<dyn Runtime>) {
    let mut forwarded = Some(forwarded);
    let mut idle = pin!(idle(&state).fuse());
    loop {
        let Some(sender) = forwarded.as_mut() else {
            let Some(delivery) = received.next().await else { return };
            refuse(delivery, &runtime);
            continue;
        };
        let next = {
            let mut delivery = received.next();
            select_biased! {
                delivery = delivery => Some(delivery),
                () = idle => None,
            }
        };
        match next {
            Some(Some(delivery)) => {
                if std::future::poll_fn(|cx| sender.poll_ready(cx)).await.is_err() {
                    forwarded = None;
                    refuse(delivery, &runtime);
                } else if let Err(unsent) = sender.try_send(delivery) {
                    forwarded = None;
                    refuse(unsent.into_inner(), &runtime);
                }
            }
            Some(None) => return,
            None => forwarded = None,
        }
    }
}

/// Resolves once the link is draining and holds no call.
async fn idle(state: &State) {
    reached(state, DRAINING).await;
    state.held.wait_for(|held| *held == 0).await;
}

/// Answers a request or streamed request the server will not receive as the server answers one
/// during the drain; anything else names no call in flight and is dropped.
fn refuse(delivery: Delivery, runtime: &Arc<dyn Runtime>) {
    let (Frame::Req { id, .. } | Frame::Open { id, .. }) = delivery.frame else { return };
    let Some(reply) = delivery.reply else { return };
    let error = ErrorBody::new(ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE, Details::new());
    drop(runtime.spawn(Box::pin(async move {
        if let Err(error) = reply.send(Frame::Err { id, error }).await {
            tracing::debug!(%error, "the TCP link could not refuse a request that arrived after the drain");
        }
    })));
}

/// One connection's calls in flight: the caller's id and the server's, both ways, counted in
/// `State::held`.
struct Ids {
    by_caller: HashMap<u64, u64>,
    by_server: HashMap<u64, u64>,
    state: Arc<State>,
}

impl Ids {
    fn new(state: Arc<State>) -> Self {
        Ids { by_caller: HashMap::new(), by_server: HashMap::new(), state }
    }

    /// Applies the change in calls in flight since `before`.
    fn counted(&self, before: usize) {
        let after = self.by_server.len();
        if after != before {
            self.state.held.modify(|held| *held = (*held + after).saturating_sub(before));
        }
    }

    fn open(&mut self, caller: u64, next: &AtomicU64) -> u64 {
        let before = self.by_server.len();
        let server = next.fetch_add(1, Ordering::Relaxed);
        if let Some(stale) = self.by_caller.insert(caller, server) {
            self.by_server.remove(&stale);
        }
        self.by_server.insert(server, caller);
        self.counted(before);
        server
    }

    fn server(&self, caller: u64) -> Option<u64> {
        self.by_caller.get(&caller).copied()
    }

    /// The caller cancelled: its call is no longer answered. Released once the `cancel` is
    /// queued for the server, by the connection's reader.
    fn cancel(&mut self, caller: u64) -> Option<u64> {
        let server = self.by_caller.remove(&caller)?;
        self.by_server.remove(&server);
        Some(server)
    }

    fn caller(&self, server: u64) -> Option<u64> {
        self.by_server.get(&server).copied()
    }

    fn finish(&mut self, server: u64) {
        let before = self.by_server.len();
        if let Some(caller) = self.by_server.remove(&server) {
            self.by_caller.remove(&caller);
        }
        self.counted(before);
    }

    /// Every server id still in flight, forgotten; the reader releases them once their `cancel`s
    /// are queued.
    fn drain(&mut self) -> Vec<u64> {
        self.by_caller.clear();
        self.by_server.drain().map(|(server, _)| server).collect()
    }
}

/// Replies to one connection: the server's id written back as the caller's, the mapping forgotten
/// once a reply ends the call.
fn reply_path(queue: ordered::Sender<Bytes>, codec: Codec, limit: u64, ids: Arc<Mutex<Ids>>, peer: SocketAddr) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        // Queued at the call, so replies go out in the order their sends were called.
        let queued = queue_reply(&queue, codec, limit, &ids, frame);
        Box::pin(async move { queued?.await.map_err(|_| BoxError::from("the TCP connection closed")) })
    })
    .peer(peer)
}

/// `frame` under the caller's id, queued now; the mapping is forgotten once a reply ends the
/// call.
fn queue_reply(queue: &ordered::Sender<Bytes>, codec: Codec, limit: u64, ids: &Mutex<Ids>, frame: Frame) -> Result<ordered::Room<Bytes>, BoxError> {
    let Some(server) = frame.id() else {
        return Err("a reply frame carries no id".into());
    };
    let Some(caller) = lock(ids).caller(server) else {
        return Err("the call is no longer in flight on this connection".into());
    };
    let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
    let bytes = encode(codec, limit, &with_id(frame, caller))?;
    let room = queue.send(bytes);
    if ends {
        lock(ids).finish(server);
    }
    Ok(room)
}

/// A reply frame under the caller's id.
fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        other => other,
    }
}

/// One frame's bytes, refused with `FrameTooLarge` over `limit` before anything is written.
fn encode(codec: Codec, limit: u64, frame: &Frame) -> Result<Bytes, BoxError> {
    let bytes = codec.encode_frame(frame)?;
    let size = bytes.len() as u64;
    if size > limit {
        return Err(Box::new(FrameTooLarge { size, limit }));
    }
    Ok(bytes)
}

/// One length-prefixed frame; `None` at a clean end between frames. A prefix over `limit` fails
/// before the body is read.
async fn read_frame(reader: &mut Reader, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = [0u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u64::from(u32::from_be_bytes(prefix));
    if len > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes is over the limit of {limit} bytes")));
    }
    let mut body = vec![0; len as usize];
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Writes each queued frame behind its length prefix until the queue's senders are gone or `stop`
/// resolves, then shuts the writer.
async fn write_frames(mut writer: Writer, mut queued: ordered::Receiver<Bytes>, stop: impl Future<Output = ()>) {
    let mut stop = pin!(stop);
    loop {
        let bytes = match select(pin!(queued.recv()), stop.as_mut()).await {
            Either::Left((Some(bytes), _)) => bytes,
            Either::Left((None, _)) | Either::Right(((), _)) => break,
        };
        let prefix = (bytes.len() as u32).to_be_bytes();
        let written = async {
            writer.write_all(&prefix).await?;
            writer.write_all(&bytes).await?;
            writer.flush().await
        };
        if written.await.is_err() {
            return;
        }
    }
    let _ = writer.close().await;
}
