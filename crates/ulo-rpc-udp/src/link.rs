use std::collections::{HashMap, HashSet};
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
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture, Permanent, Refused, Runtime, RuntimeShutDown, Timer};
use ulo_runtime_tokio::Tokio;
use ulo_net::{Endpoint, EndpointSpec, HostPort, Lookup, ResolveCause, ResolveError, Target};
use ulo_rpc::link::{Inbound, UNARY_ONLY};
use ulo_rpc::{
    Ack, Capabilities, Codec, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link, NoTimer, Outbound, Pattern, ReplyPath, ReplyTo, Subscription,
};
use ulo_transport::sync::{Watch, ordered};
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
/// `Unavailable`. A socket that has already failed has told its calls, and its reply lane ends
/// once the client drops that connection. A call made afterwards binds a new socket.
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
/// neither an IP address and port nor a host name and port, and an inherited one.
///
/// A host name, `billing:7000`, is resolved through the system resolver, `ToSocketAddrs`, so
/// `/etc/hosts`, nsswitch and the search list apply, on the held runtime's blocking pool
/// (`Runtime::spawn_blocking`). A server resolves it once, in `prepare`, which fails on a name
/// resolving to no address, and binds the first of its addresses that binds, logging the address
/// bound at `info`. A client resolves it at every connect,
/// each `connect` binding a new socket, and connects its socket to the first address the OS
/// accepts, in the resolver's order, so a later connect follows the name to new addresses; a name
/// resolving to no address fails that connect, the call failing `Unavailable` with the name and
/// the resolver's error.
///
/// # A name resolving to several addresses
///
/// UDP has no handshake, so the OS accepts the first address whatever listens there, and the
/// client sends every datagram to it, as `std::net::UdpSocket::connect` does. `localhost` commonly
/// resolves to `::1` before `127.0.0.1`, so a client on `localhost:7000` never reaches a server
/// bound on `127.0.0.1:7000` alone: where the host answers port unreachable, each call fails
/// `Unavailable` naming the address it sent to and how many addresses the name resolved to, and
/// where nothing answers, `Timeout`. The client does not move to the next address after a
/// refusal, which is an ICMP message firewalls drop, so which server it reached would depend on
/// the network between them. Name the address the server is bound on, `127.0.0.1:7000`, or bind
/// a server on each of the name's addresses, `127.0.0.1:7000` and `[::1]:7000`.
pub struct Udp {
    pub(crate) endpoint: EndpointSpec,
    /// How a host name becomes addresses: the system resolver, or a test's.
    pub(crate) lookup: Lookup,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    /// Set by `prepare`: the address, or each address a host name resolved to, in the resolver's
    /// order.
    pub(crate) prepared: Option<Vec<SocketAddr>>,
    /// The app's clock, taken in `prepare`, which bounds the server writer's last sends.
    timer: Option<Arc<dyn Timer>>,
    pub(crate) state: Arc<State>,
    /// The server's receive task; set while the server listens. It is held here and not in
    /// `State`, which the task holds itself, so the link's drop reaches it.
    receiving: Mutex<Option<Owned>>,
    /// The server socket's writer and its task, which `close` stops once it has queued its
    /// refusals and waits for, so the socket is closed when `close` returns. Dropped with the link,
    /// the task is left to send what is queued.
    writing: Mutex<Option<(Flushed, JoinHandle<()>)>>,
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
    /// Advanced by `close`; a client socket's reply lane ends when it changes, unless the socket
    /// has already failed.
    client_epoch: watch::Sender<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Udp {
    /// The link on `endpoint`: text read in `usable`, where a client takes the link, and in a
    /// server's `prepare`, as an IP address and port, `10.0.0.5:7000` or `[::1]:7000`, or a host
    /// name and port, `billing:7000`; or a `SocketAddr`. An inherited endpoint is refused there
    /// too, since the activation adopts listening TCP sockets alone.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Udp {
            endpoint: endpoint.into(),
            lookup: Lookup::system(),
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

    /// `Codec::Cbor` carries raw bytes and reports `binary: true`; JSON unset.
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
    /// holds as a TCP socket into that socket, which a datagram socket cannot be. Text is an IP
    /// address and port or a host name and port. Each refusal is permanent, so `usable` reports it
    /// where the client takes the link.
    fn target(&self) -> Result<Target, BoxError> {
        let target = self.endpoint.target()?;
        match target {
            Target::Endpoint(Endpoint::Addr(_)) | Target::Host(_) => Ok(target),
            target => Err(format!("{target}: the UDP link takes an address; the activation adopts listening TCP sockets alone").into()),
        }
    }

    /// The endpoint's address, or the addresses its host name resolves to on the link's runtime,
    /// in the resolver's order; looked up again at every call.
    async fn addresses(&self, runtime: &Tokio) -> Result<Vec<SocketAddr>, BoxError> {
        match self.target()? {
            Target::Endpoint(Endpoint::Addr(addr)) => Ok(vec![addr]),
            Target::Host(host) => self.resolve(&host, runtime).await,
            Target::Endpoint(endpoint) => Err(format!("{endpoint}: the UDP link takes an address").into()),
        }
    }

    async fn resolve(&self, host: &HostPort, runtime: &Tokio) -> Result<Vec<SocketAddr>, BoxError> {
        self.lookup.resolve(host, runtime).await.map_err(resolve_failure)
    }
}

/// A runtime that has shut down is reported as one, as every other refusal of the runtime is.
fn resolve_failure(error: ResolveError) -> BoxError {
    match error.cause() {
        ResolveCause::Refused => RuntimeShutDown::new(Refused).into(),
        _ => error.into(),
    }
}

const NO_RUNTIME: &str = "the UDP link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Udp {
    const NAME: &'static str = "udp";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
            .codec(self.codec)
            .max_frame(Some(MAX_DATAGRAM))
            .ordering(ulo_rpc::Ordering::Unordered)
            .shapes(UNARY_ONLY)
            .without_native_backpressure(
                "a UDP server reads each request off its socket as it arrives, and no broker holds one over the server's bound, which the server refuses",
            )
            .without_unserved_hold("the caller sends to one server's address, and no broker keeps a request while that server is not serving")
            .without_durable_replies("a reply is one datagram sent once to the caller's address, and nothing keeps it for a caller that is not there")
            .without_confirmed_drain(
                "`close` answers the requests its socket already holds, but a datagram arriving after the socket closes reaches no one; \
                 a connected client socket may learn of it through an ICMP port unreachable, which is often filtered and is not relied on",
            )
            .without_ordered_control("datagrams arrive in any order, so a `cancel` may overtake its request")
            .without_every_instance("the caller sends to one server's address, so an event reaches that server and no other instance")
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime()?.require_io()?;
        self.target().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        // `listen` binds on the link's runtime: one that has shut down, or has no IO driver, is
        // refused here, before the configuration and before any server binds. The shutdown is
        // asked first, since a runtime that has shut down answers nothing about its driver.
        let runtime = self.runtime()?;
        runtime.run(async {}).await.map_err(Permanent::new)?;
        runtime.require_io().map_err(Permanent::new)?;
        // A host name is resolved here alone, so the server binds one address for its life.
        self.prepared = Some(self.addresses(&runtime).await?);
        self.timer = app.timer().cloned();
        Ok(())
    }

    async fn listen(&self, subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = subscriptions;
        let Some(addrs) = self.prepared.clone() else {
            return Err("the UDP link listens once `prepare` has resolved its endpoint".into());
        };
        if lock(&self.receiving).is_some() {
            return Err("the UDP link is already listening".into());
        }
        let Some(timer) = self.timer.clone() else {
            return Err(NoTimer { link: "UDP" }.into());
        };
        let runtime = self.runtime()?;
        let endpoint = self.target()?;
        // `bind` over a slice binds the first address that binds, as `std::net::UdpSocket::bind` does.
        let socket = Arc::new(runtime.run(async move { UdpSocket::bind(addrs.as_slice()).await }).await.map_err(Permanent::new)??);
        let local = socket.local_addr()?;
        tracing::info!(ulo.transport = "rpc", link = "udp", endpoint = %endpoint, address = %local, "the RPC server listens on `address`");
        *lock(&self.state.bound) = vec![BoundAddr::new("rpc", local)];
        *lock(&self.state.socket) = Some(Arc::clone(&socket));
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        let (dropped, stop) = oneshot::channel::<()>();
        let writer = runtime.handle().spawn(write_server_datagrams(Arc::clone(&socket), queued, stop, timer));
        *lock(&self.writing) = Some((Flushed(Some(dropped)), writer));
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
        let name = match self.target()? {
            Target::Host(host) => Some(host),
            Target::Endpoint(_) => None,
        };
        let runtime = self.runtime()?;
        let addrs = self.addresses(&runtime).await?;
        let resolved = addrs.len();
        let (socket, sent_to) = runtime
            .run(async move {
                let mut last = None;
                for (taken, addr) in addrs.into_iter().enumerate() {
                    let local: SocketAddr = if addr.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
                    let connected = async {
                        let socket = UdpSocket::bind(local).await?;
                        socket.connect(addr).await?;
                        Ok::<_, io::Error>(socket)
                    };
                    match connected.await {
                        Ok(socket) => return Ok((socket, SentTo { addr, name: name.map(|host| Resolved { host, resolved, taken }) })),
                        Err(error) => last = Some(error),
                    }
                }
                Err(last.unwrap_or_else(|| io::Error::other("the UDP link had no address to send to")))
            })
            .await??;
        // The reply lane's reader holds the socket; the writer reaches it through a `Weak`, so the
        // socket is released once the reader returns, at `close` or when the OS reports the server
        // unreachable.
        let socket = Arc::new(socket);
        let codec = self.codec;
        let epoch = self.state.client_epoch.subscribe();
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        runtime.handle().spawn(write_datagrams(Arc::downgrade(&socket), queued, closed(epoch.clone())));
        let awaiting = Arc::new(Mutex::new(Awaiting::default()));
        let refused = Arc::new(Notify::new());
        let (sending, refusing, sending_to) = (Arc::clone(&awaiting), Arc::clone(&refused), sent_to.clone());
        // Queued at the call, so datagrams leave in the order their sends were called.
        let send = Box::new(move |_pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
            let (awaiting, refused, sent_to) = (Arc::clone(&sending), Arc::clone(&refusing), sending_to.clone());
            let sent = encode(codec, &frame).map(|bytes| (lock(&awaiting).sent(&frame), bytes));
            let id = request_id(&frame);
            let queued = match sent {
                Ok((Sent::Queued, bytes)) => write(&writes, bytes, None),
                Ok((Sent::Told(lane, frame), _)) => return Box::pin(async move { lane.send(frame).await.map_err(|_| BoxError::from("the UDP link's reply lane is closed")) }),
                Ok((Sent::Dropped, _)) => return Box::pin(async { Ok(()) }),
                Err(error) => return Box::pin(async move { Err(error) }),
            };
            Box::pin(async move {
                let error = match queued.await {
                    Ok(()) => return Ok(()),
                    Err(error) => error,
                };
                lock(&awaiting).send_failed(id, error, &sent_to, &refused)
            })
        });
        let (frames, replies) = mpsc::channel(REPLY_QUEUE);
        runtime.handle().spawn(read_replies(socket, epoch, codec, frames, awaiting, refused, sent_to));
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
    /// not. It returns once its writer has sent what was queued, within two seconds on the app's
    /// `Timer`, and has let go of the socket: the address can be bound again, and a datagram sent
    /// to it afterwards meets a closed port.
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
        // The writer holds the last handle on the socket: stopped, it sends what is queued, within
        // `CUT_GRACE`, and ends, which closes the socket.
        let writing = lock(&self.writing).take();
        if let Some((stop, writer)) = writing {
            drop(stop);
            let _ = writer.await;
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

/// Where a client socket sends: the address, and the host name it came from.
#[derive(Clone, Debug)]
struct SentTo {
    addr: SocketAddr,
    name: Option<Resolved>,
}

/// A host name, how many addresses it resolved to, and the position of the one taken among them.
#[derive(Clone, Debug)]
struct Resolved {
    host: HostPort,
    resolved: usize,
    taken: usize,
}

impl SentTo {
    /// What a call is told when the socket fails with `error`: the address the datagrams went to,
    /// and for a name resolving to several addresses which one the link took, so the message
    /// itself shows a server bound on another of them, `127.0.0.1` where `localhost` answers `::1`
    /// first.
    fn failure(&self, error: &io::Error) -> String {
        let addr = self.addr;
        let what = if error.kind() == io::ErrorKind::ConnectionRefused {
            format!("the UDP link's datagrams to {addr} were refused (ICMP port unreachable)")
        } else {
            format!("the UDP link's socket sending to {addr} failed: {error}")
        };
        match &self.name {
            Some(Resolved { host, resolved, taken }) if *resolved > 1 => {
                let order = if *taken == 0 { "first".to_owned() } else { format!("as number {}, the first a socket could connect to", taken + 1) };
                format!("{what}; `{host}` resolved to {resolved} addresses and the link took {addr} {order}")
            }
            _ => what,
        }
    }
}

/// The calls a client socket has sent a request for and not seen end, and, once the socket has
/// failed, what each call is told and the reply lane that tells it.
///
/// Every request learns of the failure through the reply lane, after the `goaway` the lane sends
/// first, and never from its own send: the client reads the `goaway` before any failure, so a
/// call made once a request was told the socket failed finds the connection closed and connects
/// again. An event whose send the host refused is told by that send, having no reply to wait for,
/// so a call made right after it may still be sent on the failed socket.
#[derive(Default)]
struct Awaiting {
    ids: HashSet<u64>,
    failed: Option<String>,
    /// Set once the reply lane has sent its `goaway`; a request after it is answered here.
    lane: Option<mpsc::Sender<Frame>>,
}

/// What becomes of a frame handed to a client socket.
enum Sent {
    /// Written to the socket.
    Queued,
    /// A request reaching the socket after the `goaway`: its failure, sent on the reply lane
    /// behind the `goaway`, where its call reads it.
    Told(mpsc::Sender<Frame>, Frame),
    /// An event or a `cancel` reaching the socket after the `goaway`, lost as a datagram to a host
    /// nothing listens on is.
    Dropped,
}

impl Awaiting {
    /// Records `frame` as sent: a request awaits its reply, and a `cancel` ends that wait. Once the
    /// reply lane has sent its `goaway`, nothing more is written to the socket.
    fn sent(&mut self, frame: &Frame) -> Sent {
        if let (Some(lane), Some(failed)) = (&self.lane, &self.failed) {
            return match request_id(frame) {
                Some(id) => Sent::Told(lane.clone(), Frame::err(id, ErrorBody::new(ErrorKind::Unavailable, failed.clone(), Details::new()))),
                None => Sent::Dropped,
            };
        }
        match frame {
            Frame::Req { id, .. } | Frame::Open { id, .. } => {
                self.ids.insert(*id);
            }
            Frame::Cancel { id, .. } => {
                self.ids.remove(id);
            }
            _ => {}
        }
        Sent::Queued
    }

    /// The answer to a send whose datagram did not leave. The host's refusal is reported once, to
    /// the socket's receive or to its next send, whichever reads it first, so a send the host
    /// refused fails the socket as the receive would, `refused` waking the reply lane, and an
    /// event or a `cancel` it refused is answered with the failure, the one report an emitting
    /// client gets. Once the socket has failed, any other send succeeds: a request's call is among
    /// those the lane answers, its id kept for it, and an event or a `cancel` is lost, as a
    /// datagram to a host nothing listens on is. A send failing otherwise before then ends its
    /// call's wait with its own error.
    fn send_failed(&mut self, id: Option<u64>, error: BoxError, sent_to: &SentTo, refused: &Notify) -> Result<(), BoxError> {
        if self.failed.is_none()
            && let Some(io) = error.downcast_ref::<io::Error>()
            && io.kind() == io::ErrorKind::ConnectionRefused
        {
            let failure = sent_to.failure(io);
            self.failed = Some(failure.clone());
            refused.notify_one();
            if id.is_none() {
                return Err(failure.into());
            }
        }
        if self.failed.is_some() {
            return Ok(());
        }
        if let Some(id) = id {
            self.ids.remove(&id);
        }
        Err(error)
    }
}

fn request_id(frame: &Frame) -> Option<u64> {
    match frame {
        Frame::Req { id, .. } | Frame::Open { id, .. } => Some(*id),
        _ => None,
    }
}

/// Reads the client socket's reply datagrams into `frames` until the link closes, the reply lane
/// is dropped, or the OS reports the server unreachable; the socket is released as this returns.
/// A failed socket, its receive failing or a send the host refused waking `refused`, first sends a
/// `goaway`, which closes the connection to new calls, then delivers each reply its receive buffer
/// already holds, since a host may report a pending refusal ahead of a datagram that arrived
/// before it, then answers each call it still awaits `err` of kind `unavailable` naming `sent_to`.
/// The lane stays open for a request reaching the socket after that, which [`Awaiting::sent`]
/// answers the same way, and ends once the connection is dropped.
async fn read_replies(
    socket: Arc<UdpSocket>,
    epoch: watch::Receiver<u64>,
    codec: Codec,
    frames: mpsc::Sender<Frame>,
    awaiting: Arc<Mutex<Awaiting>>,
    refused: Arc<Notify>,
    sent_to: SentTo,
) {
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    loop {
        // Biased so a wake is taken before a datagram ready beside it, which the drain below reads:
        // the order is then the same whichever the host reports first.
        let received = tokio::select! {
            biased;
            () = closed(epoch.clone()) => return,
            () = frames.closed() => return,
            // `Awaiting::send_failed` recorded the failure before the wake.
            () = refused.notified() => Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
            received = socket.recv(&mut buffer) => received,
        };
        // An error here is the OS reporting the server unreachable: the `goaway` below closes the
        // connection, and the next call binds a new socket.
        let len = match received {
            Ok(len) => len,
            Err(error) => {
                tracing::debug!(%error, addr = %sent_to.addr, "the UDP link's client socket failed");
                if frames.send(Frame::Goaway).await.is_err() {
                    return;
                }
                // Recorded after the `goaway` is queued, under the lock `Awaiting::sent` takes, so a
                // request either joins the calls taken below or is answered behind the `goaway`.
                let failure = {
                    let mut awaiting = lock(&awaiting);
                    awaiting.lane = Some(frames.clone());
                    awaiting.failed.get_or_insert_with(|| sent_to.failure(&error)).clone()
                };
                for datagram in already_queued(&socket) {
                    if !deliver(&datagram, codec, &awaiting, &frames).await {
                        return;
                    }
                }
                let ids = std::mem::take(&mut lock(&awaiting).ids);
                for id in ids {
                    let frame = Frame::err(id, ErrorBody::new(ErrorKind::Unavailable, failure.clone(), Details::new()));
                    if frames.send(frame).await.is_err() {
                        return;
                    }
                }
                #[cfg(test)]
                tests::gate::after_failure(sent_to.addr).await;
                return;
            }
        };
        if !deliver(&buffer[..len], codec, &awaiting, &frames).await {
            return;
        }
    }
}

/// Hands a reply datagram to the reply lane, its call's wait ended by a last frame; `false` once
/// the lane is gone. A datagram that does not decode is dropped.
async fn deliver(datagram: &[u8], codec: Codec, awaiting: &Mutex<Awaiting>, frames: &mpsc::Sender<Frame>) -> bool {
    match codec.decode_frame(datagram) {
        Ok(frame) => {
            if matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. })
                && let Some(id) = frame.id()
            {
                lock(awaiting).ids.remove(&id);
            }
            frames.send(frame).await.is_ok()
        }
        Err(error) => {
            tracing::debug!(%error, "a reply datagram did not decode and was dropped");
            true
        }
    }
}

