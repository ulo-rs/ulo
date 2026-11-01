//! A stand-in Redis server on a local port answering the few commands the adapter sends, counting
//! the connections it has accepted and holds open.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// A stand-in Redis server: what it has accepted and how many of those are still open.
#[derive(Default)]
pub struct Standin {
    accepted: AtomicUsize,
    open: AtomicUsize,
}

impl Standin {
    /// Listens on a local port, answering each connection on a task of its own.
    pub async fn start() -> (Arc<Standin>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("the stand-in binds");
        let url = format!("redis://{}", listener.local_addr().expect("the stand-in's address"));
        let standin = Arc::new(Standin::default());
        let serving = Arc::clone(&standin);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serving.accepted.fetch_add(1, Ordering::SeqCst);
                serving.open.fetch_add(1, Ordering::SeqCst);
                let serving = Arc::clone(&serving);
                tokio::spawn(async move {
                    let _ = answer(stream).await;
                    serving.open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        (standin, url)
    }

    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    pub fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// Answers RESP commands until the client hangs up: `SUBSCRIBE` and `PSUBSCRIBE` with their
/// confirmations, `PUBLISH` with no receiver, anything else `+OK`.
async fn answer(stream: TcpStream) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        let Some(command) = command(&mut read).await? else { return Ok(()) };
        let reply = match command.first().map(|name| name.to_ascii_uppercase()) {
            Some(name) if name == b"SUBSCRIBE" || name == b"PSUBSCRIBE" => {
                let kind = String::from_utf8_lossy(&name).to_lowercase();
                let channel = String::from_utf8_lossy(command.get(1).map_or(&[][..], Vec::as_slice)).into_owned();
                format!("*3\r\n${}\r\n{kind}\r\n${}\r\n{channel}\r\n:1\r\n", kind.len(), channel.len())
            }
            Some(name) if name == b"PUBLISH" => ":0\r\n".to_owned(),
            _ => "+OK\r\n".to_owned(),
        };
        write.write_all(reply.as_bytes()).await?;
    }
}

/// One command as an array of bulk strings; `None` once the client has hung up.
async fn command(read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> std::io::Result<Option<Vec<Vec<u8>>>> {
    let mut line = String::new();
    if read.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    let count: usize = line.trim_end().trim_start_matches('*').parse().map_err(std::io::Error::other)?;
    let mut parts = Vec::with_capacity(count);
    for _ in 0..count {
        line.clear();
        read.read_line(&mut line).await?;
        let len: usize = line.trim_end().trim_start_matches('$').parse().map_err(std::io::Error::other)?;
        let mut part = vec![0; len + 2];
        read.read_exact(&mut part).await?;
        part.truncate(len);
        parts.push(part);
    }
    Ok(Some(parts))
}
