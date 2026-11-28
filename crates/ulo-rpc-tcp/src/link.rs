use std::io;
use std::sync::Arc;

use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use ulo::{AppHandle, Bound, BoundAddr, BoxError, Permanent, Runtime, Timer};
use ulo_net::rustls::ServerConfig;
use ulo_net::{BoundListener, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::config::Count;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Subscription};
use ulo_rpc_tcp_core::{Accept, Core, Incoming, Reader, Writer};
use ulo_runtime_tokio::{NoIoDriver, Tokio};

/// The TCP link: a server binds its endpoint, a client connects to it.
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
/// The link's sockets and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it takes the link and a server's `listen()`
/// fails, and in `connect`. A runtime built without its IO driver (`enable_io`) is refused the
/// same way ([`Tokio::require_io`]), a server's as a permanent `Configure`. A client's link also refuses in `usable` what
/// its `connect` would refuse on every call: an endpoint that is neither an IP address and port nor
/// a host name and port, an inherited socket, and `tls` set.
///
/// A host name, `billing:7000`, is resolved through the system resolver, `ToSocketAddrs`, so
/// `/etc/hosts`, nsswitch and the search list apply, on the held runtime's blocking pool
/// (`Runtime::spawn_blocking`). A server resolves it once, in `prepare`, binds the first of its
/// addresses that binds, logging the address bound at `info`, and reports a name resolving to no
/// address as a `Configure` refusal beside its other problems.
/// A client resolves it at every connect and tries each address in the resolver's order until one
/// connects, so a reconnect follows the name to new addresses; a name resolving to no address
/// fails that connect, the call failing `Unavailable` with the name and the resolver's error.
pub struct Tcp {
    core: Core,
    runtime: Option<Tokio>,
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. Text is
    /// an IP address and port, `10.0.0.5:7000` or `[::1]:7000`, or a host name and port,
    /// `billing:7000`. A server may also listen on an inherited socket.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp { core: Core::new(endpoint.into()), runtime: Tokio::try_current() }
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

    /// The tokio runtime the link's sockets and tasks run on, in place of the one current where
    /// it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the TCP link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Tcp {
    const NAME: &'static str = "tcp";

    fn capabilities(&self) -> Capabilities {
        self.core.capabilities()
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime()?.require_io()?;
        self.core.check_client()
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        // A server reports `usable`'s runtime refusal alone: the rest is about what a client
        // connects to, and `Core::prepare` reports the server's own endpoint.
        let usable = self.runtime().map(drop);
        // `listen` binds on the link's runtime: one that has shut down is refused here, before
        // any server binds, and alone, since `Core::prepare` reports its problems as text.
        let runtime = self.runtime().ok();
        if let Some(runtime) = &runtime {
            runtime.run(async {}).await.map_err(Permanent::new)?;
            runtime.require_io().map_err(Permanent::new)?;
        }
        self.core.prepare(usable, app.timer().cloned(), runtime.as_ref().map(|runtime| runtime as &dyn Runtime)).await
    }

    async fn listen(&self, subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = subscriptions;
        let runtime = self.runtime()?;
        // Adopted on a runtime that has shut down, the socket fails as an I/O error naming tokio's
        // context, which reads as the address's.
        runtime.run(async {}).await.map_err(Permanent::new)?;
        let handle = runtime.handle().clone();
        self.core.listen(Arc::new(runtime) as Arc<dyn Runtime>, move |listener, tls| TokioListener::adopt(&handle, listener, tls))
    }

    async fn connect(&self, handed: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.core.check_client()?;
        let runtime = self.runtime()?;
        let stream = self.core.dial(&runtime, |addr| {
            let connecting = runtime.run(TcpStream::connect(addr));
            async move { Ok(connecting.await??) }
        })
        .await?;
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        let runtime: Arc<dyn Runtime> = Arc::new(runtime);
        // The connection's tasks run on the link's runtime and its grace on the handed one's clock.
        let timer = Arc::clone(handed) as Arc<dyn Timer>;
        Ok(self.core.connect(&runtime, timer, Box::pin(reader.compat()), Box::pin(writer.compat_write())))
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

/// A socket registered with the link's tokio runtime, whose tasks accept on it.
struct TokioListener {
    tcp: TcpListener,
    tls: Option<TlsAcceptor>,
}

impl TokioListener {
    fn adopt(handle: &Handle, listener: BoundListener, tls: Option<Arc<ServerConfig>>) -> io::Result<TokioListener> {
        let _entered = handle.enter();
        let std = listener.into_std();
        // Backstop for a probe that could not find out: registering on a runtime without its IO
        // driver panics.
        let tcp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| TcpListener::from_std(std)))
            .map_err(|_| io::Error::other(NoIoDriver))??;
        Ok(TokioListener { tcp, tls: tls.map(TlsAcceptor::from) })
    }
}

impl Accept for TokioListener {
    async fn accept(&mut self) -> io::Result<Incoming> {
        let (stream, peer) = self.tcp.accept().await?;
        let _ = stream.set_nodelay(true);
        let Some(acceptor) = self.tls.clone() else {
            let (reader, writer) = stream.into_split();
            return Ok(Incoming::plain(peer, Box::pin(reader.compat()), Box::pin(writer.compat_write())));
        };
        Ok(Incoming::handshaking(peer, async move {
            let stream = acceptor.accept(stream).await?;
            Ok(halves(stream))
        }))
    }
}