/// The datagrams the socket's receive buffer holds now, read without waiting until it is empty,
/// through a duplicate of the socket as [`final_read`] reads: tokio tries a read only once its
/// reactor has seen the socket readable. A refusal reported again is passed over.
fn already_queued(socket: &UdpSocket) -> Vec<Vec<u8>> {
    let duplicate = match socket2::SockRef::from(socket).try_clone().and_then(|duplicate| {
        duplicate.set_nonblocking(true)?;
        Ok(std::net::UdpSocket::from(duplicate))
    }) {
        Ok(duplicate) => duplicate,
        Err(error) => {
            tracing::debug!(%error, "the UDP link could not read its failed socket's buffer; replies still in it go undelivered");
            return Vec::new();
        }
    };
    let mut buffer = vec![0u8; MAX_DATAGRAM as usize + 1];
    let mut queued = Vec::new();
    loop {
        match duplicate.recv(&mut buffer) {
            Ok(len) => queued.push(buffer[..len].to_vec()),
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused) => continue,
            Err(_) => return queued,
        }
    }
}

/// Every datagram the socket receives, as deliveries, until the link closes. A datagram that does
/// not decode carries no id to answer and is dropped. Once the link is draining and holds no call,
/// the inbound stream ends, and a request arriving afterwards is refused here: nothing tells a UDP
/// caller to stop sending, and the stream ending only at `close` would hold the drain to its
/// deadline. This loop alone sends deliveries and decides the end between datagrams, and a
/// `cancel` releases its call once the server has taken it, so no `cancel` is left behind the
/// stream's end.
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
        let ack = match &frame {
            Frame::Cancel { id, .. } => taken(&ids, *id),
            _ => Ack::none(),
        };
        if sender.send(Delivery { frame, reply: Some(reply), ack }).await.is_err() {
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
    Frame::err(id, ErrorBody::new(ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE, Details::new()))
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
        Frame::Req { id, pattern, headers, data, .. } => Some(Frame::req(ids.open(peer, id, next), pattern, headers, data)),
        Frame::Open { id, pattern, headers, .. } => Some(Frame::open(ids.open(peer, id, next), pattern, headers)),
        Frame::Evt { pattern, headers, data, .. } => Some(Frame::evt(pattern, headers, data)),
        Frame::In { id, data, .. } => ids.server(peer, id).map(|id| Frame::input(id, data)),
        Frame::InEnd { id, .. } => ids.server(peer, id).map(Frame::in_end),
        Frame::Cancel { id, .. } => ids.cancel(peer, id).map(Frame::cancel),
        _ => None,
    };
    mapped.map_or(Inward::Discard, Inward::Deliver)
}

