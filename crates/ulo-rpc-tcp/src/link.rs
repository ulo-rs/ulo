use std::io;
use std::sync::Arc;

use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use ulo::{AppHandle, BoundAddr, BoxError, Runtime};
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
/// `close` also ends every connection the link's client side opened: its socket is shut and its
/// reply lane ends, so the calls waiting on it fail `Unavailable`. A call made afterwards connects
/// again.
///
/// The link's sockets and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it takes the link and a server's `listen()`
/// fails, and in `connect`.
pub struct Tcp {
    core: Core,
    runtime: Option<Tokio>,
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. A
    /// server may listen on an inherited socket; a client connects to an address.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp { core: Core::new(endpoint.into()), runtime: Tokio::try_current() }
    }

    /// TLS on the server's endpoint, loaded in `prepare`, with no ALPN. The client side speaks
    /// plain TCP: a client link with it set refuses to connect.
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
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        let usable = self.usable();
        self.core.prepare(usable)
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = patterns;
        let runtime = self.runtime()?;
        let handle = runtime.handle().clone();
        self.core.listen(Arc::new(runtime) as Arc<dyn Runtime>, move |listener, tls| TokioListener::adopt(&handle, listener, tls))
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let addr = self.core.client_addr()?;
        let runtime = self.runtime()?;
        let stream = runtime.run(TcpStream::connect(addr)).await??;
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        let runtime: Arc<dyn Runtime> = Arc::new(runtime);
        Ok(self.core.connect(&runtime, Box::pin(reader.compat()), Box::pin(writer.compat_write())))
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
