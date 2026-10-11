//! A TCP relay a [`Broker`](crate::Broker) puts between a client link and what it reaches, so
//! `disrupt` can sever the client's connections and nothing else: the server keeps its own, and
//! the relay keeps accepting, so the client's next connection goes through, at once or after an
//! outage the environment chooses.
//!
//! The relay belongs to no runtime: it accepts and copies on threads of its own, over `std`
//! sockets, so it carries a link whatever runtime the link and the scenario run on. Its waits are
//! `async`, a thread's end signalled through a channel that needs no runtime either.
//!
//! [`reachable`] and [`unshadowed`] serve a container's published port. On a VM-backed engine
//! (OrbStack, Docker Desktop) the engine picks that port inside its VM, without seeing the host's
//! sockets, from a range that overlaps the host's ephemeral one, and forwards it from a wildcard
//! listener on the host. A host process already listening on `127.0.0.1` at that port, a relay or
//! an editor's local server, is the more specific match, and a connection to `127.0.0.1` reaches
//! it instead of the container.

use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use futures_channel::oneshot;

use crate::report;

/// Forwards every connection made to its address to its upstream.
pub struct Relay {
    addr: SocketAddr,
    /// Set once: at the start, or by [`Relay::forward_to`] for a relay started before its upstream
    /// was bound.
    upstream: Arc<OnceLock<SocketAddr>>,
    shared: Arc<Shared>,
}

/// What the accepting thread and the relay share.
struct Shared {
    /// Every relayed connection not yet seen to have ended.
    connections: Mutex<Vec<Relayed>>,
    outage: Mutex<Window>,
    /// Set by the relay's drop; the accepting thread stops at its next accept.
    stopped: AtomicBool,
}

/// One relayed connection: its two sockets, shut by a cut, and its end.
struct Relayed {
    client: TcpStream,
    /// The upstream socket once connected; `cut` set first stops the connection's thread from
    /// relaying through one it connects afterwards.
    upstream: Arc<Mutex<Upstream>>,
    /// Fails with `Canceled` once the connection's thread has ended, both directions done.
    ended: oneshot::Receiver<()>,
}

#[derive(Default)]
struct Upstream {
    socket: Option<TcpStream>,
    cut: bool,
}

impl Relayed {
    fn has_ended(&mut self) -> bool {
        matches!(self.ended.try_recv(), Err(oneshot::Canceled))
    }

    /// Shuts both sockets, which ends both directions' blocking reads.
    fn shut(&self) {
        let mut upstream = lock(&self.upstream);
        upstream.cut = true;
        if let Some(socket) = &upstream.socket {
            let _ = socket.shutdown(Shutdown::Both);
        }
        drop(upstream);
        let _ = self.client.shutdown(Shutdown::Both);
    }
}

/// The last cut: until when a connection the relay accepts is closed at once, and the moments
/// [`Outage`] reports.
#[derive(Default)]
struct Window {
    until: Option<Instant>,
    shut: Option<Instant>,
    reopened: Option<Instant>,
}