/// The calls in flight, counted in `State::held`: by each sender's id and by the server's while
/// the sender can address them, and by the server's alone once the sender has cancelled, until the
/// server has taken the `cancel`.
struct Ids {
    by_caller: HashMap<(SocketAddr, u64), u64>,
    by_server: HashMap<u64, Caller>,
    /// Set by `forget_all`: a request arriving afterwards is refused, not opened.
    closed: bool,
    state: Arc<State>,
}

impl Ids {
    fn new(state: Arc<State>) -> Self {
        Ids { by_caller: HashMap::new(), by_server: HashMap::new(), closed: false, state }
    }

    /// Every call held and not cancelled, as its sender's address and id, each forgotten, so a
    /// reply to it is refused, and the table closed, so a request arriving afterwards is refused
    /// too. A cancelled call is forgotten with them and answered nothing, the sends waiting on it
    /// answered as at its release.
    fn forget_all(&mut self) -> Vec<(SocketAddr, u64)> {
        self.closed = true;
        self.by_caller.clear();
        let held = self
            .by_server
            .drain()
            .filter_map(|(_, caller)| match caller {
                Caller::Live(peer, caller) => Some((peer, caller)),
                Caller::Cancelled(released) => {
                    released.modify(|released| *released = true);
                    None
                }
            })
            .collect();
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
        self.by_server.insert(server, Caller::Live(peer, caller));
        self.counted();
        server
    }

