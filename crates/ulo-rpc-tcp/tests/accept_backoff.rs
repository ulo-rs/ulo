//! The accept loop paces its retries of a failed accept on the link's runtime. A runtime that has
//! shut down refuses the pause, and the loop ends there, as the drain ends it: retried at once
//! instead, it would spin on a listener that keeps failing. The listener here fails every accept,
//! and the link's runtime refuses `ACCEPT_BACKOFF`.

use std::future::pending;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ulo::{BoxFuture, Runtime, Spawn, TaskHandle, Timer};
use ulo_conformance_support::ScriptedTimer;
use ulo_net::{BoundListener, EndpointSpec};
use ulo_rpc_tcp_core::{ACCEPT_BACKOFF, Accept, Core, Incoming};
use ulo_runtime_tokio::Tokio;

/// The accepts after which a loop still retrying is taken to spin: its listener then never
/// answers again, so the test reads the count rather than hanging on it.
const TURNS: usize = 3;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// A scripted clock with tokio's spawning: the link's runtime.
struct Handed {
    clock: ScriptedTimer,
    spawn: Tokio,
}

impl Timer for Handed {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        Timer::try_sleep(&self.clock, d)
    }
}

impl Spawn for Handed {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawn.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        Spawn::try_spawn(&self.spawn, fut)
    }
}

/// A listener whose every accept fails, as one out of descriptors does, counting the accepts and
/// recording its own drop.
struct Failing {
    accepts: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
    _socket: BoundListener,
}

impl Accept for Failing {
    async fn accept(&mut self) -> io::Result<Incoming> {
        if self.accepts.fetch_add(1, Ordering::SeqCst) + 1 > TURNS {
            pending::<()>().await;
        }
        Err(io::Error::other("out of descriptors"))
    }
}

impl Drop for Failing {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_accept_loop_ends_when_its_runtime_refuses_the_backoff() {
    let clock = ScriptedTimer::new();
    clock.refuse(ACCEPT_BACKOFF);
    let (accepts, dropped) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let mut core = Core::new(EndpointSpec::from("127.0.0.1:0"));
    core.prepare(Ok(()), Some(Arc::new(ScriptedTimer::new()))).expect("the link prepares");
    let runtime: Arc<dyn Runtime> = Arc::new(Handed { clock: clock.clone(), spawn: Tokio::current() });
    let (counting, dropping) = (Arc::clone(&accepts), Arc::clone(&dropped));
    let mut inbound = core
        .listen(runtime, move |socket: BoundListener, _tls| Ok(Failing { accepts: counting, dropped: dropping, _socket: socket }))
        .expect("the link listens");

    let began = Instant::now();
    while !dropped.load(Ordering::SeqCst) && accepts.load(Ordering::SeqCst) <= TURNS {
        assert!(began.elapsed() < PATIENCE, "the accept loop neither ended nor spun within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        (accepts.load(Ordering::SeqCst), clock.refused(ACCEPT_BACKOFF)),
        (1, 1),
        "the accept loop's accepts and backoffs on a runtime refusing the backoff: it retried rather than ending"
    );
    assert!(dropped.load(Ordering::SeqCst), "the accept loop kept its listener after the refusal");
    let ended = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("the inbound stream ends with the accept loop");
    assert!(ended.is_none(), "the inbound stream yielded a delivery from a listener that accepts nothing");
}
