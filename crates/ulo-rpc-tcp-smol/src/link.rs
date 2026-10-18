use std::io;
use std::sync::Arc;

use async_net::{TcpListener, TcpStream};
use futures_rustls::TlsAcceptor;
use futures_util::AsyncReadExt;
use ulo::{AppHandle, BoundAddr, BoxError, Runtime};
use ulo_net::rustls::ServerConfig;
use ulo_net::{BoundListener, EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};
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
/// `close` also ends every connection the link's client side opened: its socket is shut and its
/// reply lane ends, so the calls waiting on it fail `Unavailable`. A call made afterwards connects
/// again.
///
/// A server's link dropped without `close`, as it is when its app drops before the close step,
/// stops accepting: its listener and the connections it accepted close, and the address can be
/// bound again.
///
/// The sockets need no runtime of their own: `async-io` drives them from its reactor thread, so the
/// link's futures and streams may be polled on any executor or on a plain thread. Its tasks, a
/// connection's writer and reader and a server's accept loop, run on a runtime: the one
/// [`with_runtime`](Self::with_runtime) names, or else the one the link runs under, a server's app's,
/// taken in `prepare`, and a client's own, which `RpcClient` hands to `connect`: the app's inside an
/// app, the one given to `RpcClient::new` outside one.
pub struct Tcp {
    core: Core,
    runtime: Option<Arc<dyn Runtime>>,
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. A
    /// server may listen on an inherited socket; a client connects to an IP address and port, such
    /// as `10.0.0.5:7000`, and is refused a host name where it takes the link.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp { core: Core::new(endpoint.into()), runtime: None }
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
    /// is not an IP address and port, the link resolving no host name, and `tls` set.
    fn usable(&self) -> Result<(), BoxError> {
        self.core.client_addr().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        if self.runtime.is_none() {
            self.runtime = app.runtime().cloned();
        }
        let usable = self.server_runtime().map(drop);
        self.core.prepare(usable)
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        // The server routes by the `p` each frame carries; the socket takes every pattern.
        let _ = patterns;
        self.core.listen(self.server_runtime()?, SmolListener::adopt)
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let addr = self.core.client_addr()?;
        let runtime = self.runtime.clone().unwrap_or_else(|| Arc::clone(runtime));
        let stream = TcpStream::connect(addr).await?;
        let _ = stream.set_nodelay(true);
        Ok(self.core.connect(&runtime, Box::pin(stream.clone()), Box::pin(stream)))
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
