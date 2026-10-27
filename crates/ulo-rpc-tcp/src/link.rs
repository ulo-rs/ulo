use std::io;
use std::sync::Arc;

use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use ulo::{AppHandle, BoundAddr, BoxError, Runtime, Timer};
use ulo_net::rustls::ServerConfig;
use ulo_net::{BoundListener, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};
use ulo_rpc_tcp_core::{Accept, Core, Incoming, Reader, Writer};
use ulo_runtime_tokio::Tokio;

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
/// ends on the clock of the runtime the client hands `connect`, the app's inside an app. A call
/// made afterwards connects again.
///
/// A server's link dropped without `close`, as it is when its app drops before the close step,
/// stops accepting: its listener closes, the connections it accepted end as at `close`, and the
/// address can be bound again. A client's link dropped without `close`, and a connection whose
/// client drops its reply lane, end as at `close` too, read for at most two seconds from the drop,
/// so a server still writing is not reset by it.
///
/// The link's sockets and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it takes the link and a server's `listen()`
/// fails, and in `connect`. A client's link also refuses in `usable` what its `connect` would
/// refuse on every call: an endpoint that is not an IP address and port, the link resolving no
/// host name, and `tls` set.
pub struct Tcp {
    core: Core,
    runtime: Option<Tokio>,
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. A
    /// server may listen on an inherited socket; a client connects to an IP address and port, such
    /// as `10.0.0.5:7000`, and is refused a host name where it takes the link.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp { core: Core::new(endpoint.into()), runtime: Tokio::try_current() }
    }

    /// TLS on the server's endpoint, loaded in `prepare`, with no ALPN. The client side speaks
    /// plain TCP: a client is refused a link with it set where it takes the link.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.core.set_tls(tls);
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.core.set_codec(codec);
        self
    }

    /// The largest frame, in bytes, read or written; a larger length prefix closes the connection
    /// before the body is read. 4 MiB unset; a value above the 4-byte prefix's range is bounded by
    /// it. Zero is refused in `prepare`.
    pub fn max_frame(mut self, bytes: u64) -> Self {
        self.core.set_max_frame(bytes);
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
        self.runtime()?;
        self.core.client_addr().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        // A server reports `usable`'s runtime refusal alone: the rest is about what a client
        // connects to, and `Core::prepare` reports the server's own endpoint.
        let usable = self.runtime().map(drop);
        self.core.prepare(usable, app.timer().cloned())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = patterns;
        let runtime = self.runtime()?;
        let handle = runtime.handle().clone();
        self.core.listen(Arc::new(runtime) as Arc<dyn Runtime>, move |listener, tls| TokioListener::adopt(&handle, listener, tls))
    }

    async fn connect(&self, handed: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let addr = self.core.client_addr()?;
        let runtime = self.runtime()?;
        let stream = runtime.run(TcpStream::connect(addr)).await??;
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
        Ok(TokioListener { tcp: TcpListener::from_std(listener.into_std())?, tls: tls.map(TlsAcceptor::from) })
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
