//! What the example tests share: a bare HTTP/1.1 client (one request on one connection, the reply
//! read to the end, standing in for `curl`), a way to serve an app in a task, a scripted-clock
//! driver, and [`Bin`], which runs an example's compiled binary for its smoke test.

mod bin;
pub mod graphql;
pub mod http;
pub mod rpc;
pub mod ws;

pub use bin::{Bin, Exit, GUARD};

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use ulo::app::Bound;
use ulo::{App, AppBuilder, AppHandle, Connected, Shutdown, ShutdownError, Signal};
use ulo::testing::ScriptedTimer;
use ulo_http_hyper::Server;
use ulo_runtime_tokio::Tokio;

/// A bound app serving in a task, as a test drives it: `stop` closes it through its handle and
/// returns what `serve` returned.
pub struct Served {
    pub addr: SocketAddr,
    pub handle: AppHandle,
    task: JoinHandle<Result<Shutdown, ShutdownError>>,
}

/// Wires, connects and binds `app` on the tokio runtime and an HTTP server on port 0, then serves it.
/// Panics on a startup failure; a test that expects one runs the phases itself.
pub async fn start(app: AppBuilder) -> Served {
    start_with(app.runtime(Tokio::current())).await
}

/// As [`start`], on the tokio runtime under `clock`: a drain window, a bound or a backoff of the
/// app passes when the test advances the clock and not before.
pub async fn start_on(app: AppBuilder, clock: ScriptedTimer) -> Served {
    start_with(app.runtime(Tokio::current()).timer(clock)).await
}

async fn start_with(app: AppBuilder) -> Served {
    listen(app.wire().expect("the app wires").connect().await.expect("the app connects")).await
}

/// Binds a connected app to an HTTP server on port 0 and serves it; for an app a `TestApp` built.
pub async fn listen(app: App<Connected>) -> Served {
    serve(app.bind(Server::new("127.0.0.1:0")).listen().await.expect("the app listens"))
}

/// Starts serving `app` with no signal of its own, so only `stop` (or a `close` on `handle`) ends it.
/// `addr` is the first address the app bound.
pub fn serve(app: App<Bound>) -> Served {
    let addr = app.addresses().first().expect("the app bound an address").addr;
    let handle = app.handle();
    let task = tokio::spawn(app.serve(std::future::pending()));
    Served { addr, handle, task }
}

impl Served {
    /// Closes under the signal `test` and returns the shutdown `serve` reported.
    pub async fn stop(self) -> Result<Shutdown, ShutdownError> {
        let closed = self.handle.close(Signal::new("test")).await;
        let served = self.task.await.expect("serve joins");
        assert_eq!(closed.as_ref().map(|s| s.signal.name()).ok(), served.as_ref().map(|s| s.signal.name()).ok());
        served
    }
}

/// What the server answered.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    /// Header names lowercased, in the order received.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

pub async fn get(addr: SocketAddr, path: &str) -> Reply {
    send(addr, &format!("GET {path} HTTP/1.1\r\nhost: example\r\nconnection: close\r\n\r\n")).await
}

/// Writes `request` verbatim and parses what comes back until the server closes the connection.
pub async fn send(addr: SocketAddr, request: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).await.expect("connect to the example server");
    stream.write_all(request.as_bytes()).await.expect("write the request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read the reply");
    parse(&raw)
}

fn parse(raw: &[u8]) -> Reply {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").expect("a reply with a head");
    let mut lines = head.split("\r\n");
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).expect("a status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked");
    let body = if chunked { dechunk(body) } else { body.to_owned() };
    Reply { status, headers, body }
}

fn dechunk(mut rest: &str) -> String {
    let mut out = String::new();
    while let Some((size, tail)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        out.push_str(&tail[..size]);
        rest = &tail[size + 2..];
    }
    out
}

/// Polls `fut` to its end while moving `clock` forward by `step` each time a sleep of exactly
/// `step` is waiting on it, so a backoff or a bound of that length passes without anyone waiting
/// for it. What the code under test does is decided by the clock alone; the ten seconds of real
/// time it allows only turn a future that never ends into a failure instead of a hang.
pub async fn advancing<T>(clock: &ScriptedTimer, step: Duration, fut: impl Future<Output = T>) -> T {
    let mut fut = std::pin::pin!(fut);
    let driven = async {
        loop {
            tokio::select! {
                out = &mut fut => return out,
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    if clock.live(step) > 0 {
                        clock.advance(step);
                    }
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), driven).await.expect("the future ended within ten seconds of real time")
}
