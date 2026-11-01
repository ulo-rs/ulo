//! A request builder and a reply read as it arrives, for the HTTP examples whose tests need more
//! than `get`: a method, headers and a body, and a streaming reply read one block at a time.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{Reply, send};

/// One request: `Call::post("/notes").header("content-type", "application/json").body("{}")`.
#[derive(Clone, Debug)]
pub struct Call {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Call {
    pub fn new(method: &str, path: &str) -> Call {
        Call { method: method.to_owned(), path: path.to_owned(), headers: Vec::new(), body: Vec::new() }
    }

    pub fn get(path: &str) -> Call {
        Call::new("GET", path)
    }

    pub fn post(path: &str) -> Call {
        Call::new("POST", path)
    }

    pub fn header(mut self, name: &str, value: &str) -> Call {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Call {
        self.body = body.into();
        self
    }

    /// A body with `content-type: application/json`.
    pub fn json(self, body: &str) -> Call {
        self.header("content-type", "application/json").body(body)
    }

    fn render(&self) -> Vec<u8> {
        let mut head = format!("{} {} HTTP/1.1\r\nhost: example\r\nconnection: close\r\n", self.method, self.path);
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if !self.body.is_empty() {
            head.push_str(&format!("content-length: {}\r\n", self.body.len()));
        }
        head.push_str("\r\n");
        let mut raw = head.into_bytes();
        raw.extend_from_slice(&self.body);
        raw
    }
}

/// Sends `call` and reads the reply to the end of the connection. A body that is not UTF-8 is read
/// lossily, so this suits the text the examples answer with.
pub async fn call(addr: SocketAddr, call: Call) -> Reply {
    send(addr, &String::from_utf8_lossy(&call.render())).await
}

/// A reply being read: the head arrived, the body is read a block at a time with [`next_block`].
///
/// [`next_block`]: Live::next_block
pub struct Live {
    stream: TcpStream,
    pub status: u16,
    /// Header names lowercased, in the order received.
    pub headers: Vec<(String, String)>,
    /// Decoded body bytes read and not yet returned.
    pending: String,
    /// Raw bytes read and not yet decoded as chunks.
    raw: Vec<u8>,
    chunked: bool,
    ended: bool,
}

/// How long any one read of a live reply may take in real time.
const PATIENCE: Duration = Duration::from_secs(10);

/// Sends `call` and returns once the reply's head has arrived.
pub async fn open(addr: SocketAddr, call: Call) -> Live {
    let mut stream = TcpStream::connect(addr).await.expect("connect to the example server");
    stream.write_all(&call.render()).await.expect("write the request");
    let mut raw = Vec::new();
    let split = loop {
        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        read_more(&mut stream, &mut raw).await.expect("the reply's head arrives");
    };
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let rest = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).expect("a status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked");
    Live { stream, status, headers, pending: String::new(), raw: rest, chunked, ended: false }
}

async fn read_more(stream: &mut TcpStream, into: &mut Vec<u8>) -> Option<()> {
    let mut buffer = [0u8; 4096];
    let n = tokio::time::timeout(PATIENCE, stream.read(&mut buffer)).await.expect("the server wrote within ten seconds of real time").expect("the read succeeds");
    into.extend_from_slice(&buffer[..n]);
    (n > 0).then_some(())
}

impl Live {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The next block of the body ending in a blank line (one Server-Sent Event, comments
    /// included), without the blank line; the rest of the body when the connection ends first, and
    /// `None` once nothing is left.
    pub async fn next_block(&mut self) -> Option<String> {
        loop {
            if let Some(at) = self.pending.find("\n\n") {
                let block = self.pending[..at].to_owned();
                self.pending.drain(..at + 2);
                return Some(block);
            }
            if self.ended {
                return (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending));
            }
            self.fill().await;
        }
    }

    /// The rest of the body, read to the end of the reply.
    pub async fn rest(&mut self) -> String {
        while !self.ended {
            self.fill().await;
        }
        std::mem::take(&mut self.pending)
    }

    async fn fill(&mut self) {
        if !self.chunked {
            let raw = std::mem::take(&mut self.raw);
            self.pending.push_str(&String::from_utf8_lossy(&raw));
            if read_more(&mut self.stream, &mut self.raw).await.is_none() {
                self.pending.push_str(&String::from_utf8_lossy(&std::mem::take(&mut self.raw)));
                self.ended = true;
            }
            return;
        }
        loop {
            let Some(line_end) = self.raw.windows(2).position(|w| w == b"\r\n") else {
                if read_more(&mut self.stream, &mut self.raw).await.is_none() {
                    self.ended = true;
                    return;
                }
                continue;
            };
            let size = usize::from_str_radix(String::from_utf8_lossy(&self.raw[..line_end]).trim(), 16).expect("a chunk size");
            if size == 0 {
                self.ended = true;
                return;
            }
            let need = line_end + 2 + size + 2;
            while self.raw.len() < need {
                if read_more(&mut self.stream, &mut self.raw).await.is_none() {
                    self.ended = true;
                    return;
                }
            }
            self.pending.push_str(&String::from_utf8_lossy(&self.raw[line_end + 2..line_end + 2 + size]));
            self.raw.drain(..need);
            return;
        }
    }
}
