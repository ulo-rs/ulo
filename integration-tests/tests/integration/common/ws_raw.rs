//! A WebSocket client that speaks frames by hand.
//!
//! The upgrade is written as text, the response head is read a byte at a time so no frame byte is
//! swallowed, and each frame comes back as `fin`, `rsv`, opcode and payload. A client library sits
//! between an assertion and the socket and answers what it decided the bytes meant; a suite that
//! asks what went on the wire reads the bytes.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The example key from RFC 6455 §1.3. A server checks its shape, not its entropy.
const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// One frame as read off the socket.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub fin: bool,
    pub rsv: u8,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    /// The status code a Close frame carries; `None` for another opcode or a Close with no body.
    pub fn close_code(&self) -> Option<u16> {
        if self.opcode == 0x8 && self.payload.len() >= 2 {
            Some(u16::from_be_bytes([self.payload[0], self.payload[1]]))
        } else {
            None
        }
    }

    /// The reason after a Close frame's status code, empty when there is none.
    pub fn close_reason(&self) -> String {
        if self.opcode == 0x8 && self.payload.len() > 2 {
            String::from_utf8_lossy(&self.payload[2..]).into_owned()
        } else {
            String::new()
        }
    }

    /// The payload read as UTF-8, lossily.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.payload).into_owned()
    }
}

/// Why a read produced no frame. A peer that hung up and a peer that said nothing look the same
/// to a reader that only reports absence, and the two mean different things.
#[derive(Debug)]
pub enum ReadEnd {
    /// The peer closed the TCP connection.
    Eof,
    /// Nothing arrived within the bound.
    Timeout,
    Io(std::io::Error),
}

pub struct Raw {
    stream: TcpStream,
    response_head: String,
}

impl Raw {
    /// Open a TCP connection and write the upgrade request, with `extra` as further header lines.
    pub async fn connect(addr: SocketAddr, path: &str, extra: &[(&str, &str)]) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut req = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 13\r\n"
        );
        for (k, v) in extra {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut head = Vec::new();
        loop {
            let mut b = [0u8; 1];
            match tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut b)).await {
                Ok(Ok(_)) => {}
                _ => break,
            }
            head.push(b[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        Self {
            stream,
            response_head: String::from_utf8_lossy(&head).into_owned(),
        }
    }

    /// The response's status line.
    pub fn status(&self) -> &str {
        self.response_head.lines().next().unwrap_or("")
    }

    /// Write one frame. A client frame is masked; `masked: false` writes the violation. A failed
    /// write is handed back: after a Close the peer may already have hung up.
    pub async fn send(
        &mut self,
        fin: bool,
        opcode: u8,
        payload: &[u8],
        masked: bool,
    ) -> std::io::Result<()> {
        let mut out = vec![if fin { 0x80 | opcode } else { opcode }];
        let mask_bit = if masked { 0x80u8 } else { 0x00 };
        let len = payload.len();
        if len < 126 {
            out.push(mask_bit | len as u8);
        } else if len <= u16::MAX as usize {
            out.push(mask_bit | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(mask_bit | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        if masked {
            let mask = [0xA1u8, 0xB2, 0xC3, 0xD4];
            out.extend_from_slice(&mask);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        } else {
            out.extend_from_slice(payload);
        }
        self.stream.write_all(&out).await?;
        self.stream.flush().await
    }

    /// One masked text frame carrying `s`.
    pub async fn send_text(&mut self, s: &str) -> std::io::Result<()> {
        self.send(true, 0x1, s.as_bytes(), true).await
    }

    /// The next frame, or what ended the read instead.
    pub async fn read_frame(&mut self, within: Duration) -> Result<Frame, ReadEnd> {
        let io = |e: std::io::Error| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                ReadEnd::Eof
            } else {
                ReadEnd::Io(e)
            }
        };
        let fut = async {
            let mut h = [0u8; 2];
            self.stream.read_exact(&mut h).await.map_err(io)?;
            let fin = h[0] & 0x80 != 0;
            let rsv = (h[0] & 0x70) >> 4;
            let opcode = h[0] & 0x0F;
            let masked = h[1] & 0x80 != 0;
            let mut len = (h[1] & 0x7F) as u64;
            if len == 126 {
                let mut e = [0u8; 2];
                self.stream.read_exact(&mut e).await.map_err(io)?;
                len = u16::from_be_bytes(e) as u64;
            } else if len == 127 {
                let mut e = [0u8; 8];
                self.stream.read_exact(&mut e).await.map_err(io)?;
                len = u64::from_be_bytes(e);
            }
            let mask = if masked {
                let mut m = [0u8; 4];
                self.stream.read_exact(&mut m).await.map_err(io)?;
                Some(m)
            } else {
                None
            };
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload).await.map_err(io)?;
            if let Some(m) = mask {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= m[i % 4];
                }
            }
            Ok(Frame {
                fin,
                rsv,
                opcode,
                payload,
            })
        };
        match tokio::time::timeout(within, fut).await {
            Ok(r) => r,
            Err(_) => Err(ReadEnd::Timeout),
        }
    }

    /// True when the peer closed the TCP connection within the bound and wrote nothing first.
    pub async fn eof(&mut self, within: Duration) -> bool {
        let mut b = [0u8; 1];
        matches!(
            tokio::time::timeout(within, self.stream.read(&mut b)).await,
            Ok(Ok(0))
        )
    }
}
