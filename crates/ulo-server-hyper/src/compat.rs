//! [`FuturesIo`]: a stream between `futures-io`'s I/O traits and hyper's.

// Listed in `unsafe-allowed.txt` as pure Rust, so the `miri` CI job runs this module's tests under
// Miri: handing hyper's uninitialized read buffer to a `futures-io` reader, which takes
// initialized memory, goes through `ReadBufCursor::as_mut` and `advance`, both `unsafe`.
#![allow(unsafe_code)]

use std::io;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use hyper::rt::{Read, ReadBuf, ReadBufCursor, Write};

/// A stream between `futures-io`'s traits and hyper's, either way round, as hyper-util's `TokioIo`
/// is between tokio's and hyper's.
///
/// Around a `futures-io` stream, an `async-net` socket or a `futures-rustls` session, it
/// implements hyper's [`Read`] and [`Write`], so hyper serves it. `futures-io` reads into
/// initialized memory and hyper hands out uninitialized memory, so each read zeroes the part of
/// hyper's buffer it offers first. Around hyper's own I/O, an upgraded connection, it implements
/// `futures-io`'s `AsyncRead` and `AsyncWrite`, which need no zeroing.
#[derive(Debug)]
pub struct FuturesIo<T> {
    inner: T,
}

impl<T> FuturesIo<T> {
    pub fn new(inner: T) -> FuturesIo<T> {
        FuturesIo { inner }
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
        // SAFETY: `as_mut` requires that no byte that may already be initialized is
        // uninitialized; the loop only writes, so every byte stays or becomes initialized. The
        // cast from `[MaybeUninit<u8>]` to `[u8]` keeps the length and the layout, which the two
        // element types share, and is valid because the loop has just initialized every element.
        // The reader receives a `&mut [u8]`, through which safe code cannot uninitialize a byte.
        let unfilled: &mut [u8] = unsafe {
            let unfilled: &mut [MaybeUninit<u8>] = buf.as_mut();
            for byte in unfilled.iter_mut() {
                byte.write(0);
            }
            &mut *(std::ptr::from_mut::<[MaybeUninit<u8>]>(unfilled) as *mut [u8])
        };
        let offered = unfilled.len();
        let read = ready!(Pin::new(&mut self.get_mut().inner).poll_read(cx, unfilled))?;
        // `AsyncRead` is a safe trait, so a count past the slice is a bug in the reader that the
        // `advance` below must not trust.
        assert!(read <= offered, "a futures-io reader reported {read} bytes read into a {offered}-byte buffer");
        // SAFETY: `advance(n)` requires the first `n` unfilled bytes to be initialized; the block
        // above initialized all `offered` of them, and `read <= offered` was just checked.
        unsafe { buf.advance(read) };
        Poll::Ready(Ok(()))
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

/// No sockets, so Miri runs every test here: hyper's buffers are fresh uninitialized memory, and
/// each test reads every byte hyper is told was filled as a value, which Miri reports when the
/// byte was never written.
#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::task::Waker;

    use super::*;

    /// A `futures-io` stream over `data` that fills at most `chunk` bytes a read, pending on every
    /// other poll when `pending` is set. With `peek` set it reads every byte of the slice it is
    /// offered before writing, as `&mut [u8]` lets any reader do.
    struct Source {
        data: Vec<u8>,
        at: usize,
        chunk: usize,
        pending: bool,
        peek: bool,
        polls: usize,
    }

    impl Source {
        fn new(data: Vec<u8>, chunk: usize) -> Source {
            Source { data, at: 0, chunk, pending: false, peek: false, polls: 0 }
        }
    }

    impl futures_io::AsyncRead for Source {
        fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
            self.polls += 1;
            if self.pending && self.polls % 2 == 1 {
                return Poll::Pending;
            }
            if self.peek {
                black_box(buf.iter().map(|&byte| u64::from(byte)).sum::<u64>());
            }
            let n = buf.len().min(self.chunk).min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Poll::Ready(Ok(n))
        }
    }

