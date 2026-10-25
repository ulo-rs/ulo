//! The hyper backend's `close` waits up to 500 ms for its connections to write what the drain's
//! end left them, timed by the app's clock. The app here runs on a clock that holds every sleep
//! until the test fires it, and a client that reads nothing holds its connection open past the
//! drain: the connection is an in-memory pipe carrying 1 KiB towards the client, its response an
//! endless stream, so hyper's write waits from its first poll on and never finishes. `close` asks
//! the clock for that wait once the drain window has ended, and returns once the test fires it.

use std::convert::Infallible;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use futures_util::stream;
use tokio::sync::{Notify, oneshot, watch};
use ulo::{App, BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Signal, Timer, injectable, routes};
use ulo_http::{Bytes, HttpBody, Response};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;
use ulo_server_hyper::{FuturesIo, Incoming, Listener};

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// The drain window, a length nothing else in the app asks the clock for.
const WINDOW: Duration = Duration::from_secs(7);

/// What `close` lets the connections take after the drain window: the backend's `FLUSH`.
const FLUSH: Duration = Duration::from_millis(500);

/// What the pipe towards the client holds before a write waits.
const TOWARDS_CLIENT: usize = 1024;

#[derive(Clone, Default)]
struct Entered(Arc<Notify>);

/// One end of a connection: what it reads from one pipe and writes to the other.
struct Pipe {
    read: piper::Reader,
    write: piper::Writer,
}

impl futures::AsyncRead for Pipe {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().read).poll_read(cx, buf)
    }
}

impl futures::AsyncWrite for Pipe {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().write).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().write).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().write).poll_close(cx)
    }
}

/// The connections [`Piped`] accepts, set before the app binds; one test in this binary binds it.
static PIPES: Mutex<Option<mpsc::UnboundedReceiver<Pipe>>> = Mutex::new(None);

/// A listener whose connections are the test's pipes. It keeps the socket `ulo-net` bound, which
/// nothing connects to.
struct Piped {
    _socket: BoundListener,
    incoming: Mutex<mpsc::UnboundedReceiver<Pipe>>,
}

impl Listener for Piped {
    type Io = FuturesIo<Pipe>;

    fn adopt(listener: BoundListener, _tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        let incoming = lock(&PIPES).take().ok_or_else(|| io::Error::other("no pipes were set for the piped listener"))?;
        Ok(Piped { _socket: listener, incoming: Mutex::new(incoming) })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        let pipe = poll_fn(|cx| lock(&self.incoming).poll_next_unpin(cx)).await;
        let pipe = pipe.ok_or_else(|| io::Error::other("the test's pipes are gone"))?;
        Ok(Incoming::plain(FuturesIo::new(pipe), SocketAddr::from(([127, 0, 0, 1], 1)), None))
    }
}

/// Both ends of a fresh connection, the client's first.
fn connection() -> (Pipe, Pipe) {
    let (to_server_read, to_server_write) = piper::pipe(64 * 1024);
    let (to_client_read, to_client_write) = piper::pipe(TOWARDS_CLIENT);
    (Pipe { read: to_client_read, write: to_server_write }, Pipe { read: to_server_read, write: to_client_write })
}

#[injectable]
struct Flood {
    entered: Dep<Entered>,
}

#[routes]
impl Flood {
    /// An endless stream of 64 KiB chunks, every one ready.
    #[ulo_http::get("/flood")]
    async fn flood(&self) -> Response {
        self.entered.0.notify_one();
        let chunk = Bytes::from(vec![b'x'; 64 * 1024]);
        Response::new(HttpBody::stream(stream::repeat_with(move || Ok::<_, Infallible>(chunk.clone()))))
    }
}

struct Root(Entered);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Flood>();
    }
}

/// The app's clock: every sleep is held until the test fires it, and each one asked for is
/// recorded by its length. `now` stands still, so a wait computed from a deadline asks for the
/// length it was configured with.
#[derive(Clone)]
struct Held(Arc<Sleeps>);

struct Sleeps {
    base: Instant,
    asked: Mutex<Vec<(Duration, Option<oneshot::Sender<()>>)>>,
    changed: watch::Sender<usize>,
}