/// When the relay last held its clients out, as [`Relay::last_outage`] reports it.
#[derive(Clone, Copy, Debug)]
pub struct Outage {
    /// When every connection the cut closed had ended.
    pub shut: Instant,
    /// When the relay first relayed a connection after `shut`; `None` while it has relayed none.
    pub reopened: Option<Instant>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Relay {
    /// A relay on a loopback port of its own, forwarding to `upstream`.
    pub async fn start(upstream: SocketAddr) -> Relay {
        Relay::listen(Relay::bind().await, upstream)
    }

    /// A relay on a loopback port of its own whose upstream is not yet bound, as a server on port 0
    /// is not until it listens; [`forward_to`](Self::forward_to) names it. A connection accepted
    /// before that is closed at once, as a refused connect would fail it.
    pub async fn without_upstream() -> Relay {
        Relay::accepting(Relay::bind().await, OnceLock::new())
    }

    /// Forwards every connection accepted from now on to `upstream`. A relay forwards to one
    /// upstream: naming another than the one it has fails the scenario.
    pub fn forward_to(&self, upstream: SocketAddr) {
        let current = *self.upstream.get_or_init(|| upstream);
        assert_eq!(current, upstream, "the relay at {} already forwards to {current}", self.addr);
    }

    /// A loopback listener for [`Relay::listen`], for an environment that has to name the relay's
    /// address before it knows the upstream one: a Kafka broker advertising the relay as its
    /// listener, say.
    pub async fn bind() -> TcpListener {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap_or_else(|error| crate::startup_failed!("the relay did not bind a loopback port: {}", report(&error)))
    }

    /// A relay accepting on `listener`, forwarding to `upstream`.
    pub fn listen(listener: TcpListener, upstream: SocketAddr) -> Relay {
        Relay::accepting(listener, OnceLock::from(upstream))
    }

    fn accepting(listener: TcpListener, upstream: OnceLock<SocketAddr>) -> Relay {
        let addr = listener.local_addr().unwrap_or_else(|error| crate::startup_failed!("the relay's listener has no address: {}", report(&error)));
        listener
            .set_nonblocking(false)
            .unwrap_or_else(|error| crate::startup_failed!("the relay's listener did not block: {}", report(&error)));
        let upstream = Arc::new(upstream);
        let shared = Arc::new(Shared { connections: Mutex::new(Vec::new()), outage: Mutex::new(Window::default()), stopped: AtomicBool::new(false) });
        {
            let (upstream, shared) = (Arc::clone(&upstream), Arc::clone(&shared));
            thread::Builder::new()
                .name(format!("relay {addr}"))
                .spawn(move || accept(&listener, &upstream, &shared))
                .unwrap_or_else(|error| crate::startup_failed!("the relay's accepting thread did not start: {}", report(&error)));
        }
        Relay { addr, upstream, shared }
    }

    /// The address clients connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Closes every connection through the relay, which keeps accepting new ones, and answers how
    /// many it closed.
    pub async fn cut(&self) -> usize {
        self.cut_for(Duration::ZERO).await
    }

    /// Closes every connection through the relay and, for `outage` from now, closes each new one
    /// as it is accepted, so the client stays disconnected that long. Answers how many open
    /// connections it closed; it returns without waiting for the outage to end.
    pub async fn cut_for(&self, outage: Duration) -> usize {
        *self.window() = Window { until: Some(Instant::now() + outage), shut: None, reopened: None };
        let severed: Vec<Relayed> = {
            let mut connections = lock(&self.shared.connections);
            connections.retain_mut(|relayed| !relayed.has_ended());
            std::mem::take(&mut *connections)
        };
        for relayed in &severed {
            relayed.shut();
        }
        let count = severed.len();
        for relayed in severed {
            let _ = relayed.ended.await;
        }
        self.window().shut = Some(Instant::now());
        count
    }

    /// How many connections the relay carries now. A relayed connection ends once both of its
    /// directions have: the client closing its socket ends it when the upstream closes its own in
    /// turn, as a broker does on reading the end of a connection.
    pub async fn open(&self) -> usize {
        let mut connections = lock(&self.shared.connections);
        connections.retain_mut(|relayed| !relayed.has_ended());
        connections.len()
    }

    /// The last [`cut_for`](Self::cut_for)'s outage, observed: `None` before any cut.
    pub fn last_outage(&self) -> Option<Outage> {
        let window = self.window();
        Some(Outage { shut: window.shut?, reopened: window.reopened })
    }

    fn window(&self) -> MutexGuard<'_, Window> {
        lock(&self.shared.outage)
    }
}

/// Stops accepting and shuts every connection, waiting for none of them.
impl Drop for Relay {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        for relayed in lock(&self.shared.connections).drain(..) {
            relayed.shut();
        }
        // Wakes the accepting thread's blocking accept, which then sees `stopped`.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
    }
}

/// How many times [`unshadowed`] starts an environment before it fails the scenario.
const SHADOW_ATTEMPTS: usize = 5;

/// Whether a connection to `addr`, a container's port published on `127.0.0.1`, reaches some other
/// listener on this host. On macOS this binds `127.0.0.1` at that port with `SO_REUSEADDR` for a
/// moment: the engine's wildcard listener allows it, and a specific listener already there
/// refuses it. On Linux the kernel refuses a wildcard bind beside a listener on `127.0.0.1`, so the
/// shadow cannot form, and the same probe would be refused by the engine's own listener: there it
/// answers `false`.
pub fn shadowed(addr: SocketAddr) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::net::TcpListener as StdTcpListener;
        // std sets `SO_REUSEADDR` on a listener it binds on Unix.
        addr.ip() == Ipv4Addr::LOCALHOST && StdTcpListener::bind(addr).is_err()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = addr;
        false
    }
}

