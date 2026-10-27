//! A server link whose connections' reads wait at a gate and hand over at most `CAP` bytes each,
//! counted into the starvation case's meter. The gate holds every read until the test has written
//! its flood, so the flood is in the kernel before the loop under test first reads and the loop
//! never meets a read that would wait; the cap makes the bytes handed over count the reads the
//! loop has made. `ulo-rpc-tcp-core`'s `Core` is the link, as `ulo-rpc-tcp-smol`'s `Tcp` builds
//! it, over a listener that wraps each accepted socket's reader.

#![allow(dead_code)]

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use async_net::{TcpListener, TcpStream};
use futures_util::io::AsyncRead;
use ulo::Runtime;
use ulo_conformance_support::ScriptedTimer;
use ulo_conformance_support::starvation::{Meter, Probe};
use ulo_net::{BoundListener, EndpointSpec};
use ulo_rpc::link::Inbound;
use ulo_rpc_tcp_core::{Accept, Core, Incoming};

/// The most bytes one read of a connection's socket hands over.
pub const CAP: usize = 64;

pub struct Gate {
    open: AtomicBool,
    waker: Mutex<Option<Waker>>,
    parks: AtomicUsize,
}

impl Gate {
    pub fn release(&self) {
        self.open.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap_or_else(PoisonError::into_inner).take() {
            waker.wake();
        }
    }

    /// How many reads have found the gate shut.
    pub fn parks(&self) -> usize {
        self.parks.load(Ordering::Acquire)
    }
}

struct Gated {
    inner: TcpStream,
    gate: Arc<Gate>,
    meter: Meter,
}

impl AsyncRead for Gated {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if !this.gate.open.load(Ordering::Acquire) {
            *this.gate.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(cx.waker().clone());
            if !this.gate.open.load(Ordering::Acquire) {
                this.gate.parks.fetch_add(1, Ordering::AcqRel);
                return Poll::Pending;
            }
        }
        let room = buf.len().min(CAP);
        let read = match Pin::new(&mut this.inner).poll_read(cx, &mut buf[..room]) {
            Poll::Ready(Ok(read)) => read,
            other => return other,
        };
        this.meter.add(read as u64);
        Poll::Ready(Ok(read))
    }
}

struct GatedListener {
    tcp: TcpListener,
    gate: Arc<Gate>,
    meter: Meter,
}

impl Accept for GatedListener {
    async fn accept(&mut self) -> io::Result<Incoming> {
        let (stream, peer) = self.tcp.accept().await?;
        let _ = stream.set_nodelay(true);
        let reader = Gated { inner: stream.clone(), gate: Arc::clone(&self.gate), meter: self.meter.clone() };
        Ok(Incoming::plain(peer, Box::pin(reader), Box::pin(stream)))
    }
}

pub struct Server {
    pub core: Core,
    pub inbound: Inbound,
    pub addr: SocketAddr,
    pub gate: Arc<Gate>,
    pub clock: ScriptedTimer,
}

/// A link listening on port 0 on the case's runtime, timing its linger on a scripted clock.
pub fn serve(probe: &Probe) -> Server {
    let clock = ScriptedTimer::new();
    let gate = Arc::new(Gate { open: AtomicBool::new(false), waker: Mutex::new(None), parks: AtomicUsize::new(0) });
    let mut core = Core::new(EndpointSpec::from("127.0.0.1:0"));
    core.prepare(Ok(()), Some(Arc::new(clock.clone()))).expect("the link prepares");
    let (accepting, meter) = (Arc::clone(&gate), probe.meter());
    let runtime: Arc<dyn Runtime> = Arc::new(probe.runtime());
    let inbound = core
        .listen(runtime, move |listener: BoundListener, _tls| {
            Ok(GatedListener { tcp: TcpListener::try_from(listener.into_std())?, gate: accepting, meter })
        })
        .expect("the link listens");
    let addr = core.bound().first().map(|bound| bound.addr).expect("the link's address");
    Server { core, inbound, addr, gate, clock }
}
