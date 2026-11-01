//! The accept loop paces its retries of a failed accept on the app's runtime. A runtime that has
//! shut down refuses the pause, and the loop ends there, as the drain ends it: retried at once
//! instead, it would spin on a listener that keeps failing. The listener here fails every accept,
//! and the runtime refuses `ACCEPT_BACKOFF`.

mod support;

use std::future::pending;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_net::TcpStream;
use ulo::{BoxFuture, Runtime, Spawn, TaskHandle, Timer};
use ulo_conformance_support::ScriptedTimer;
use ulo_net::rustls::ServerConfig;
use ulo_net::{BoundListener, Endpoint};
use ulo_runtime_smol::Smol;
use ulo_server_hyper::{ACCEPT_BACKOFF, FuturesIo, Incoming, Listener, Serve, ServeConfig};

use support::{on_smol, within};

/// The accepts after which a loop still retrying is taken to spin: its listener then never
/// answers again, so the test reads the count rather than hanging on it.
const TURNS: usize = 3;

static ACCEPTS: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicBool = AtomicBool::new(false);

/// A listener whose every accept fails, as one out of descriptors does.
struct Failing {
    _socket: BoundListener,
}

impl Listener for Failing {
    type Io = FuturesIo<TcpStream>;

    fn adopt(listener: BoundListener, _tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        Ok(Failing { _socket: listener })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        if ACCEPTS.fetch_add(1, Ordering::SeqCst) + 1 > TURNS {
            pending::<()>().await;
        }
        Err(io::Error::other("out of descriptors"))
    }
}

impl Drop for Failing {
    fn drop(&mut self) {
        DROPPED.store(true, Ordering::SeqCst);
    }
}

/// A scripted clock with smol's spawning.
struct Handed {
    clock: ScriptedTimer,
    spawn: Smol,
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

#[test]
fn the_accept_loop_ends_when_its_runtime_refuses_the_backoff() {
    on_smol(|smol| async move {
        let clock = ScriptedTimer::new();
        clock.refuse(ACCEPT_BACKOFF);
        let runtime: Arc<dyn Runtime> = Arc::new(Handed { clock: clock.clone(), spawn: smol.clone() });
        let listener = BoundListener::bind(&Endpoint::parse("127.0.0.1:0").expect("an address")).expect("a port");
        let serve = Arc::new(Serve::<Failing>::new(vec![listener], None, &ServeConfig::default(), runtime).expect("adopted"));
        let running = {
            let serve = Arc::clone(&serve);
            smol.spawn(Box::pin(async move {
                let _ = serve.run(|_accepted| async {}).await;
            }))
        };
        within("the accept loop's end or its spin", async {
            while !DROPPED.load(Ordering::SeqCst) && ACCEPTS.load(Ordering::SeqCst) <= TURNS {
                smol.sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert_eq!(
            (ACCEPTS.load(Ordering::SeqCst), clock.refused(ACCEPT_BACKOFF)),
            (1, 1),
            "the accept loop's accepts and backoffs on a runtime refusing the backoff: it retried rather than ending"
        );
        assert!(DROPPED.load(Ordering::SeqCst), "the accept loop kept its listener after the refusal");
        assert!(within("the serve loop's end", running).await.is_finished(), "`run` outlived its only accept loop");
    });
}
