//! An upgraded connection carries bytes both ways through `futures-io`'s traits.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::io::{AsyncReadExt, AsyncWriteExt};
use ulo_http::Upgraded;

/// Fails the test when `steps` has not finished in five seconds, so bytes that never arrive fail
/// it rather than hang it.
async fn within(steps: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(5), steps).await.expect("the steps finish within five seconds");
}

/// I/O whose reads answer what was written before them, in order.
#[derive(Default)]
struct Loopback {
    queued: VecDeque<u8>,
}

impl futures::io::AsyncRead for Loopback {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let n = buf.len().min(self.queued.len());
        for (slot, byte) in buf.iter_mut().zip(self.queued.drain(..n)) {
            *slot = byte;
        }
        Poll::Ready(Ok(n))
    }
}

impl futures::io::AsyncWrite for Loopback {
    fn poll_write(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.queued.extend(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_futures_io_connection_reads_back_what_was_written() {
    within(async {
        let mut upgraded = Upgraded::from_futures(Loopback::default());
        upgraded.write_all(b"ping").await.expect("the write goes through");
        upgraded.flush().await.expect("the flush goes through");
        let mut read = [0u8; 4];
        upgraded.read_exact(&mut read).await.expect("the bytes written come back");
        assert_eq!(&read, b"ping");
        upgraded.close().await.expect("the close goes through");
    })
    .await;
}