/// Runs `start`, which starts an environment and answers it with the loopback addresses it is
/// reached at, until no other listener on this host shadows any of them ([`shadowed`]), at most
/// five times. An environment that is shadowed is dropped, which stops its container, and each
/// retry is logged on stderr. `what` names the environment in the log and in the failure.
pub async fn unshadowed<E, F, Fut>(what: &str, mut start: F) -> (E, Vec<SocketAddr>)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = (E, Vec<SocketAddr>)>,
{
    let mut collisions = Vec::new();
    for attempt in 1..=SHADOW_ATTEMPTS {
        let (environment, addrs) = start().await;
        let shadows: Vec<SocketAddr> = addrs.iter().copied().filter(|addr| shadowed(*addr)).collect();
        if shadows.is_empty() {
            return (environment, addrs);
        }
        eprintln!(
            "{what} was published on {shadows:?}, where another listener on this host would receive its connections; starting it again (attempt {attempt} of {SHADOW_ATTEMPTS})"
        );
        collisions.extend(shadows);
        drop(environment);
    }
    crate::startup_failed!("{what} was published on a port another listener on this host holds at each of {SHADOW_ATTEMPTS} starts: {collisions:?}")
}

/// Waits until `addr` accepts a TCP connection, failing the scenario after `within`. A container's
/// forwarded port can refuse connections for a moment after the broker inside reports ready. The
/// attempts run on a thread of their own, which answers through a channel.
pub async fn reachable(addr: SocketAddr, within: Duration) {
    let (answer, answered) = oneshot::channel();
    thread::spawn(move || {
        let deadline = Instant::now() + within;
        let outcome = loop {
            match TcpStream::connect_timeout(&addr, within) {
                Ok(_) => break Ok(()),
                Err(error) if Instant::now() >= deadline => break Err(error),
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        };
        let _ = answer.send(outcome);
    });
    match answered.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => crate::startup_failed!("{addr} did not accept a connection within {within:?}: {}", report(&error)),
        Err(oneshot::Canceled) => crate::startup_failed!("the thread trying {addr} ended without an answer"),
    }
}

/// Relays each connection to `upstream`, or closes it at once during an outage or while no
/// upstream is set, until the relay is dropped.
fn accept(listener: &TcpListener, upstream: &OnceLock<SocketAddr>, shared: &Arc<Shared>) {
    loop {
        let accepted = listener.accept();
        if shared.stopped.load(Ordering::SeqCst) {
            return;
        }
        let Ok((client, _)) = accepted else { continue };
        let Some(upstream) = upstream.get().copied() else { continue };
        {
            let mut window = lock(&shared.outage);
            let now = Instant::now();
            if window.until.is_some_and(|until| now < until) {
                continue;
            }
            if window.shut.is_some() && window.reopened.is_none() {
                window.reopened = Some(now);
            }
        }
        let Ok(handle) = client.try_clone() else { continue };
        let slot = Arc::new(Mutex::new(Upstream::default()));
        let (end, ended) = oneshot::channel::<()>();
        {
            let mut connections = lock(&shared.connections);
            connections.retain_mut(|relayed| !relayed.has_ended());
            connections.push(Relayed { client: handle, upstream: Arc::clone(&slot), ended });
        }
        // A thread that cannot start drops the closure, the socket and `end` with it: the client's
        // connection closes, as a refused connect would fail it, and the relay reads it ended.
        let _ = thread::Builder::new().name("relayed connection".to_owned()).spawn(move || {
            relay(client, upstream, &slot);
            // Dropped unsent: the relay reads the connection's end as `Canceled`.
            drop(end);
        });
    }
}

/// Connects to `upstream` and copies both ways until both directions have ended. An upstream
/// refusing the connect closes the client's connection, as a refused connect would fail the
/// client's.
///
/// The client's socket is shut on every way out: the relay holds a handle to it for `cut`, so
/// dropping this thread's handle alone would leave the connection open.
fn relay(client: TcpStream, upstream: SocketAddr, slot: &Mutex<Upstream>) {
    let _closer = client.try_clone().map(Closer);
    let Ok(server) = TcpStream::connect(upstream) else { return };
    {
        let mut slot = lock(slot);
        if slot.cut {
            return;
        }
        let Ok(handle) = server.try_clone() else { return };
        slot.socket = Some(handle);
    }
    let (Ok(client_reader), Ok(server_reader)) = (client.try_clone(), server.try_clone()) else { return };
    let inward = thread::spawn(move || copy(client_reader, server));
    copy(server_reader, client);
    let _ = inward.join();
}

/// Shuts its socket when dropped.
struct Closer(TcpStream);

impl Drop for Closer {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// Copies `from` into `to` until `from` ends, then shuts `to`'s writing half, as
/// `copy_bidirectional` does; a failure either way shuts both sockets, ending the other direction
/// too.
fn copy(mut from: TcpStream, mut to: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let read = match from.read(&mut buf) {
            Ok(0) => {
                let _ = to.shutdown(Shutdown::Write);
                return;
            }
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if to.write_all(&buf[..read]).is_err() {
            break;
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}
