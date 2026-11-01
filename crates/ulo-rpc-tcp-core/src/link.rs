use std::collections::HashMap;
use std::future::{Future, pending};
use std::io;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use futures_channel::{mpsc, oneshot};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::future::{Either, join, select};
use futures_util::stream::FuturesUnordered;
use futures_util::{AsyncReadExt, AsyncWriteExt, FutureExt, SinkExt, StreamExt, select_biased};
use ulo::{BoundAddr, BoxError, BoxFuture, Runtime, Spawn, TaskHandle, Timer};
use ulo_net::rustls::ServerConfig;
use ulo_net::{Activation, ActivationError, BoundListener, Endpoint, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, NoTimer, Outbound, Pattern, ReplyPath, ReplyTo};
use ulo_transport::__private::{Watch, ordered};
use ulo_transport::{Details, ErrorKind, TaskSet};

/// The largest frame unless `max_frame` sets one: twice HTTP's default body limit, room for a
/// payload HTTP accepts unset with its envelope and its JSON encoding's growth.
pub const DEFAULT_MAX_FRAME: u64 = 4 * 1024 * 1024;

/// How long the accept loop waits after the OS refuses an accept, out of descriptors or memory,
/// before it accepts again. A runtime that refuses the wait, having shut down, ends the loop as the
/// drain does. The value is the current one, not a stability promise.
pub const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// Frames queued for one connection's writer ahead of the next send's wait: each send is queued at
/// its call and waits until it is among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// Deliveries queued for the server; a full queue holds each connection's reader back.
const DELIVERY_QUEUE: usize = 1024;

/// Reply frames read ahead of the client; a full queue holds the connection's reader back.
const REPLY_QUEUE: usize = 64;

/// Reads a connection makes between the turns it gives the runtime's other tasks.
const READS_PER_TURN: usize = 32;

/// How long, at most, a connection outlives the end of a `close`, completed or cut, the link's
/// drop, or, on a client, its reply lane's drop: its writer delivering what was queued before the
/// end, its reader discarding what the peer still sends (transports DESIGN §5.2, "A cut
/// `close`"). Measured with the app's `Timer` on a server, and on a client with the clock of the
/// runtime its client hands `connect`, the app's inside an app. A clock that refuses the grace,
/// its runtime shut down, bounds no linger: the connection drops at once.
pub const CUT_GRACE: Duration = Duration::from_secs(2);

const RUNNING: u8 = 0;
const DRAINING: u8 = 1;
const CLOSED: u8 = 2;

/// One direction of a connection, on `futures-io`'s traits: what a link's sockets, plain or TLS,
/// are handed to the core as.
pub type Reader = Pin<Box<dyn AsyncRead + Send>>;
pub type Writer = Pin<Box<dyn AsyncWrite + Send>>;

