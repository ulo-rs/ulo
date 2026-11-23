use std::io;
use std::sync::Arc;

use async_net::{TcpListener, TcpStream};
use futures_rustls::TlsAcceptor;
use futures_util::AsyncReadExt;
use ulo::{AppHandle, Bound, BoundAddr, BoxError, Runtime, Timer};
use ulo_net::rustls::ServerConfig;
use ulo_net::{BoundListener, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::config::Count;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Subscription};
use ulo_rpc_tcp_core::{Accept, Core, Incoming};

/// The TCP link on `async-io`'s sockets: a server binds its endpoint, a client connects to it.
///
/// A server maps each connection's call ids to ids of its own, so calls on two connections never
/// share one, and writes the caller's id back on every reply. A connection that closes cancels its
/// calls in flight `Disconnected`. At the drain the listener closes and every connection receives
/// `goaway`, one registered after the drain began included, while its calls in flight finish. The
/// inbound stream ends once the draining server holds no call, and a request arriving afterwards,
/// on a connection the caller kept open, is answered `err` of kind `unavailable` by the link, as
/// the server answers one during the drain.
///
/// At `close` every connection the server accepted ends: its writer shuts its half once the
/// answers queued before the close are written, and the link reads and discards what the peer
/// still sends until the peer closes its half or the connection fails. Two seconds on the app's
/// `Timer` after `close` ends, completed or cut, every connection still open drops, a writer
/// mid-answer included, so a peer that has stopped reading loses what it has not read. `close`
/// does not wait for that. A peer that is still writing as the server closes, and reading,
/// receives what is written in that time and then the server's FIN, not a reset.
///
/// `close` also ends every connection the link's client side opened: its writer shuts its half
/// once the frames queued before the close are written, and its reply lane ends, so the calls
/// waiting on it fail `Unavailable`. The link reads and discards what the server still sends
/// until the server closes its half or the connection fails, for at most two seconds after `close`
/// ends on the clock of the runtime the client hands `connect`, the app's inside an app. The link
/// connects again for a later `connect`; an `RpcClient` closed with it makes none, a closed client
/// staying closed.
///
/// A server's link dropped without `close`, as it is when its app drops before the close step,
/// stops accepting: its listener closes, the connections it accepted end as at `close`, and the
/// address can be bound again. A client's link dropped without `close`, and a connection whose
/// client drops its reply lane, end as at `close` too, read for at most two seconds from the drop,
/// so a server still writing is not reset by it. A client whose runtime has shut down cannot bound
/// a linger, and drops its connection without one.
///
/// The sockets need no runtime of their own: `async-io` drives them from its reactor thread, so the
/// link's futures and streams may be polled on any executor or on a plain thread. Its tasks, a
/// connection's writer and reader and a server's accept loop, run on a runtime: the one
/// [`with_runtime`](Self::with_runtime) names, or else the one the link runs under, a server's app's,
/// taken in `prepare`, and a client's own, which `RpcClient` hands to `connect`: the app's inside an
/// app, the one given to `RpcClient::new` outside one.
///
/// A host name, `billing:7000`, is resolved through the system resolver, `ToSocketAddrs`, so
/// `/etc/hosts`, nsswitch and the search list apply, as that runtime's blocking work
/// (`Runtime::spawn_blocking`, `blocking::unblock` on `ulo_runtime_smol::Smol`). A server resolves
/// it once, in `prepare`, binds the first of its addresses that binds, logging the address bound at
/// `info`, and reports a name resolving to no address as a
/// `Configure` refusal beside its other problems. A client resolves it at every connect and tries
/// each address in the resolver's order until one connects, so a reconnect follows the name to new
/// addresses; a name resolving to no address fails that connect, the call failing `Unavailable`
/// with the name and the resolver's error.
pub struct Tcp {
    core: Core,
    runtime: Option<Arc<dyn Runtime>>,
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. Text is
    /// an IP address and port, `10.0.0.5:7000` or `[::1]:7000`, or a host name and port,
    /// `billing:7000`. A server may also listen on an inherited socket.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp { core: Core::new(endpoint.into()), runtime: None }
    }

    /// TLS on the server's endpoint, loaded in `prepare`, with no ALPN. The client side speaks
    /// plain TCP: a client is refused a link with it set where it takes the link.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.core.set_tls(tls);
        self
    }

    /// `Codec::Cbor` carries raw bytes and reports `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.core.set_codec(codec);
        self
    }

    /// The largest frame, in bytes, read or written. A larger frame a server reads to its declared
    /// end and drops, scanning its first 4 KiB for the call it names: where those bytes name one,
    /// `t` and `id` ahead of the payload as `RpcClient` writes them, the call is answered
    /// `bad_request` with `payload_too_large`, and the connection reads the next frame either way.
    /// 1 MiB unset, which bounds the request frames a server holds at 1 GiB under its default
    /// `max_inflight`; a value above the 4-byte prefix's range is bounded by it. Zero is refused in
    /// `prepare`, and so is a value below the size of that answer, 246 bytes in JSON and 198 in
    /// CBOR at a three-digit limit, which a server could not write.
    pub fn max_frame(mut self, bytes: u64) -> Self {
        self.core.set_max_frame(bytes);
        self
    }

    /// How long a server connection stays open idle: 60 s at `Bound::Default`
    /// ([`DEFAULT_IDLE_TIMEOUT`](ulo_rpc_tcp_core::DEFAULT_IDLE_TIMEOUT)), with no limit at
    /// `Bound::Unbounded`. A connection is idle while it holds no call in flight, a streamed request
    /// or reply included, and no frame is read whole from it or written whole to it; the time runs
    /// from the latest of those and of its last call's end. A frame still arriving counts once it
    /// is whole, so on a connection holding no call the timeout also bounds how long one frame
    /// takes to arrive, and it bounds a TLS handshake. A connection is idle too, whatever calls it
    /// holds, once its writer has held a frame for the timeout without writing it whole: its peer
    /// reads too slowly or not at all, and its calls, a streamed reply waiting for room among them,
    /// are cancelled `Disconnected`, so a peer cannot hold a connection and its calls' places by
    /// reading nothing or a byte at a time. A peer taking part of a frame within each timeout is
    /// no progress, and a reply larger than a peer can read within the timeout is what `max_frame`
    /// bounds. At the expiry the server queues
    /// `goaway`, shuts its half once what was queued is written, and reads and discards what the
    /// peer still sends for two seconds on the app's `Timer`, as at `close`. `RpcClient` reads the
    /// `goaway` and connects again for its next call; a request written before it read the
    /// `goaway` is never read by the server and fails `unavailable` with nothing run for it.
    /// Timed on the app's `Timer`; `Bound::After(Duration::ZERO)` is refused in `prepare`. A
    /// client's connections have no idle timeout.
    pub fn idle_timeout(mut self, timeout: Bound) -> Self {
        self.core.set_idle_timeout(timeout);
        self
    }

    /// The most connections a server holds open at once, with no limit at `Count::Default` and
    /// `Count::Unlimited`. At the bound the server accepts no connection until one of its
    /// connections stops being read; those that arrive meanwhile wait in the listener's backlog,
    /// which the OS bounds and past which it refuses them. A connection counts from its accept,
    /// its TLS handshake included, until it stops being read, so one lingering after `close` or
    /// its idle expiry does not count. With no limit, the connections are bounded by the process's
    /// descriptor limit and each holding no call by `idle_timeout`: one open connection holds the bytes of the
    /// frame it is reading, at most `max_frame`, a 16 KiB buffer while it drops an over-limit
    /// frame, and its writer's queue, which its reader keeps near 64 frames by waiting for room,
    /// plus what is still on its way from the server. The calls across every connection are
    /// bounded by the server's `max_inflight`. `Count::Max(0)` is refused in `prepare`.
    pub fn max_connections(mut self, connections: Count) -> Self {
        self.core.set_max_connections(connections);
        self
    }

    /// The runtime the link's tasks run on in place of the one it runs under: `ulo_runtime_smol::Smol`, or
    /// any other. Unset, a server's tasks run on its app's runtime and a client's on its client's.
    pub fn with_runtime(mut self, runtime: impl Runtime) -> Self {
        self.runtime = Some(Arc::new(runtime));
        self
    }

    fn server_runtime(&self) -> Result<Arc<dyn Runtime>, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the smol TCP link's server has no runtime for its tasks: its app has none and the link was given none with `.with_runtime(..)`";