    fn server(&self, peer: SocketAddr, caller: u64) -> Option<u64> {
        self.by_caller.get(&(peer, caller)).copied()
    }

    /// The sender cancelled: its frames naming the call are dropped from now on, a second
    /// `cancel` among them, and the call's replies are dropped before the wire until
    /// [`release`](Self::release) forgets it, each send answered then. The call stays known until
    /// the server has taken the `cancel`: a reply refused before then would cancel the call
    /// `ReplyFailed` ahead of the `cancel` naming its reason, and one answered at once would let
    /// a stream whose items are always ready run on without yielding, which on a one-thread
    /// runtime keeps the `cancel` from ever arriving.
    fn cancel(&mut self, peer: SocketAddr, caller: u64) -> Option<u64> {
        let server = self.by_caller.remove(&(peer, caller))?;
        self.by_server.insert(server, Caller::Cancelled(Arc::new(Watch::new(false))));
        Some(server)
    }

    fn caller(&self, server: u64) -> Option<Caller> {
        self.by_server.get(&server).cloned()
    }

    /// A reply ended the call. A call its sender has cancelled is left to its `cancel`.
    fn finish(&mut self, server: u64) {
        if let Some(Caller::Live(peer, caller)) = self.by_server.get(&server) {
            self.by_caller.remove(&(*peer, *caller));
            self.by_server.remove(&server);
        }
        self.counted();
    }

    /// Forgets a call its sender cancelled, once the server has taken the `cancel`, and answers
    /// the sends waiting on it.
    fn release(&mut self, server: u64) {
        if let Some(Caller::Cancelled(released)) = self.by_server.remove(&server) {
            released.modify(|released| *released = true);
        }
        self.counted();
    }
}

/// Who a call's replies go to.
#[derive(Clone)]
enum Caller {
    /// The sender, at its address and under its own id.
    Live(SocketAddr, u64),
    /// Nobody: the sender cancelled the call. Its replies are dropped before the wire, each send
    /// answered once the flag is set, as the `cancel`'s `Ack` settles or drops unsettled, or at
    /// `close`.
    Cancelled(Arc<Watch<bool>>),
}

/// The `Ack` of a sender's `cancel` for call `server`, which forgets the call once the server has
/// taken the `cancel`: the server cancels the call before it settles the `cancel`'s `Ack`, and an
/// `Ack` dropped unsettled, its `cancel` dropped untaken, forgets the call as it drops.
fn taken(ids: &Arc<Mutex<Ids>>, server: u64) -> Ack {
    let cancelled = Cancelled { ids: Arc::clone(ids), server };
    Ack::new(move |_| drop(cancelled))
}

/// A cancelled call, forgotten when dropped.
struct Cancelled {
    ids: Arc<Mutex<Ids>>,
    server: u64,
}