/// A link's listener, adopted from the socket `ulo-net` bound or inherited.
pub trait Accept: Send + 'static {
    /// The next connection. A failure is logged and retried after [`ACCEPT_BACKOFF`], as the OS
    /// refusing an accept for want of descriptors or memory is.
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
/// listener closes, the connections the server accepted end as at `close`, and the address can be
/// bound again; the connections its client side opened end as at `close` too, their grace counted
/// from the drop.
pub struct Core {
    endpoint: EndpointSpec,
    tls: Option<Tls>,
    codec: Codec,
    max_frame: Option<u64>,
    /// Set by `prepare`.
    prepared: Option<Prepared>,
    /// The app's clock, handed over in `prepare`, which times each server connection's linger.
    timer: Option<Arc<dyn Timer>>,
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
    /// Advanced as `close` begins and at the link's drop; a client connection ends when it
    /// changes.
    client_epoch: Watch<u64>,
    /// The highest epoch of a `close` that has ended, completed or cut, and `u64::MAX` once the
    /// link is dropped: a client connection's deadline runs from the end of a `close` begun after
    /// it opened, a server connection's from the end of the first `close`.
    ended: Watch<u64>,
    /// Dropped by each client connection's writer once it has shut its half of the socket, which
    /// `close` waits for.
    client_writers: Mutex<FuturesUnordered<oneshot::Receiver<()>>>,
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
            timer: None,
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
                ended: Watch::new(0),
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
    /// `timer` is the app's, `AppHandle::timer`, which bounds each server connection's linger.
    pub fn prepare(&mut self, usable: Result<(), BoxError>, timer: Option<Arc<dyn Timer>>) -> Result<(), BoxError> {
        self.timer = timer;
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
        let Some(timer) = self.timer.clone() else {
            return Err(NoTimer { link: "TCP" }.into());
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
        let server = Server { codec: self.codec, limit: self.limit(), state: Arc::clone(&self.state), runtime: Arc::clone(&runtime), timer };
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
    /// lane's reader as tasks on `runtime`. At `close` the writer shuts its half once the frames
    /// queued before it are written, the reply lane ends, and the connection reads and discards
    /// what the server still sends until the server closes its half, the read fails, or
    /// `CUT_GRACE` on `timer` after `close` ends. The link's drop ends it the same way, and so
    /// does the drop of its reply lane, the grace then counted from the drop: a socket closed with
    /// the server's bytes unread would reset the server mid-write.
    pub fn connect(&self, runtime: &Arc<dyn Runtime>, timer: Arc<dyn Timer>, reader: Reader, writer: Writer) -> Outbound {
        let state = &self.state;
        let at = state.client_epoch.read(|epoch| *epoch);
        let (queue, queued) = ordered::channel(WRITE_QUEUE);
        let (lingering, handed) = oneshot::channel();
        let (released, lane_dropped) = oneshot::channel::<()>();
        {
            let (shut, wrote) = oneshot::channel();
            let mut writers = lock(&state.client_writers);
            while let Some(Some(_)) = writers.next().now_or_never() {}
            writers.push(wrote);
            let end = closed(Arc::clone(state), at);
            let close_ended = close_ended(state, at);
            let ended = async move {
                let released = async {
                    if lane_dropped.await.is_err() {
                        pending::<()>().await;
                    }
                };
                select(pin!(close_ended), pin!(released)).await;
            };
            drop(runtime.spawn(Box::pin(run_connection(writer, queued, end, Some(shut), handed, ended, timer))));
        }
        let (codec, limit) = (self.codec, self.limit());
        let (frames, replies) = mpsc::channel(REPLY_QUEUE);
        let (lane, dropped) = oneshot::channel::<()>();
        drop(runtime.spawn(Box::pin(read_replies(Reading::new(reader), Arc::clone(state), at, codec, limit, frames, lane, lingering, released))));
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
    /// socket shut, its reply lane ended. Each connection's `CUT_GRACE` runs from this future's
    /// end, completed or dropped.
    pub async fn close(&self) {
        self.state.phase.modify(|phase| advance(phase, CLOSED));
        let mut epoch = 0;
        self.state.client_epoch.modify(|at| {
            *at += 1;
            epoch = *at;
        });
        let _ended = Ended { state: &self.state, epoch };
        let accept = lock(&self.accept).take();
        if let Some(mut accept) = accept {
            let _ = (&mut accept.0).await;
        }
        // The connection tasks are gone; each writer ends, shutting its half of the socket, once
        // its queue's last sender drops, and this map holds one for every connection whose task
        // was aborted before it could remove its own.
        lock(&self.state.connections).clear();
        let mut writers = std::mem::take(&mut *lock(&self.state.client_writers));
        // Awaited because each connection's grace runs from this future's end: ended sooner, it would
        // drop a writer still delivering what was queued before the close, and a peer slower than
        // `CUT_GRACE` would read a reset where those frames and the FIN belong.
        while writers.next().await.is_some() {}
    }

    pub fn bound(&self) -> Vec<BoundAddr> {
        lock(&self.state.bound).clone()
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.state.client_epoch.modify(|epoch| *epoch += 1);
        self.state.ended.modify(|ended| *ended = u64::MAX);
    }
}

/// Records, as `close`'s future ends or is dropped, that the `close` begun at `epoch` has ended.
struct Ended<'a> {
    state: &'a State,
    epoch: u64,
}

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        let epoch = self.epoch;
        self.state.ended.modify(|ended| *ended = (*ended).max(epoch));
    }
}

/// Resolves at the end of the first `close` begun once the client epoch was `at`, or at the
/// link's drop.
fn close_ended(state: &Arc<State>, at: u64) -> impl Future<Output = ()> + Send + 'static {
    let state = Arc::clone(state);
    async move { state.ended.wait_for(|ended| *ended > at).await }
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