impl Link for Tcp {
    const NAME: &'static str = "tcp";

    fn capabilities(&self) -> Capabilities {
        self.core.capabilities()
    }

    /// A client's link refuses here what its `connect` would refuse on every call: an endpoint that
    /// is neither an IP address and port nor a host name and port, an inherited socket, and `tls`
    /// set.
    fn usable(&self) -> Result<(), BoxError> {
        self.core.check_client()
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        if self.runtime.is_none() {
            self.runtime = app.runtime().cloned();
        }
        let usable = self.server_runtime().map(drop);
        let runtime = self.runtime.clone();
        self.core.prepare(usable, app.timer().cloned(), runtime.as_deref()).await
    }

    async fn listen(&self, subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = subscriptions;
        self.core.listen(self.server_runtime()?, SmolListener::adopt)
    }

    async fn connect(&self, handed: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.core.check_client()?;
        let runtime = self.runtime.clone().unwrap_or_else(|| Arc::clone(handed));
        let stream = self.core.dial(&*runtime, |addr| async move { Ok(TcpStream::connect(addr).await?) }).await?;
        let _ = stream.set_nodelay(true);
        let timer = Arc::clone(handed) as Arc<dyn Timer>;
        Ok(self.core.connect(&runtime, timer, Box::pin(stream.clone()), Box::pin(stream)))
    }

    async fn drain(&self) {
        self.core.drain().await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.core.close().await;
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.core.bound()
    }
}

/// One socket adopted into `async-io`'s reactor, with its TLS acceptor when the server has TLS.
struct SmolListener {
    tcp: TcpListener,
    tls: Option<TlsAcceptor>,
}

