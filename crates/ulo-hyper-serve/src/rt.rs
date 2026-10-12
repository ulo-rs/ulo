//! hyper's runtime edges over the app's: its executor and timer over a `ulo` `Runtime` and
//! `Timer`, and its I/O traits over `futures-io`'s.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use hyper::rt::{Executor, Read, ReadBuf, ReadBufCursor, Sleep, Timer, Write};
use ulo::{BoxFuture, Runtime};

/// hyper's executor over a `ulo` [`Runtime`]: each future hyper hands it, HTTP/2's stream and
/// connection tasks among them, is spawned on the runtime and detached. Built from the app's
/// runtime, `Mounted::runtime()` in a server's `bind`, so a connection's tasks run on the runtime
/// the app runs on and a panic in one is redacted with the app's secrets.
#[derive(Clone)]
pub struct RuntimeExecutor {
    runtime: Arc<dyn Runtime>,
}

impl RuntimeExecutor {
    pub fn new(runtime: Arc<dyn Runtime>) -> RuntimeExecutor {
        RuntimeExecutor { runtime }
    }
}

impl<F> Executor<F> for RuntimeExecutor
where
    F: Future + Send + 'static,
{
    fn execute(&self, fut: F) {
        // Dropping the handle detaches the task, which hyper's own executors do too.
        drop(self.runtime.spawn(Box::pin(async move {
            fut.await;
        })));
    }
}

/// hyper's timer over a `ulo` [`Timer`](ulo::Timer): the HTTP/1.1 header-read timeout, and any
/// other clock a connection builder reads, on the app's clock. A deadline is measured against that
/// clock's `now`, which hyper reads through [`Timer::now`] too, so a test's fake clock is the one
/// both sides agree on.
#[derive(Clone)]
pub struct RuntimeTimer {
    timer: Arc<dyn ulo::Timer>,
}

impl RuntimeTimer {
    pub fn new(timer: Arc<dyn ulo::Timer>) -> RuntimeTimer {
        RuntimeTimer { timer }
    }
}

impl Timer for RuntimeTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Sleep>> {
        Box::pin(AppSleep(Mutex::new(self.timer.sleep(duration))))
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Sleep>> {
        self.sleep(deadline.saturating_duration_since(self.timer.now()))
    }

    fn now(&self) -> Instant {
        self.timer.now()
    }
}

/// One of the app timer's sleeps as hyper's `Sleep`, which must be `Sync`. The mutex is what makes
/// it so; it is only ever reached through `&mut`, so it is never contended.
struct AppSleep(Mutex<BoxFuture<'static, ()>>);

impl Future for AppSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.get_mut().0.get_mut().unwrap_or_else(PoisonError::into_inner).as_mut().poll(cx)
    }
}

impl Sleep for AppSleep {}

/// The most one read through [`FuturesIo`] asks of the stream it wraps.
const SCRATCH: usize = 64 * 1024;

/// A stream between `futures-io`'s traits and hyper's, either way round, as hyper-util's `TokioIo`
/// is between tokio's and hyper's.
///
/// Around a `futures-io` stream, an `async-net` socket or a `futures-rustls` session, it
/// implements hyper's [`Read`] and [`Write`], so hyper serves it. `futures-io` reads into
/// initialized memory and hyper hands out uninitialized memory: a read goes into a buffer of the
/// wrapper's own, at most 64 KiB, and is copied into hyper's. The buffer is zeroed only as it grows
/// and is released whenever the stream has nothing to read, and an idle connection holds none.
/// Around hyper's own I/O, an upgraded connection, it implements `futures-io`'s `AsyncRead` and
/// `AsyncWrite`, which need neither.
#[derive(Debug)]
pub struct FuturesIo<T> {
    inner: T,
    scratch: Vec<u8>,
}

impl<T> FuturesIo<T> {
    pub fn new(inner: T) -> FuturesIo<T> {
        FuturesIo { inner, scratch: Vec::new() }
    }

    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: futures_io::AsyncRead + Unpin> Read for FuturesIo<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let want = buf.remaining().min(SCRATCH);
        if this.scratch.len() < want {
            this.scratch.resize(want, 0);
        }
        match Pin::new(&mut this.inner).poll_read(cx, &mut this.scratch[..want]) {
            Poll::Ready(Ok(read)) => {
                buf.put_slice(&this.scratch[..read]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => {
                this.scratch = Vec::new();
                Poll::Pending
            }
        }
    }
}

impl<T: futures_io::AsyncWrite + Unpin> Write for FuturesIo<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl<T: Read + Unpin> futures_io::AsyncRead for FuturesIo<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut read = ReadBuf::new(buf);
        ready!(Pin::new(&mut self.get_mut().inner).poll_read(cx, read.unfilled()))?;
        Poll::Ready(Ok(read.filled().len()))
    }
}

impl<T: Write + Unpin> futures_io::AsyncWrite for FuturesIo<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;
    use std::task::Waker;

    use super::*;

    /// A `futures-io` stream over `data`, filling all it is offered of what is left, pending on
    /// every poll while `pending` is set.
    struct Source {
        data: Vec<u8>,
        at: usize,
        pending: bool,
    }

    impl futures_io::AsyncRead for Source {
        fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
            if self.pending {
                return Poll::Pending;
            }
            let n = buf.len().min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Poll::Ready(Ok(n))
        }
    }

    /// One read through `io` into a fresh hyper buffer of `capacity`: the bytes it filled.
    fn read(io: &mut FuturesIo<Source>, capacity: usize) -> Poll<Vec<u8>> {
        let mut memory = vec![MaybeUninit::<u8>::uninit(); capacity];
        let mut buf = ReadBuf::uninit(&mut memory);
        let mut cx = Context::from_waker(Waker::noop());
        Read::poll_read(Pin::new(io), &mut cx, buf.unfilled()).map(|read| {
            read.expect("the source does not fail");
            buf.filled().to_vec()
        })
    }

    #[test]
    fn every_byte_reaches_hyper_s_buffer_in_order_at_most_64_kib_a_read() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut io = FuturesIo::new(Source { data: data.clone(), at: 0, pending: false });
        let mut arrived = Vec::new();
        for capacity in [100, 8 * 1024, 256 * 1024].into_iter().cycle() {
            let Poll::Ready(filled) = read(&mut io, capacity) else { panic!("the source is never pending here") };
            assert!(filled.len() <= capacity.min(SCRATCH), "a read filled {} bytes of a {capacity}-byte buffer", filled.len());
            if filled.is_empty() {
                break;
            }
            arrived.extend_from_slice(&filled);
        }
        assert!(arrived == data, "the bytes hyper read differ from the stream's: {} of {}", arrived.len(), data.len());
    }

    #[test]
    fn a_pending_read_releases_the_buffer() {
        let mut io = FuturesIo::new(Source { data: vec![7; 16], at: 0, pending: false });
        assert_eq!(read(&mut io, 8), Poll::Ready(vec![7; 8]));
        assert!(io.scratch.capacity() > 0, "a ready read kept no buffer for the next");
        io.inner.pending = true;
        assert_eq!(read(&mut io, 8), Poll::Pending);
        assert_eq!(io.scratch.capacity(), 0, "an idle connection held its read buffer");
        io.inner.pending = false;
        assert_eq!(read(&mut io, 8), Poll::Ready(vec![7; 8]), "the read after a pending one");
    }
}