/// Reads one client connection's reply frames into `frames` until the connection ends, or until
/// the link closes or drops or the reply lane is dropped, when it hands the reader to `lingering`,
/// the connection's writer task, and the reply lane ends. A dropped lane also sends `released`,
/// which starts the connection's grace.
#[allow(clippy::too_many_arguments)]
async fn read_replies(
    mut reading: Reading,
    state: Arc<State>,
    at: u64,
    codec: Codec,
    limit: u64,
    mut frames: mpsc::Sender<Frame>,
    mut lane: oneshot::Sender<()>,
    lingering: oneshot::Sender<Reading>,
    released: oneshot::Sender<()>,
) {
    let end = loop {
        let delivered = async {
            let bytes = match reading.frame(limit).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => return Some(LaneEnd::Lost),
                Err(error) => {
                    tracing::debug!(%error, "the TCP connection's reply lane ended");
                    return Some(LaneEnd::Lost);
                }
            };
            match codec.decode_frame(&bytes) {
                Ok(frame) => frames.send(frame).await.err().map(|_| LaneEnd::Dropped),
                Err(error) => {
                    tracing::warn!(%error, "a reply frame did not decode; the TCP connection is dropped");
                    Some(LaneEnd::Lost)
                }
            }
        }
        .fuse();
        let ended = closed(Arc::clone(&state), at).fuse();
        let dropped = lane.cancellation().fuse();
        let (mut delivered, mut ended, mut dropped) = (pin!(delivered), pin!(ended), pin!(dropped));
        select_biased! {
            () = ended => break LaneEnd::Closed,
            () = dropped => break LaneEnd::Dropped,
            delivered = delivered => if let Some(end) = delivered { break end },
        }
    };
    match end {
        LaneEnd::Lost => {}
        LaneEnd::Closed => {
            let _ = lingering.send(reading);
        }
        LaneEnd::Dropped => {
            let _ = lingering.send(reading);
            let _ = released.send(());
        }
    }
}

/// Why a client connection's reply lane stopped reading.
enum LaneEnd {
    /// The connection ended, or sent a frame that does not decode: nothing is left to read.
    Lost,
    /// The link closed or dropped.
    Closed,
    /// The client dropped the reply lane. Its stream drops the channel's receiver ahead of the
    /// lane's own signal, so a send failing is the same drop seen first. Read as `Lost`, that send
    /// would leave the reader unhanded and a peer still writing would read a reset; no caller reads
    /// either end, the lane being gone. A reader on another worker, between the stream's two drops,
    /// reaches it; `a_reply_lane_whose_send_fails_hands_its_reader_on_and_the_writing_peer_reads_a_fin`
    /// in ulo-rpc-tcp's and ulo-rpc-tcp-smol's `tests/lane_send_failure.rs` pins it, dropping the
    /// lane inside the read that completes a frame.
    Dropped,
}

/// A connection's reading half, which gives the runtime's other tasks a turn every
/// `READS_PER_TURN` reads, whatever becomes of what it reads (F485). `async-io`'s sockets answer
/// a read at once while the peer's bytes keep arriving, and a channel with room answers a send at
/// once, so a loop reading a peer that never stops writing would otherwise keep its thread from
/// every other task: the abort of a server connection, which `close` waits for, lands only when
/// the task yields, and a linger would never poll the deadline it races.
struct Reading {
    reader: Reader,
    reads: usize,
}

impl Reading {
    fn new(reader: Reader) -> Reading {
        Reading { reader, reads: 0 }
    }

    async fn turn(&mut self) {
        self.reads += 1;
        if self.reads.is_multiple_of(READS_PER_TURN) {
            // Unpinned on tokio: its cooperative budget has a read answer `Pending` without it.
            yield_now().await;
        }
    }

    /// One length-prefixed frame; `None` at a clean end between frames. A prefix over `limit`
    /// fails before the body is read.
    async fn frame(&mut self, limit: u64) -> io::Result<Option<Vec<u8>>> {
        self.turn().await;
        let mut prefix = [0u8; 4];
        match self.reader.read_exact(&mut prefix).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        let len = u64::from(u32::from_be_bytes(prefix));
        if len > limit {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes is over the limit of {limit} bytes")));
        }
        let mut body = vec![0; len as usize];
        self.reader.read_exact(&mut body).await?;
        Ok(Some(body))
    }

    /// Reads and discards until the peer closes its half or a read fails.
    async fn discard(mut self) {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            self.turn().await;
            match self.reader.read(&mut buffer).await {
                Ok(read) if read > 0 => {}
                _ => return,
            }
        }
    }
}

/// A server connection's reader, handed to the connection's writer task if the connection's task
/// is dropped while it holds it, as `close` and the link's drop abort it, so the connection reads
/// and discards what the peer still sends until the peer closes its half, the read fails, or
/// `CUT_GRACE` after `close` ends (transports DESIGN §5.2). A socket closed with the peer's bytes unread closes with a
/// reset, which discards what the server wrote and the peer has not read (F484).
struct Lingering {
    /// `None` once `release` has let go of it.
    reading: Option<Reading>,
    handover: Option<oneshot::Sender<Reading>>,
}

