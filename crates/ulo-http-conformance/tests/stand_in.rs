//! The chunked over-the-limit scenario against stand-in servers that answer it correctly and
//! wrongly, so the scenario is shown to hold a host to the 413 while tolerating the early close.
//!
//! Over loopback the client writes its five frames before a host's close can reach it, so a
//! real socket meets the failed write only some of the time. [`Refused`] is the client's
//! connection with that order fixed: after the frame carrying the over-limit chunk, every write
//! fails with a broken pipe, as it does once the host has closed.

use std::future::Future;
use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::Duration;

use futures_io::{AsyncRead, AsyncWrite};

use ulo::App;
use ulo::app::Connected;
use ulo_http::embed::EmbedLimits;
use ulo_http_conformance::cases::extraction::chunked_over_the_limit;
use ulo_http_conformance::{Harness, Host, Mode, OnTokio};

const WITHIN: &str = r#"{"name":"ok"}"#;
const PROBLEM: &str = r#"{"status":413,"detail":"`body` is larger than 16 bytes"}"#;

/// How the stand-in answers the body over the limit.
trait Over: Send + Sync + 'static {
    /// The bytes written once the over-limit body's first chunk has arrived, then the connection
    /// is closed without reading the rest.
    fn answer() -> Vec<u8>;
}

fn response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
        .into_bytes()
}

/// 413 with the problem document, closing before the body ends.
struct Refuses;
impl Over for Refuses {
    fn answer() -> Vec<u8> {
        response("413 Payload Too Large", "application/problem+json", PROBLEM)
    }
}

/// Closes without answering.
struct Silent;
impl Over for Silent {
    fn answer() -> Vec<u8> {
        Vec::new()
    }
}

/// Accepts the body: 200 where the limit says 413.
struct Accepts;
impl Over for Accepts {
    fn answer() -> Vec<u8> {
        response("200 OK", "application/json", WITHIN)
    }
}

/// 413 carrying the problem's text under another content type, so the content type is the one
/// check it fails.
struct Bare;
impl Over for Bare {
    fn answer() -> Vec<u8> {
        response("413 Payload Too Large", "text/plain", PROBLEM)
    }
}

struct StandIn<O, N = OnTokio> {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    _over: PhantomData<fn() -> (O, N)>,
}

/// [`OnTokio`] with a client connection whose writes fail once the over-limit chunk is written.
struct Refused;

impl Harness for Refused {
    type Runtime = <OnTokio as Harness>::Runtime;
    type Listener = <OnTokio as Harness>::Listener;
    type Stream = BreaksAfterOver<<OnTokio as Harness>::Stream>;

    fn runtime() -> Self::Runtime {
        OnTokio::runtime()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        OnTokio::block_on(fut)
    }

    async fn connect(addr: SocketAddr) -> io::Result<Self::Stream> {
        Ok(BreaksAfterOver { inner: OnTokio::connect(addr).await?, broken: false })
    }
}

struct BreaksAfterOver<S> {
    inner: S,
    broken: bool,
}

impl<S: AsyncRead + Unpin> AsyncRead for BreaksAfterOver<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for BreaksAfterOver<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.broken {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let written = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, buf));
        if buf.windows(4).any(|w| w == b"\"far") {
            self.broken = true;
        }
        Poll::Ready(written)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

fn serve<O: Over>(mut stream: TcpStream) {
    let mut seen = Vec::new();
    let mut buffer = [0u8; 512];
    // The head, then the first chunk, which is all the over-limit answer waits for.
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") || !seen.windows(2).any(|w| w == b"c\r") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => seen.extend_from_slice(&buffer[..n]),
        }
    }
    let text = String::from_utf8_lossy(&seen).into_owned();
    if text.contains("\"far") {
        let _ = stream.write_all(&O::answer());
        return;
    }
    // The body within the limit is read to its terminating chunk and echoed.
    while !seen.ends_with(b"0\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => seen.extend_from_slice(&buffer[..n]),
        }
    }
    let _ = stream.write_all(&response("200 OK", "application/json", WITHIN));
}

impl<O: Over, N: Harness> Host for StandIn<O, N> {
    type Harness = N;

    async fn start(_app: App<Connected>, _mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the stand-in binds");
        let addr = listener.local_addr().expect("the stand-in has an address");
        listener.set_nonblocking(true).expect("the listener is nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).expect("the connection blocks");
                        stream.set_read_timeout(Some(Duration::from_secs(5))).expect("a read timeout");
                        std::thread::spawn(move || serve::<O>(stream));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        StandIn { addr, stop, thread, _over: PhantomData }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
    }

    fn connections_read(&self) -> Option<usize> {
        None
    }

    async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

fn run<O: Over>() {
    <OnTokio as Harness>::block_on(chunked_over_the_limit::<StandIn<O>>(Mode::Fallback));
}

/// The write after the over-limit chunk fails on every run, so a scenario that does not tolerate
/// the failed write fails here every time.
#[test]
fn a_write_failing_after_the_host_refused_is_tolerated() {
    <Refused as Harness>::block_on(chunked_over_the_limit::<StandIn<Refuses, Refused>>(Mode::Fallback));
}

/// The failed write is tolerated, not taken for the answer: a host that closes without one still
/// fails.
#[test]
#[should_panic(expected = "a chunked body of")]
fn a_failed_write_with_no_answer_fails() {
    <Refused as Harness>::block_on(chunked_over_the_limit::<StandIn<Silent, Refused>>(Mode::Fallback));
}

#[test]
fn a_host_answering_413_and_closing_early_passes() {
    // Over a real socket the client's writes usually finish before the close reaches it; the
    // failed write itself is met every time in `a_write_failing_after_the_host_refused_is_tolerated`.
    run::<Refuses>();
}

#[test]
#[should_panic(expected = "a chunked body of")]
fn a_host_closing_without_an_answer_fails() {
    run::<Silent>();
}

#[test]
#[should_panic(expected = "a chunked body of")]
fn a_host_accepting_the_body_fails() {
    run::<Accepts>();
}

#[test]
#[should_panic(expected = "content-type: text/plain")]
fn a_413_without_the_problem_document_fails() {
    run::<Bare>();
}