impl Drop for Cancelled {
    fn drop(&mut self) {
        lock(&self.ids).release(self.server);
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
/// forgotten once a reply ends the call, or once `close` has answered the calls held. A reply to a
/// call its sender cancelled is dropped before the wire, and its send answered once the server has
/// taken the `cancel` and cancelled the call `ClientCancelled`.
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
    let caller = match ids.caller(server) {
        Some(Caller::Live(_, caller)) => caller,
        Some(Caller::Cancelled(released)) => {
            return Ok(Either::Right(async move {
                released.wait_for(|released| *released).await;
                Ok(())
            }));
        }
        None => return Err("the call is no longer in flight".into()),
    };
    let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
    let bytes = encode(codec, &with_id(frame, caller))?;
    let written = write(writes, bytes, Some(peer));
    if ends {
        ids.finish(server);
    }
    Ok(Either::Left(written))
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
        Either::Right((Ok(()), _)) => tracing::debug!("the UDP link let go of its socket with writes still queued, {grace:?} after its close or its drop"),
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
        Frame::Res { data, .. } => Frame::res(id, data),
        Frame::Err { error, .. } => Frame::err(id, error),
        Frame::Item { data, .. } => Frame::item(id, data),
        Frame::End { .. } => Frame::end(id),
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
                queue_reply(&writes, peer, Codec::Json, &ids, Frame::res(server, ulo_rpc::Data::new(Bytes::from_static(b"1")))).map(drop)
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
        Frame::req(id, LATE, ulo_rpc::CallHeaders::new(), ulo_rpc::Data::new(Bytes::from_static(b"1")))
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
        let open = Frame::open(8, LATE, ulo_rpc::CallHeaders::new());
        match inward(open.clone(), peer, &ids, &state.next_id) {
            Inward::Refuse(frame) => assert_eq!(frame, open, "the streamed request refused after the sweep"),
            other => panic!("a streamed request after the sweep was {other:?}"),
        }
        assert!(lock(&ids).by_server.is_empty(), "the swept table holds a call");
        let event = Frame::evt(LATE, ulo_rpc::CallHeaders::new(), ulo_rpc::Data::new(Bytes::from_static(b"1")));
        match inward(event.clone(), peer, &ids, &state.next_id) {
            Inward::Deliver(frame) => assert_eq!(frame, event, "the event delivered after the sweep"),
            other => panic!("an event after the sweep was {other:?}"),
        }
        let forgotten = [Frame::input(7, ulo_rpc::Data::new(Bytes::from_static(b"1"))), Frame::in_end(7), Frame::cancel(7)];
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

        static AFTER_FAILURE: Mutex<Vec<Stop>> = Mutex::new(Vec::new());

        /// Stops the reply lane of the next client socket sending to `addr` to fail once it has
        /// answered the calls awaiting it, before it returns: the receiver answers once the lane
        /// has stopped there, and the sender releases it.
        pub(crate) fn stop_after_failure(addr: SocketAddr) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
            let (arrived, stopped) = oneshot::channel();
            let (release, released) = oneshot::channel();
            lock(&AFTER_FAILURE).push((addr, arrived, released));
            (stopped, release)
        }

        pub(crate) async fn after_failure(addr: SocketAddr) {
            let stop = {
                let mut stops = lock(&AFTER_FAILURE);
                let at = stops.iter().position(|(stopped, _, _)| *stopped == addr);
                at.map(|at| stops.swap_remove(at))
            };
            if let Some((_, arrived, released)) = stop {
                let _ = arrived.send(());
                let _ = released.await;
            }
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
        let inbound = udp.listen(&[Pattern::from(LATE).into()]).await.expect("the link listens");
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
        let _inbound = udp.listen(&[Pattern::from(LATE).into()]).await.expect("the link listens");
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

#[cfg(test)]
mod host_names {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::*;

    /// How long a test waits for anything before it fails.
    const PATIENCE: Duration = Duration::from_secs(5);

    fn on(endpoint: &str, lookup: Lookup) -> Udp {
        let mut udp = Udp::new(endpoint);
        udp.lookup = lookup;
        udp
    }

    async fn receiving() -> UdpSocket {
        UdpSocket::bind("127.0.0.1:0").await.expect("a loopback port")
    }

    /// Sends one `cancel` through `outbound` and waits for `socket` to receive a datagram.
    async fn reaches(outbound: &Outbound, socket: &UdpSocket, which: &str) {
        (outbound.send)(Pattern::new("host_names"), Frame::cancel(1), None).await.expect("the frame is sent");
        let mut buf = [0u8; 256];
        tokio::time::timeout(PATIENCE, socket.recv_from(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("{which} received no datagram"))
            .unwrap_or_else(|error| panic!("{which} failed to receive: {error}"));
    }

    /// F416: each connect looks the name up again, so a connect made after the name's address
    /// changed sends to the new one. The lookup answers the first socket, then the second.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_connect_resolves_the_name_again() {
        let (first, second) = (receiving().await, receiving().await);
        let answers = [first.local_addr().expect("an address"), second.local_addr().expect("an address")];
        let asked = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&asked);
        let udp = on(
            "billing:7000",
            Lookup::from_fn(move |host| {
                assert_eq!(host.to_string(), "billing:7000", "the name looked up");
                Ok(vec![answers[counting.fetch_add(1, Ordering::SeqCst).min(1)]])
            }),
        );
        let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());
        let one = udp.connect(&runtime).await.expect("the first connect");
        reaches(&one, &first, "the address the name had at the first connect").await;
        let two = udp.connect(&runtime).await.expect("the second connect");
        reaches(&two, &second, "the address the name had at the second connect").await;
        assert_eq!(asked.load(Ordering::SeqCst), 2, "the lookups two connects made");
    }

    /// F416: the lookup runs on the link's blocking pool, off the thread polling the connect. On a
    /// current-thread runtime the lookup waits for a release that a task on that thread sends, which
    /// it can send only while the connect leaves the thread free.
    #[tokio::test(flavor = "current_thread")]
    async fn a_connect_resolves_off_the_thread_polling_it() {
        let socket = receiving().await;
        let addr = socket.local_addr().expect("an address");
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let udp = on(
            "billing:7000",
            Lookup::from_fn(move |_| match lock(&released).recv_timeout(PATIENCE) {
                Ok(()) => Ok(vec![addr]),
                Err(_) => Err(io::Error::other("the lookup's release never came: the lookup held the thread polling the connect")),
            }),
        );
        let releasing = tokio::spawn(async move {
            let _ = release.send(());
        });
        let connected = udp.connect(&(Arc::new(Tokio::current()) as Arc<dyn Runtime>)).await;
        let outbound = connected.unwrap_or_else(|error| panic!("the connect failed: {error}"));
        reaches(&outbound, &socket, "the address the lookup answered").await;
        releasing.await.expect("the releasing task ends");
    }

    struct Empty;