/// A TLS connection's two halves, each on `futures-io`'s traits.
fn halves(stream: TlsStream<TcpStream>) -> (Reader, Writer) {
    let (reader, writer): (ReadHalf<_>, WriteHalf<_>) = tokio::io::split(stream);
    (Box::pin(reader.compat()), Box::pin(writer.compat_write()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use ulo_net::Lookup;

    use super::*;

    /// How long a test waits for anything before it fails.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// A listener on a loopback port, accepting on the test's runtime.
    async fn listening() -> TcpListener {
        TcpListener::bind("127.0.0.1:0").await.expect("a loopback port")
    }

    async fn accepted(listener: &TcpListener, which: &str) {
        tokio::time::timeout(PATIENCE, listener.accept())
            .await
            .unwrap_or_else(|_| panic!("{which} accepted no connection"))
            .unwrap_or_else(|error| panic!("{which} failed to accept: {error}"));
    }

    fn on(endpoint: &str, lookup: Lookup) -> Tcp {
        let mut tcp = Tcp::new(endpoint);
        tcp.core.set_lookup(lookup);
        tcp
    }

    /// F416: each connect looks the name up again, so a connect made after the name's address
    /// changed reaches the new one. The lookup answers the first listener, then the second.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_connect_resolves_the_name_again() {
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
        let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());
        let _one = tcp.connect(&runtime).await.expect("the first connect");
        accepted(&first, "the address the name had at the first connect").await;
        let _two = tcp.connect(&runtime).await.expect("the second connect");
        accepted(&second, "the address the name had at the second connect").await;
        assert_eq!(asked.load(Ordering::SeqCst), 2, "the lookups two connects made");
    }

    /// F416: a connect tries each address the name resolves to, in order, until one connects.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_connect_tries_each_address_until_one_connects() {
        let closed = ulo_conformance_support::relay::refusing_addr();
        let live = listening().await;
        let answers = vec![closed, live.local_addr().expect("an address")];
        let tcp = on("billing:7000", Lookup::from_fn(move |_| Ok(answers.clone())));
        let _connected = tcp.connect(&(Arc::new(Tokio::current()) as Arc<dyn Runtime>)).await.expect("a connect past a refused address");
        accepted(&live, "the second address").await;
    }

    /// F416: the lookup runs as the runtime's blocking work, off the thread polling the connect. On a
    /// current-thread runtime the lookup waits for a release that a task on that thread sends, which
    /// it can send only while the connect leaves the thread free.
    #[tokio::test(flavor = "current_thread")]
    async fn a_connect_resolves_off_the_thread_polling_it() {
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
        let releasing = tokio::spawn(async move {
            let _ = release.send(());
        });
        let connected = tcp.connect(&(Arc::new(Tokio::current()) as Arc<dyn Runtime>)).await;
        assert!(connected.is_ok(), "the connect failed: {:?}", connected.err());
        accepted(&live, "the address the lookup answered").await;
        releasing.await.expect("the releasing task ends");
    }

    /// F416: a link whose runtime has shut down runs no lookup, and its connect fails as
    /// `RuntimeShutDown`, as every other refusal of that runtime does, not as an unresolved name.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_lookup_its_runtime_refuses_is_runtime_shut_down() {
        let stopped = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("a runtime to shut down");
        let asked = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&asked);
        let tcp = on(
            "billing:7000",
            Lookup::from_fn(move |_| {
                counting.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other("the lookup ran on a runtime that has shut down"))
            }),
        )
        .with_handle(stopped.handle().clone());
        std::thread::spawn(move || drop(stopped)).join().expect("the runtime shuts down");
        let refused = tcp.connect(&(Arc::new(Tokio::current()) as Arc<dyn Runtime>)).await.err().expect("a connect on a runtime that has shut down");
        assert!(refused.is::<ulo::RuntimeShutDown>(), "the connect failed as `{refused}`, not as `RuntimeShutDown`");
        assert_eq!(asked.load(Ordering::SeqCst), 0, "the lookups a runtime that has shut down ran");
    }

    /// A client whose name resolves to nothing is built, the name being well formed, and each call
    /// fails `Unavailable`, naming it, since a later connect may find it. The lookup is planted:
    /// answering at once, it does not depend on a resolver's time, which a machine with its
    /// network down may spend past any bound.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_client_on_a_name_resolving_to_nothing_fails_each_call_unavailable() {
        let tcp = on("nosuch.invalid:7000", Lookup::from_fn(|_| Err(io::Error::new(io::ErrorKind::NotFound, "no such host"))));
        let rpc = ulo_rpc::RpcClient::new(tcp, Arc::new(Tokio::current())).expect("a client on a well-formed host name is built");
        for _ in 0..2 {
            let failed = rpc.request::<_, u32>("client_address.add", &1).timeout(PATIENCE).await.expect_err("a call to a name resolving to nothing");
            assert_eq!(failed.kind(), ulo_transport::ErrorKind::Unavailable, "the call to `nosuch.invalid:7000` answered {failed:?}, where a `Timeout` is its {PATIENCE:?} passing while it waited on the lookup");
            assert!(failed.message().contains("`nosuch.invalid:7000` resolved to no address"), "the failure does not name the host: {failed:?}");
        }
    }
}