impl SmolListener {
    fn adopt(listener: BoundListener, tls: Option<Arc<ServerConfig>>) -> io::Result<SmolListener> {
        Ok(SmolListener { tcp: TcpListener::try_from(listener.into_std())?, tls: tls.map(TlsAcceptor::from) })
    }
}

impl Accept for SmolListener {
    async fn accept(&mut self) -> io::Result<Incoming> {
        let (stream, peer) = self.tcp.accept().await?;
        let _ = stream.set_nodelay(true);
        let Some(acceptor) = self.tls.clone() else {
            return Ok(Incoming::plain(peer, Box::pin(stream.clone()), Box::pin(stream)));
        };
        Ok(Incoming::handshaking(peer, async move {
            let (reader, writer) = acceptor.accept(stream).await?.split();
            Ok((Box::pin(reader) as _, Box::pin(writer) as _))
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use ulo_net::Lookup;
    use ulo_rpc_conformance::{Harness, OnSmol};
    use ulo_runtime_smol::{Executor, Smol};

    use super::*;

    /// How long a test waits for anything before it fails.
    const PATIENCE: Duration = Duration::from_secs(5);

    async fn listening() -> TcpListener {
        TcpListener::bind("127.0.0.1:0").await.expect("a loopback port")
    }

    async fn accepted(timer: &dyn Timer, listener: &TcpListener, which: &str) {
        ulo::timeout(timer, PATIENCE, listener.accept())
            .await
            .unwrap_or_else(|_| panic!("{which} accepted no connection"))
            .unwrap_or_else(|error| panic!("{which} failed to accept: {error}"));
    }

    fn on(endpoint: &str, lookup: Lookup) -> Tcp {
        let mut tcp = Tcp::new(endpoint);
        tcp.core.set_lookup(lookup);
        tcp
    }

    /// F416: each connect looks the name up again on the runtime the client hands it, so a
    /// connect made after the name's address changed reaches the new one.
    #[test]
    fn each_connect_resolves_the_name_again() {
        OnSmol::block_on(async {
            let (first, second) = (listening().await, listening().await);
            let answers = [first.local_addr().expect("an address"), second.local_addr().expect("an address")];
            let asked = Arc::new(AtomicUsize::new(0));
            let counting = Arc::clone(&asked);
            let tcp = on(
                "billing:7000",
                Lookup::from_fn(move |host| {
                    assert_eq!(host.to_string(), "billing:7000", "the name looked up");
                    Ok(vec![answers[counting.fetch_add(1, Ordering::SeqCst).min(1)]])
                }),
            );
            let runtime: Arc<dyn Runtime> = Arc::new(OnSmol::runtime());
            let _one = tcp.connect(&runtime).await.expect("the first connect");
            accepted(&*runtime, &first, "the address the name had at the first connect").await;
            let _two = tcp.connect(&runtime).await.expect("the second connect");
            accepted(&*runtime, &second, "the address the name had at the second connect").await;
            assert_eq!(asked.load(Ordering::SeqCst), 2, "the lookups two connects made");
        });
    }

    /// F416: a connect tries each address the name resolves to, in order, until one connects.
    #[test]
    fn a_connect_tries_each_address_until_one_connects() {
        OnSmol::block_on(async {
            let closed = ulo_conformance_support::relay::refusing_addr();
            let live = listening().await;
            let answers = vec![closed, live.local_addr().expect("an address")];
            let tcp = on("billing:7000", Lookup::from_fn(move |_| Ok(answers.clone())));
            let runtime: Arc<dyn Runtime> = Arc::new(OnSmol::runtime());
            let _connected = tcp.connect(&runtime).await.expect("a connect past a refused address");
            accepted(&*runtime, &live, "the second address").await;
        });
    }

    /// F416: the lookup runs as the runtime's blocking work, off every thread running its
    /// executor. With the test's thread the executor's only one, the lookup waits for a release
    /// that a task on that executor sends, which it can send only while the connect leaves the
    /// thread free.
    #[test]
    fn a_connect_resolves_off_the_thread_polling_it() {
        let smol = Smol::new(Arc::new(Executor::new()));
        let runtime: Arc<dyn Runtime> = Arc::new(smol.clone());
        smol.block_on(async move {
            let live = listening().await;
            let addr = live.local_addr().expect("an address");
            let (release, released) = mpsc::channel::<()>();
            let released = Mutex::new(released);
            let tcp = on(
                "billing:7000",
                Lookup::from_fn(move |_| match released.lock().expect("the release's lock").recv_timeout(PATIENCE) {
                    Ok(()) => Ok(vec![addr]),
                    Err(_) => Err(io::Error::other("the lookup's release never came: the lookup held the thread polling the connect")),
                }),
            );
            let releasing = runtime.spawn(Box::pin(async move {
                let _ = release.send(());
            }));
            let connected = tcp.connect(&runtime).await;
            assert!(connected.is_ok(), "the connect failed: {:?}", connected.err());
            accepted(&*runtime, &live, "the address the lookup answered").await;
            assert!(releasing.await.is_finished(), "the releasing task ends");
        });
    }
}