impl Lingering {
    fn get(&mut self) -> &mut Reading {
        self.reading.as_mut().expect("the connection's reader is held until its task stops reading")
    }

    /// Drops the reader: the task stopped reading at the connection's end, on a frame it drops the
    /// connection for, or once the server took no more deliveries.
    fn release(&mut self) {
        self.reading = None;
        self.handover = None;
    }
}

impl Drop for Lingering {
    fn drop(&mut self) {
        if let (Some(reading), Some(handover)) = (self.reading.take(), self.handover.take()) {
            let _ = handover.send(reading);
        }
    }
}

async fn yield_now() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
}

/// What every connection task of one server reads.
#[derive(Clone)]
struct Server {
    codec: Codec,
    limit: u64,
    state: Arc<State>,
    /// The link's runtime, which every connection's tasks are spawned on.
    runtime: Arc<dyn Runtime>,
    /// The app's clock, which times each connection's linger.
    timer: Arc<dyn Timer>,
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
                    if let Err(refused) = self.runtime.try_sleep(ACCEPT_BACKOFF).await {
                        tracing::debug!(%refused, "the runtime timing the TCP link's accept backoff refused it; the link stops accepting");
                        break;
                    }
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

    /// Reads one connection's frames until it closes, then cancels its calls in flight. Ended by
    /// the link instead, at `close` or at the link's drop, the connection lingers.
    async fn serve(self, reader: Reader, writer: Writer, peer: SocketAddr, mut deliveries: mpsc::Sender<Delivery>) {
        let (queue, queued) = ordered::channel(WRITE_QUEUE);
        let (handover, handed) = oneshot::channel();
        // Every `close` ends the server's connections, so the first one to end starts the grace,
        // also for a connection whose task first ran after that `close` advanced the epoch and
        // before its abort landed.
        let ended = close_ended(&self.state, 0);
        // The writer runs apart from this task: the server's calls hold reply paths into its queue
        // after the connection's reader has ended, and it ends when the last of them drops.
        drop(self.runtime.spawn(Box::pin(run_connection(writer, queued, pending(), None, handed, ended, Arc::clone(&self.timer)))));
        let mut reader = Lingering { reading: Some(Reading::new(reader)), handover: Some(handover) };
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
            let bytes = match reader.get().frame(self.limit).await {
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
        reader.release();
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

/// One connection's writing half and, once the link ends the connection, its reading half: writes
/// each queued frame behind its length prefix until the queue's senders are gone or `end`
/// resolves, then shuts the writer and drops `shut`; if `lingering` hands the reader over, reads
/// and discards until the peer closes its half or a read fails. `CUT_GRACE` on `timer` after
/// `ended` resolves, or at once there when `timer` refuses the grace, both halves drop wherever
/// they are, which closes the socket.
async fn run_connection(
    writer: Writer,
    mut queued: ordered::Receiver<Bytes>,
    end: impl Future<Output = ()>,
    shut: Option<oneshot::Sender<()>>,
    lingering: oneshot::Receiver<Reading>,
    ended: impl Future<Output = ()>,
    timer: Arc<dyn Timer>,
) {
    let writing = async move {
        let (mut writer, _shut) = (writer, shut);
        let mut end = pin!(end);
        loop {
            let bytes = match select(pin!(queued.recv()), end.as_mut()).await {
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
    };
    let reading = async move {
        if let Ok(reading) = lingering.await {
            reading.discard().await;
        }
    };
    // Declared ahead of the connection, so the halves drop before the sleep does and a clock that
    // sees the sleep go sees the socket closed.
    let mut grace = None;
    let lingered = {
        let mut connection = pin!(join(writing, reading));
        match select(connection.as_mut(), pin!(ended)).await {
            Either::Left(_) => None,
            Either::Right(((), _)) => {
                let sleep = grace.insert(timer.try_sleep(CUT_GRACE));
                match select(connection, sleep.as_mut()).await {
                    Either::Left(_) => None,
                    Either::Right((slept, _)) => Some(slept),
                }
            }
        }
    };
    match lingered {
        None => {}
        Some(Ok(())) => tracing::debug!("a TCP connection was still open `CUT_GRACE` after the link's close; it is dropped"),
        // Timed on no other clock: the grace is the handed runtime's, and that runtime is gone.
        Some(Err(refused)) => tracing::debug!(%refused, "the runtime timing a TCP connection's linger refused its grace; the connection is dropped without one"),
    }
}