    impl ulo::Module for Empty {
        fn identity(&self) -> ulo::ModuleIdentity {
            ulo::ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, _m: &mut ulo::ModuleDef<'_>) {}
    }

    /// F416: a server binds the first of its host name's addresses that binds, as
    /// `std::net::UdpSocket::bind` does: `192.0.2.1`, a documentation address no interface holds,
    /// is refused by the OS, and the loopback address after it binds.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_binds_the_first_of_its_host_name_s_addresses_that_binds() {
        let mut udp = on("billing:0", Lookup::from_fn(|host| Ok(vec![SocketAddr::from(([192, 0, 2, 1], host.port())), SocketAddr::from(([127, 0, 0, 1], host.port()))])));
        let app = ulo::App::builder(Empty).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
        udp.prepare(&app.handle()).await.expect("the link prepares");
        let _inbound = udp.listen(&[]).await.expect("the link listens past an address no interface holds");
        let bound: Vec<_> = udp.bound().iter().map(|bound| bound.addr.ip()).collect();
        assert_eq!(bound, vec![std::net::IpAddr::from([127, 0, 0, 1])], "the address the server bound");
        udp.close().await.expect("the link closes");
        let _ = app.close(ulo::Signal::new("host_names")).await;
    }

    /// F416: a server on a host name resolves it once, in `prepare`, and binds the first address
    /// it answers that binds; `listen` looks nothing up.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_resolves_its_host_name_once_and_binds_the_first_address() {
        let asked = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&asked);
        let mut udp = on(
            "billing:0",
            Lookup::from_fn(move |host| {
                counting.fetch_add(1, Ordering::SeqCst);
                Ok(vec![SocketAddr::from(([127, 0, 0, 1], host.port())), SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], host.port()))])
            }),
        );
        let app = ulo::App::builder(Empty).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
        udp.prepare(&app.handle()).await.expect("the link prepares");
        let _inbound = udp.listen(&[]).await.expect("the link listens");
        let bound: Vec<_> = udp.bound().iter().map(|bound| bound.addr.ip()).collect();
        assert_eq!(bound, vec![std::net::IpAddr::from([127, 0, 0, 1])], "the address the server bound");
        assert_eq!(asked.load(Ordering::SeqCst), 1, "the lookups a server on a host name made through `prepare` and `listen`");
        udp.close().await.expect("the link closes");
        let _ = app.close(ulo::Signal::new("host_names")).await;
    }

    /// A client on a name resolving to `[::1]:p` and then `127.0.0.1:p` sends to the first, as
    /// `std::net::UdpSocket::connect` does, though the only socket on `p` is on `127.0.0.1`. The
    /// host answers port unreachable, and each call, the second on a new socket that resolved the
    /// name again, fails `Unavailable` naming the address it sent to and the name's two addresses.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_refused_at_the_first_of_a_name_s_addresses_names_the_address() {
        let socket = receiving().await;
        let port = socket.local_addr().expect("an address").port();
        let first = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
        // Bound and released: nothing else holds the port on `::1`, so the host refuses there.
        drop(std::net::UdpSocket::bind(first).expect("the port is free on `::1`"));
        let udp = on(&format!("billing:{port}"), Lookup::from_fn(|host| Ok(vec![SocketAddr::from((Ipv6Addr::LOCALHOST, host.port())), SocketAddr::from(([127, 0, 0, 1], host.port()))])));
        let rpc = ulo_rpc::RpcClient::new(udp, Arc::new(Tokio::current())).expect("the client is built");
        let expected = format!("the UDP link's datagrams to {first} were refused (ICMP port unreachable); `billing:{port}` resolved to 2 addresses and the link took {first} first");
        for call in ["the first call", "the call on the next socket"] {
            let failed = rpc.request::<_, u32>("host_names", &1).timeout(PATIENCE).await.expect_err("a call to an address nothing listens on");
            assert_eq!(failed.kind(), ulo_transport::ErrorKind::Unavailable, "{call}: {failed:?}");
            assert!(failed.message().contains(&expected), "{call}: the failure does not name the address it sent to: {:?}", failed.message());
        }
        let mut buf = [0u8; 256];
        assert!(socket.try_recv_from(&mut buf).is_err(), "the socket on `127.0.0.1` received a datagram");
    }

    #[test]
    fn a_failure_names_the_address_and_where_a_name_s_answer_put_it() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        let addr = SocketAddr::from(([127, 0, 0, 1], 7000));
        let host = HostPort::parse("billing:7000").expect("a host name and port");
        let at = |name: Option<Resolved>| SentTo { addr, name }.failure(&refused);
        let refusal = "the UDP link's datagrams to 127.0.0.1:7000 were refused (ICMP port unreachable)";
        assert_eq!(at(None), refusal, "an address");
        assert_eq!(at(Some(Resolved { host: host.clone(), resolved: 1, taken: 0 })), refusal, "a name resolving to one address");
        assert_eq!(
            at(Some(Resolved { host: host.clone(), resolved: 3, taken: 1 })),
            format!("{refusal}; `billing:7000` resolved to 3 addresses and the link took 127.0.0.1:7000 as number 2, the first a socket could connect to")
        );
        let other = SentTo { addr, name: None }.failure(&io::Error::other("the interface went down"));
        assert_eq!(other, "the UDP link's socket sending to 127.0.0.1:7000 failed: the interface went down");
    }

    /// A request joins the calls the reply lane answers until the lane has sent its `goaway`, a
    /// recorded failure alone changing nothing; after it, the request's failure is put on the lane
    /// behind the `goaway`, and an event or a `cancel` is dropped, nothing written to the socket.
    #[test]
    fn a_request_after_the_goaway_is_answered_on_the_reply_lane() {
        let mut awaiting = Awaiting::default();
        let request = |id| Frame::req(id, "p", Default::default(), ulo_rpc::Data::default());
        assert!(matches!(awaiting.sent(&request(1)), Sent::Queued), "a request before the failure");
        assert!(matches!(awaiting.sent(&request(2)), Sent::Queued), "a request before the failure");
        assert!(matches!(awaiting.sent(&Frame::cancel(2)), Sent::Queued), "a cancel");
        assert_eq!(awaiting.ids, HashSet::from([1]), "the requests awaiting a reply");
        awaiting.failed = Some("the socket failed".to_owned());
        assert!(matches!(awaiting.sent(&request(3)), Sent::Queued), "a request after the failure, before the `goaway`");
        assert_eq!(awaiting.ids, HashSet::from([1, 3]), "the calls the lane takes");
        let (lane, _replies) = tokio::sync::mpsc::channel(1);
        awaiting.lane = Some(lane);
        match awaiting.sent(&request(4)) {
            Sent::Told(_, Frame::Err { id, error, .. }) => assert_eq!((id, error.kind, error.message.as_str()), (4, ErrorKind::Unavailable, "the socket failed")),
            _ => panic!("a request after the `goaway` was not answered on the lane"),
        }
        let event = Frame::evt("p", Default::default(), ulo_rpc::Data::default());
        assert!(matches!(awaiting.sent(&event), Sent::Dropped), "an event after the `goaway`");
        assert!(matches!(awaiting.sent(&Frame::cancel(1)), Sent::Dropped), "a cancel after the `goaway`");
        assert_eq!(awaiting.ids, HashSet::from([1, 3]), "nothing after the `goaway` changes the calls taken");
    }

    /// A request reaching the socket while the reply lane waits for room for its `goaway` joins
    /// the calls the lane takes, so no call is told the socket failed ahead of the `goaway`. On one
    /// thread, the lane, its one slot full, runs until its `goaway` waits.
    #[tokio::test(flavor = "current_thread")]
    async fn a_request_before_the_goaway_is_queued_joins_the_calls_the_lane_takes() {
        let silent = receiving().await;
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("a loopback port");
        socket.connect(silent.local_addr().expect("an address")).await.expect("the socket connects");
        let (_epoch, epoch) = watch::channel(0);
        let (frames, mut replies) = tokio::sync::mpsc::channel(1);
        frames.send(Frame::cancel(0)).await.expect("the lane's one slot is filled");
        let awaiting = Arc::new(Mutex::new(Awaiting { ids: HashSet::from([4]), failed: Some("the socket failed".to_owned()), lane: None }));
        let refused = Arc::new(Notify::new());
        refused.notify_one();
        let sent_to = SentTo { addr: silent.local_addr().expect("an address"), name: None };
        let reading = tokio::spawn(read_replies(Arc::new(socket), epoch, Codec::Json, frames, Arc::clone(&awaiting), refused, sent_to));
        tokio::task::yield_now().await;
        let request = Frame::req(5, "p", Default::default(), ulo_rpc::Data::default());
        assert!(matches!(lock(&awaiting).sent(&request), Sent::Queued), "a request before the `goaway` is queued was answered apart from the calls the lane takes");
        let mut sent = Vec::new();
        while sent.len() < 4 {
            let frame = tokio::time::timeout(PATIENCE, replies.recv()).await.expect("the lane sends").expect("the lane is open");
            sent.push(match frame {
                Frame::Cancel { id: 0, .. } => "filler".to_owned(),
                Frame::Goaway => "goaway".to_owned(),
                Frame::Err { id, .. } => format!("err {id}"),
                other => panic!("the reply lane sent {other:?}"),
            });
        }
        sent[2..].sort();
        assert_eq!(sent, ["filler", "goaway", "err 4", "err 5"], "the frames the lane sent");
        tokio::time::timeout(PATIENCE, reading).await.expect("the reply lane returns").expect("the reply lane's task");
    }

    /// The call after a refused socket connects again and follows the name: the lookup answers
    /// `[::1]:p`, where nothing listens, at the first connect and `127.0.0.1:p`, where a socket
    /// answers each request, at the second, and that call and the one after it are answered.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_call_after_a_refused_socket_connects_again_and_is_answered() {
        let server = receiving().await;
        let port = server.local_addr().expect("an address").port();
        drop(std::net::UdpSocket::bind((Ipv6Addr::LOCALHOST, port)).expect("the port is free on `::1`"));
        let asked = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&asked);
        let udp = on(
            &format!("billing:{port}"),
            Lookup::from_fn(move |host| {
                let first = counting.fetch_add(1, Ordering::SeqCst) == 0;
                Ok(vec![if first { SocketAddr::from((Ipv6Addr::LOCALHOST, host.port())) } else { SocketAddr::from(([127, 0, 0, 1], host.port())) }])
            }),
        );
        let answering = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_DATAGRAM as usize + 1];
            while let Ok((len, peer)) = server.recv_from(&mut buf).await {
                if let Ok(Frame::Req { id, .. }) = Codec::Json.decode_frame(&buf[..len]) {
                    let reply = Codec::Json.encode_frame(&Frame::res(id, ulo_rpc::Data::new("2"))).expect("the reply encodes");
                    let _ = server.send_to(&reply, peer).await;
                }
            }
        });
        let rpc = ulo_rpc::RpcClient::new(udp, Arc::new(Tokio::current())).expect("the client is built");
        let refused = rpc.request::<_, u32>("host_names", &1).timeout(PATIENCE).await.expect_err("a call to an address nothing listens on");
        assert_eq!(refused.kind(), ulo_transport::ErrorKind::Unavailable, "{refused:?}");
        for call in ["the call after the refusal", "the call after that"] {
            let answered = rpc.request::<_, u32>("host_names", &1).timeout(PATIENCE).await;
            assert!(matches!(answered, Ok(2)), "{call}: {answered:?}");
        }
        assert_eq!(asked.load(Ordering::SeqCst), 2, "the lookups the two sockets made");
        answering.abort();
    }

    /// F693: a call made once another was told its socket failed connects again. The reply lane
    /// is held after it has answered the refused call and before it returns, the window in which
    /// the client could still read the connection as open; the next call, made in that window,
    /// follows the name to `127.0.0.1:p` and is answered.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_made_once_another_was_told_the_socket_failed_connects_again() {
        let server = receiving().await;
        let port = server.local_addr().expect("an address").port();
        let refusing = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
        drop(std::net::UdpSocket::bind(refusing).expect("the port is free on `::1`"));
        let asked = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&asked);
        let udp = on(
            &format!("billing:{port}"),
            Lookup::from_fn(move |host| {
                let first = counting.fetch_add(1, Ordering::SeqCst) == 0;
                Ok(vec![if first { SocketAddr::from((Ipv6Addr::LOCALHOST, host.port())) } else { SocketAddr::from(([127, 0, 0, 1], host.port())) }])
            }),
        );
        let answering = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_DATAGRAM as usize + 1];
            while let Ok((len, peer)) = server.recv_from(&mut buf).await {
                if let Ok(Frame::Req { id, .. }) = Codec::Json.decode_frame(&buf[..len]) {
                    let reply = Codec::Json.encode_frame(&Frame::res(id, ulo_rpc::Data::new("2"))).expect("the reply encodes");
                    let _ = server.send_to(&reply, peer).await;
                }
            }
        });
        let (held, release) = tests::gate::stop_after_failure(refusing);
        let rpc = ulo_rpc::RpcClient::new(udp, Arc::new(Tokio::current())).expect("the client is built");
        let refused = rpc.request::<_, u32>("host_names", &1).timeout(PATIENCE).await.expect_err("a call to an address nothing listens on");
        assert_eq!(refused.kind(), ulo_transport::ErrorKind::Unavailable, "{refused:?}");
        tokio::time::timeout(PATIENCE, held).await.expect("the reply lane reaches the gate").expect("the gate answers");
        let answered = rpc.request::<_, u32>("host_names", &1).timeout(PATIENCE).await;
        let _ = release.send(());
        assert!(matches!(answered, Ok(2)), "the call made once the first was told the socket failed: {answered:?}");
        assert_eq!(asked.load(Ordering::SeqCst), 2, "the lookups the two sockets made");
        answering.abort();
    }

    /// The host's refusal is reported once, to whichever of the socket's receive and its next send
    /// reads it first. A send it reaches fails the socket as the receive would, waking the reply
    /// lane and keeping its call among those the lane answers, or, for an event, answered with
    /// the failure; a send failing otherwise ends its
    /// call with its own error until the socket has failed, and succeeds after, the lane
    /// answering its call.
    #[test]
    fn a_send_the_host_refused_fails_the_socket_as_a_receive_would() {
        let sent_to = SentTo { addr: SocketAddr::from(([127, 0, 0, 1], 7000)), name: None };
        let told = sent_to.failure(&io::Error::from(io::ErrorKind::ConnectionRefused));
        let request = |id| Frame::req(id, "p", Default::default(), ulo_rpc::Data::default());
        let refused = Notify::new();
        let mut awaiting = Awaiting::default();
        for id in [1, 2, 3] {
            assert!(matches!(awaiting.sent(&request(id)), Sent::Queued), "a request before the failure");
        }

        let other = awaiting.send_failed(Some(1), io::Error::other("the interface went down").into(), &sent_to, &refused);
        assert_eq!(other.map_err(|error| error.to_string()), Err("the interface went down".to_owned()), "a send failing otherwise before the socket failed");
        assert_eq!(awaiting.failed, None, "a send failing otherwise fails no socket");
        assert!(refused.notified().now_or_never().is_none(), "a send failing otherwise woke the reply lane");

        let own = awaiting.send_failed(Some(2), io::Error::from(io::ErrorKind::ConnectionRefused).into(), &sent_to, &refused);
        assert!(own.is_ok(), "the send the host refused: {own:?}");
        assert_eq!(awaiting.failed.as_deref(), Some(told.as_str()), "the socket's failure");
        assert!(refused.notified().now_or_never().is_some(), "the reply lane is woken");
        assert_eq!(awaiting.ids, HashSet::from([2, 3]), "the calls left for the reply lane to answer");

        let event = Awaiting::default().send_failed(None, io::Error::from(io::ErrorKind::ConnectionRefused).into(), &sent_to, &Notify::new());
        assert_eq!(event.map_err(|error| error.to_string()), Err(told.clone()), "an event whose send the host refused");

        let late = awaiting.send_failed(Some(3), "the UDP link's client socket is closed".into(), &sent_to, &refused);
        assert!(late.is_ok(), "a datagram queued as the socket failed: {late:?}");
        assert_eq!(awaiting.ids, HashSet::from([2, 3]), "the calls left for the reply lane to answer");
    }

    /// Woken by a send the host refused, the reply lane sends a `goaway`, then answers each call
    /// still awaiting the socket `err` with the failure that send recorded, then ends.
    #[tokio::test(flavor = "current_thread")]
    async fn a_reply_lane_woken_by_a_refused_send_answers_every_waiting_call() {
        let silent = receiving().await;
        let socket = receiving().await;
        socket.connect(silent.local_addr().expect("an address")).await.expect("the socket connects");
        let (_epoch, epoch) = watch::channel(0);
        let (frames, mut replies) = tokio::sync::mpsc::channel(8);
        let awaiting = Arc::new(Mutex::new(Awaiting { ids: HashSet::from([4, 5]), failed: Some("the socket failed".to_owned()), lane: None }));
        let refused = Arc::new(Notify::new());
        refused.notify_one();
        let sent_to = SentTo { addr: silent.local_addr().expect("an address"), name: None };
        tokio::time::timeout(PATIENCE, read_replies(Arc::new(socket), epoch, Codec::Json, frames, awaiting, refused, sent_to))
            .await
            .expect("the reply lane ends once woken");
        assert!(matches!(replies.recv().await, Some(Frame::Goaway)), "the lane's first frame is a `goaway`");
        let mut answered = Vec::new();
        while let Some(frame) = replies.recv().await {
            match frame {
                Frame::Err { id, error, .. } => {
                    assert_eq!((error.kind, error.message.as_str()), (ErrorKind::Unavailable, "the socket failed"), "call {id}");
                    answered.push(id);
                }
                other => panic!("the reply lane sent {other:?}"),
            }
        }
        answered.sort_unstable();
        assert_eq!(answered, vec![4, 5], "the calls answered");
    }

    /// A host may report a pending refusal ahead of a reply that arrived before it, as Linux
    /// does. The lane, failing with that reply still in its socket's buffer, delivers it to its
    /// call first and answers only the call left waiting `err`. The wake stands for the refusal,
    /// taken before the ready datagram.
    #[tokio::test(flavor = "current_thread")]
    async fn a_reply_queued_ahead_of_the_refusal_reaches_its_call() {
        let peer = receiving().await;
        let socket = receiving().await;
        socket.connect(peer.local_addr().expect("an address")).await.expect("the socket connects");
        let reply = Codec::Json.encode_frame(&Frame::res(4, ulo_rpc::Data::new("1"))).expect("the reply encodes");
        peer.send_to(&reply, socket.local_addr().expect("an address")).await.expect("the reply is sent");
        tokio::time::timeout(PATIENCE, socket.readable()).await.expect("the reply arrives").expect("the socket is readable");
        let (_epoch, epoch) = watch::channel(0);
        let (frames, mut replies) = tokio::sync::mpsc::channel(8);
        let awaiting = Arc::new(Mutex::new(Awaiting { ids: HashSet::from([4, 5]), failed: Some("the socket failed".to_owned()), lane: None }));
        let refused = Arc::new(Notify::new());
        refused.notify_one();
        let sent_to = SentTo { addr: peer.local_addr().expect("an address"), name: None };
        tokio::time::timeout(PATIENCE, read_replies(Arc::new(socket), epoch, Codec::Json, frames, awaiting, refused, sent_to))
            .await
            .expect("the reply lane ends once woken");
        let mut sent = Vec::new();
        while let Some(frame) = replies.recv().await {
            sent.push(match frame {
                Frame::Goaway => (0, "goaway"),
                Frame::Res { id, .. } => (id, "res"),
                Frame::Err { id, error, .. } => {
                    assert_eq!(error.message, "the socket failed", "call {id}");
                    (id, "err")
                }
                other => panic!("the reply lane sent {other:?}"),
            });
        }
        assert_eq!(sent, vec![(0, "goaway"), (4, "res"), (5, "err")], "the connection closed, the reply delivered, then the call left waiting failed");
    }
}