impl Timer for Held {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        let (fire, fired) = oneshot::channel::<()>();
        lock(&self.0.asked).push((d, Some(fire)));
        self.0.changed.send_modify(|asked| *asked += 1);
        Box::pin(async move {
            let _ = fired.await;
        })
    }

    fn now(&self) -> Instant {
        self.0.base
    }
}

impl Held {
    fn new() -> Held {
        Held(Arc::new(Sleeps { base: Instant::now(), asked: Mutex::new(Vec::new()), changed: watch::Sender::new(0) }))
    }

    /// How many sleeps of `d` have been asked for.
    fn asked(&self, d: Duration) -> usize {
        lock(&self.0.asked).iter().filter(|(asked, _)| *asked == d).count()
    }

    /// Waits until a sleep of `d` has been asked for.
    async fn asked_for(&self, d: Duration, what: &str) {
        let mut changed = self.0.changed.subscribe();
        tokio::time::timeout(PATIENCE, changed.wait_for(|_| self.asked(d) > 0))
            .await
            .unwrap_or_else(|_| panic!("the app's clock was never asked for {what}, {d:?}: {:?}", self.lengths()))
            .expect("the clock's record");
    }

    /// Ends every sleep of `d` asked for so far.
    fn fire(&self, d: Duration) {
        for (asked, fire) in lock(&self.0.asked).iter_mut() {
            if *asked == d
                && let Some(fire) = fire.take()
            {
                let _ = fire.send(());
            }
        }
    }

    fn lengths(&self) -> Vec<Duration> {
        lock(&self.0.asked).iter().map(|(asked, _)| *asked).collect()
    }
}

// One thread: a `close` that would end in the poll that asks for its flush has ended before the
// test reads it.
#[tokio::test(flavor = "current_thread")]
async fn close_waits_out_its_flush_on_the_apps_clock_for_a_connection_that_outlives_the_drain() {
    let (entered, clock) = (Entered::default(), Held::new());
    let (connect, incoming) = mpsc::unbounded();
    *lock(&PIPES) = Some(incoming);
    let app = App::builder(Root(entered.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .drain_timeout(WINDOW)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http_hyper::ServerOn::<Piped>::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the app listens");
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });
    let (mut frozen, server_end) = connection();
    connect.unbounded_send(server_end).expect("the listener takes the connection");
    frozen.write_all(b"GET /flood HTTP/1.1\r\nHost: flush\r\n\r\n").await.expect("writes the request");
    tokio::time::timeout(PATIENCE, entered.0.notified()).await.expect("the handler is entered in time");
    let mut head = [0u8; 15];
    tokio::time::timeout(PATIENCE, frozen.read_exact(&mut head)).await.expect("the head in time").expect("the response's first bytes");
    assert_eq!(&head, b"HTTP/1.1 200 OK", "the flood's status line");
    // From here the client reads nothing.

    let closing = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.close(Signal::new("flush")).await })
    };
    clock.asked_for(WINDOW, "the drain window").await;
    assert_eq!(clock.asked(FLUSH), 0, "the flush was asked for before the drain window ended: {:?}", clock.lengths());
    clock.fire(WINDOW);
    clock.asked_for(FLUSH, "the flush").await;
    assert!(!closing.is_finished(), "close returned before its flush ended, with the connection still open");

    clock.fire(FLUSH);
    let closed = tokio::time::timeout(PATIENCE, closing).await.expect("close returned once its flush ended").expect("the close task");
    let shutdown = closed.unwrap_or_else(|error| panic!("the close failed: {error}"));
    assert_eq!(shutdown.abandoned, 1, "the flooding call, live at the drain window's end");
    assert_eq!(clock.asked(FLUSH), 1, "one flush per close: {:?}", clock.lengths());
    let served = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time").expect("the serving task");
    let served = served.unwrap_or_else(|error| panic!("serve answered the shutdown's failure: {error}"));
    assert_eq!(served.signal, Signal::new("flush"), "serve answers the signal that closed the app");

    drop(frozen);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
