//! The spent-cap scenario starts its held call only once the readiness call's execution has ended.
//!
//! The link below delays the first reply's send until a gate opens, so the readiness call's
//! execution outlives the answer the client has already read. A scenario that counted `abandoned`
//! without waiting for that execution would see two; one that waits sees the held call alone.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use ulo::{AppHandle, BoundAddr, BoxError, Runtime};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Link, Outbound, Pattern, ReplyPath};
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_conformance::{Broker, Harness, OnTokio, startup_failed};
use ulo_rpc_tcp::Tcp;

/// How long the first reply's send is held after it has been written.
const HOLD: Duration = Duration::from_millis(800);

static FIRST_REPLY: AtomicBool = AtomicBool::new(true);
static OPEN: AtomicBool = AtomicBool::new(false);

struct Gated(Tcp);

fn gated(inner: ReplyPath) -> ReplyPath {
    ReplyPath::new(move |frame| {
        let inner = inner.clone();
        Box::pin(async move {
            inner.send(frame).await?;
            if FIRST_REPLY.swap(false, Ordering::SeqCst) {
                while !OPEN.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            Ok(())
        })
    })
}

impl Link for Gated {
    const NAME: &'static str = "gated-tcp";

    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.0.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.0.usable()
    }

    fn max_inflight(&mut self, calls: ulo_transport::Count) {
        self.0.max_inflight(calls);
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let inbound = self.0.listen(patterns).await?;
        Ok(inbound
            .map(|mut delivery| {
                delivery.reply = delivery.reply.take().map(gated);
                delivery
            })
            .boxed())
    }

    fn connect(&self, runtime: &std::sync::Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
        self.0.connect(runtime)
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.0.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.0.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.0.bound()
    }
}

struct Held {
    relay: Relay,
}

impl Broker for Held {
    type Harness = OnTokio;
    type Link = Gated;

    async fn start() -> Self {
        Held { relay: Relay::without_upstream().await }
    }

    fn link(&self) -> Gated {
        Gated(Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))))
    }

    fn client_link(&self, server: &[BoundAddr]) -> Gated {
        let [bound] = server else { startup_failed!("expected one bound server address, got {server:?}") };
        self.relay.forward_to(bound.addr);
        Gated(Tcp::new(self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }
}

#[test]
fn the_scenario_waits_for_the_readiness_call_s_execution_before_holding_its_own() {
    let opener = thread::spawn(|| {
        thread::sleep(HOLD);
        OPEN.store(true, Ordering::SeqCst);
    });
    OnTokio::block_on(ulo_rpc_conformance::cases::drain::spent_cap::<Held>());
    assert!(OPEN.load(Ordering::SeqCst), "the scenario ran on while the readiness call's execution was still held");
    opener.join().expect("the opener thread");
}