    /// One poll through `io` into hyper's buffer `memory`, whose first `filled` bytes hold earlier
    /// reads: everything hyper's buffer then holds as filled, each byte read as a value, or `None`
    /// while the source is pending.
    fn read_into<T: futures_io::AsyncRead + Unpin>(io: &mut FuturesIo<T>, memory: &mut [MaybeUninit<u8>], filled: usize) -> Option<Vec<u8>> {
        let mut buf = ReadBuf::uninit(memory);
        // SAFETY: the first `filled` bytes were written by earlier reads through `io`.
        unsafe { buf.unfilled().advance(filled) };
        let mut cx = Context::from_waker(Waker::noop());
        match Read::poll_read(Pin::new(io), &mut cx, buf.unfilled()) {
            Poll::Ready(read) => read.expect("the source does not fail"),
            Poll::Pending => return None,
        }
        Some(buf.filled().iter().map(|&byte| black_box(byte)).collect())
    }

    /// Every byte the stream yields reaches hyper's buffer in order, through partial reads, pending
    /// polls, and buffers both smaller and larger than a read; and hyper is never told of a byte
    /// the reader did not write.
    #[test]
    fn every_byte_reaches_hyper_s_buffer_in_order() {
        let data: Vec<u8> = (0..5_000u32).map(|i| (i % 251) as u8).collect();
        let mut io = FuturesIo::new(Source { pending: true, ..Source::new(data.clone(), 700) });
        let mut arrived = Vec::new();
        for capacity in [100, 1_024, 4_096].into_iter().cycle() {
            let mut memory = vec![MaybeUninit::<u8>::uninit(); capacity];
            let Some(filled) = read_into(&mut io, &mut memory, 0) else { continue };
            assert!(filled.len() <= capacity.min(700), "a read filled {} bytes of a {capacity}-byte buffer", filled.len());
            if filled.is_empty() {
                break;
            }
            arrived.extend_from_slice(&filled);
        }
        assert_eq!(arrived.len(), data.len(), "the bytes hyper read");
        assert!(arrived == data, "the bytes hyper read differ from the stream's");
    }

    /// A read into a buffer hyper has partly filled lands after what is there and leaves it as it
    /// was; a pending poll fills nothing.
    #[test]
    fn a_read_lands_after_what_hyper_s_buffer_holds() {
        let mut io = FuturesIo::new(Source { pending: true, ..Source::new((1..=64).collect(), 10) });
        let mut memory = vec![MaybeUninit::<u8>::uninit(); 48];
        let mut held = Vec::new();
        while held.len() < 48 {
            if let Some(filled) = read_into(&mut io, &mut memory, held.len()) {
                assert!(filled.len() > held.len(), "a ready read into a buffer with room filled nothing");
                assert_eq!(filled[..held.len()], held[..], "a read changed what hyper's buffer already held");
                held = filled;
            }
        }
        assert_eq!(held, (1..=48).collect::<Vec<u8>>(), "hyper's buffer after reads into its unfilled tail");
    }

    /// A reader may read the slice it is offered, so the slice must be initialized before it gets
    /// it, whatever hyper's buffer held.
    #[test]
    fn the_reader_may_read_what_it_is_offered() {
        let mut io = FuturesIo::new(Source { pending: true, peek: true, ..Source::new(vec![7; 3_000], 512) });
        let mut total = 0;
        loop {
            let mut memory = vec![MaybeUninit::<u8>::uninit(); 2_048];
            let Some(filled) = read_into(&mut io, &mut memory, 0) else { continue };
            if filled.is_empty() {
                break;
            }
            assert!(filled.iter().all(|&byte| byte == 7), "a read filled hyper's buffer with something the stream did not yield");
            total += filled.len();
        }
        assert_eq!(total, 3_000, "the bytes hyper read");
    }

    /// A reader reporting more bytes than it was offered.
    struct Overcounts;

    impl futures_io::AsyncRead for Overcounts {
        fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len() + 1))
        }
    }

    /// The count a reader reports is checked before hyper is told of it, so a reader claiming
    /// more than it was offered panics rather than marking bytes past the slice filled.
    #[test]
    #[should_panic(expected = "bytes read into a 16-byte buffer")]
    fn a_reader_counting_past_its_slice_panics() {
        let mut memory = vec![MaybeUninit::<u8>::uninit(); 16];
        let _ = read_into(&mut FuturesIo::new(Overcounts), &mut memory, 0);
    }
}
