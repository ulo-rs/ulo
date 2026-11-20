//! The suite's client: it writes each upgrade request itself over the host's stream, so a scenario
//! can send one RFC 6455 refuses and read the status, headers and body exactly as the server wrote
//! them, then speaks WebSocket through `async-tungstenite` on the same stream after a 101, or
//! writes and reads frames byte for byte through [`Raw`], for a frame the library would refuse to
//! send or would answer itself.

use std::sync::Arc;

use async_tungstenite::WebSocketStream;
use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::handshake::client::generate_key;
use async_tungstenite::tungstenite::handshake::derive_accept_key;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{AsyncReadExt, AsyncWriteExt, StreamExt};
use serde_json::Value;
use ulo::Timer;

use crate::app::within;

/// One upgrade request, valid until a scenario spoils it.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    method: String,
    path: String,
    version: &'static str,
    headers: Vec<(String, String)>,
    key: String,
}

impl Request {
    /// A valid upgrade request for `path` with a fresh key.
    pub(crate) fn upgrade(path: &str) -> Request {
        Request { method: "GET".to_owned(), path: path.to_owned(), version: "HTTP/1.1", headers: Vec::new(), key: String::new() }
            .set("Connection", "Upgrade")
            .set("Upgrade", "websocket")
            .set("Sec-WebSocket-Version", "13")
            .key(&generate_key())
    }

    pub(crate) fn key(mut self, key: &str) -> Request {
        self.key = key.to_owned();
        self.set("Sec-WebSocket-Key", key)
    }

    pub(crate) fn method(mut self, method: &str) -> Request {
        self.method = method.to_owned();
        self
    }

    pub(crate) fn version(mut self, version: &'static str) -> Request {
        self.version = version;
        self
    }

    /// One more header line, beside any of the same name.
    pub(crate) fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// `name` set to `value`, replacing every line of that name.
    pub(crate) fn set(self, name: &str, value: &str) -> Request {
        self.without(name).header(name, value)
    }

    pub(crate) fn without(mut self, name: &str) -> Request {
        self.headers.retain(|(line, _)| !line.eq_ignore_ascii_case(name));
        self
    }

    /// The request head as written on the wire, `host` its `Host` header.
    pub(crate) fn head(&self, host: &str) -> String {
        let mut head = format!("{} {} {}\r\nHost: {host}\r\n", self.method, self.path, self.version);
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        head
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn sent_key(&self) -> &str {
        &self.key
    }
}

/// The server's answer to one upgrade request: its status, its headers with lowercase names in
/// the order written, its body for a refusal, and the socket after a 101.
pub(crate) struct Answer<S> {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
    pub(crate) socket: Option<WebSocketStream<S>>,
}

impl<S> Answer<S> {
    /// The first value of header `name`, given in lowercase.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }
}

/// Writes `text` on `stream`.
pub(crate) async fn write<S: AsyncWrite + Unpin>(stream: &mut S, text: &str) {
    stream.write_all(text.as_bytes()).await.unwrap_or_else(|error| panic!("writing the upgrade request failed: {error}"));
    stream.flush().await.unwrap_or_else(|error| panic!("flushing the upgrade request failed: {error}"));
}

/// Reads the server's answer to an upgrade request whose key was `key`. A 101 must carry the
/// `Sec-WebSocket-Accept` RFC 6455 §4.2.2 derives from that key, and the stream continues as the
/// client's socket. A refusal's body is read as [`read_body`] reads it.
pub(crate) async fn read_answer<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, key: &str) -> Answer<S> {
    let (status, headers) = read_head(&mut stream).await;
    let mut answer = Answer { status, headers, body: String::new(), socket: None };
    if status == 101 {
        let expected = derive_accept_key(key.as_bytes());
        assert_eq!(answer.header("sec-websocket-accept"), Some(expected.as_str()), "the 101 carries the wrong `Sec-WebSocket-Accept`");
        answer.socket = Some(WebSocketStream::from_raw_socket(stream, Role::Client, None).await);
    } else {
        answer.body = read_body(&mut stream, &answer.headers).await;
    }
    answer
}

/// Reads the server's 101 to an upgrade request whose key was `key`, checked as [`read_answer`]
/// checks it, and hands back the stream, positioned at the first frame.
pub(crate) async fn read_switch<S: AsyncRead + Unpin>(mut stream: S, key: &str) -> S {
    let (status, headers) = read_head(&mut stream).await;
    assert_eq!(status, 101, "expected the upgrade to switch protocols: {headers:?}");
    let accept = headers.iter().find(|(name, _)| name == "sec-websocket-accept").map(|(_, value)| value.as_str());
    assert_eq!(accept, Some(derive_accept_key(key.as_bytes()).as_str()), "the 101 carries the wrong `Sec-WebSocket-Accept`");
    stream
}

/// Reads one response that switches no protocol, its body as [`read_body`] reads it, and leaves
/// `stream` where the next response would begin.
pub(crate) async fn read_response<S: AsyncRead + Unpin>(stream: &mut S) -> Answer<S> {
    let (status, headers) = read_head(stream).await;
    let body = read_body(stream, &headers).await;
    Answer { status, headers, body, socket: None }
}

/// A response's status and its headers with lowercase names, in the order written.
async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> (u16, Vec<(String, String)>) {
    // One byte at a time, so nothing after the head, a Close frame sent right after the 101 for
    // one, is taken from the stream the socket continues on.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8];
        stream.read_exact(&mut byte).await.unwrap_or_else(|error| {
            panic!("reading the response failed after {:?}: {error}", String::from_utf8_lossy(&head))
        });
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap_or_else(|error| panic!("the response head is not UTF-8: {error}"));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("the response's status line is malformed: {status_line:?}"));
    let headers = lines
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    (status, headers)
}

/// A response's body, by its `Content-Length` or its chunks, and with neither to the connection's
/// close, which RFC 9112 §6.3 makes the end of such a body: poem answers an HTTP/1.0 request that
/// way.
async fn read_body<S: AsyncRead + Unpin>(stream: &mut S, headers: &[(String, String)]) -> String {
    let header = |name: &str| headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    if let Some(length) = header("content-length").and_then(|length| length.parse::<usize>().ok()) {
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap_or_else(|error| panic!("reading the response's body failed: {error}"));
        String::from_utf8_lossy(&body).into_owned()
    } else if header("transfer-encoding").is_some_and(|coding| coding.eq_ignore_ascii_case("chunked")) {
        chunked(stream).await
    } else {
        let mut body = Vec::new();
        stream.read_to_end(&mut body).await.unwrap_or_else(|error| panic!("reading the response's body to the close failed: {error}"));
        String::from_utf8_lossy(&body).into_owned()
    }
}

/// A chunked body, read to its last chunk.
async fn chunked<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut body = Vec::new();
    loop {
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            let mut byte = [0u8];
            stream.read_exact(&mut byte).await.unwrap_or_else(|error| panic!("reading a chunk's size failed: {error}"));
            line.push(byte[0]);
        }
        let size = String::from_utf8_lossy(&line[..line.len() - 2]).split(';').next().unwrap_or_default().trim().to_owned();
        let size = usize::from_str_radix(&size, 16).unwrap_or_else(|_| panic!("a chunk's size is malformed: {size:?}"));
        let mut chunk = vec![0; size + 2];
        stream.read_exact(&mut chunk).await.unwrap_or_else(|error| panic!("reading a chunk failed: {error}"));
        if size == 0 {
            return String::from_utf8_lossy(&body).into_owned();
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

/// An open WebSocket connection, every read bounded by the app's clock.
pub(crate) struct Conn<S> {
    pub(crate) ws: WebSocketStream<S>,
    timer: Arc<dyn Timer>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    pub(crate) fn new(ws: WebSocketStream<S>, timer: Arc<dyn Timer>) -> Self {
        Conn { ws, timer }
    }

    pub(crate) async fn send(&mut self, message: Message) {
        let shown = format!("{message:?}");
        self.ws.send(message).await.unwrap_or_else(|error| panic!("sending {shown} failed: {error}"));
    }

    /// `message` as one text frame.
    pub(crate) async fn send_json(&mut self, message: &Value) {
        self.send(Message::text(message.to_string())).await;
    }

    /// The next thing the socket yields, Pings and Pongs included; `None` once it has ended.
    pub(crate) async fn next_raw(&mut self, what: &str) -> Option<Result<Message, String>> {
        within(&*self.timer, what, self.ws.next()).await.map(|next| next.map_err(|error| error.to_string()))
    }

    /// The next message that is not a Ping or a Pong.
    pub(crate) async fn next_message(&mut self) -> Message {
        let timer = Arc::clone(&self.timer);
        within(&*timer, "the next message", async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(message)) => return message,
                    other => panic!("expected a message, got {other:?}"),
                }
            }
        })
        .await
    }

    /// The next data message as JSON text.
    pub(crate) async fn next_json(&mut self) -> Value {
        match self.next_message().await {
            Message::Text(text) => {
                serde_json::from_str(&text).unwrap_or_else(|error| panic!("the server sent text that is not JSON, {text:?}: {error}"))
            }
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    /// Sends `message` and reads the next data message.
    pub(crate) async fn exchange(&mut self, message: Value) -> Value {
        self.send_json(&message).await;
        self.next_json().await
    }

    /// The close code and reason the server ends the connection with. A data message before the
    /// Close frame fails the scenario.
    pub(crate) async fn close_frame(&mut self) -> Option<(u16, String)> {
        match self.next_message().await {
            Message::Close(frame) => frame.map(|frame| (u16::from(frame.code), frame.reason.as_str().to_owned())),
            other => panic!("expected a Close frame, got {other:?}"),
        }
    }

    /// The close code and reason, past any data messages written before the Close frame; how many
    /// there were.
    pub(crate) async fn close_frame_after_data(&mut self) -> (Option<(u16, String)>, usize) {
        let mut data = 0;
        loop {
            match self.next_message().await {
                Message::Close(frame) => return (frame.map(|frame| (u16::from(frame.code), frame.reason.as_str().to_owned())), data),
                Message::Text(_) | Message::Binary(_) => data += 1,
                other => panic!("expected data or a Close frame, got {other:?}"),
            }
        }
    }

    /// Closes the client's side with `code` and `reason`, or answers a Close the server sent, and
    /// reads until the server ends the connection.
    pub(crate) async fn hang_up_with(mut self, code: u16, reason: &str) {
        let frame = CloseFrame { code: CloseCode::from(code), reason: reason.to_owned().into() };
        let _ = self.ws.close(Some(frame)).await;
        let timer = Arc::clone(&self.timer);
        within(&*timer, "the server's end of the connection", async { while let Some(Ok(_)) = self.ws.next().await {} }).await;
    }

    pub(crate) async fn hang_up(self) {
        self.hang_up_with(1000, "").await;
    }
}

/// One frame as the server wrote it (RFC 6455 §5.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawFrame {
    pub(crate) fin: bool,
    pub(crate) opcode: u8,
    pub(crate) payload: Vec<u8>,
}

impl RawFrame {
    pub(crate) const TEXT: u8 = 0x1;
    pub(crate) const CONTINUATION: u8 = 0x0;
    pub(crate) const CLOSE: u8 = 0x8;
    pub(crate) const PING: u8 = 0x9;
    pub(crate) const PONG: u8 = 0xA;

    /// A Close frame's status code, `None` for one without a body (§5.5.1).
    pub(crate) fn close_code(&self) -> Option<u16> {
        (self.opcode == Self::CLOSE && self.payload.len() >= 2).then(|| u16::from_be_bytes([self.payload[0], self.payload[1]]))
    }

    /// `opcode` and what the frame carries, as a failure message shows it.
    fn shown(&self) -> String {
        format!("opcode {:#x} fin {} {:?}", self.opcode, self.fin, String::from_utf8_lossy(&self.payload))
    }
}

/// `frames` as a failure message shows them.
pub(crate) fn shown(frames: &[RawFrame]) -> Vec<String> {
    frames.iter().map(RawFrame::shown).collect()
}

/// A client frame, masked with a fixed key unless `masked` is false, which §5.1 forbids a client.
pub(crate) fn client_frame(fin: bool, opcode: u8, payload: &[u8], masked: bool) -> Vec<u8> {
    const KEY: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];
    let mut bytes = vec![if fin { 0x80 } else { 0x00 } | opcode];
    let mask_bit = if masked { 0x80 } else { 0x00 };
    match payload.len() {
        len @ 0..=125 => bytes.push(mask_bit | len as u8),
        len @ 126..=0xffff => {
            bytes.push(mask_bit | 126);
            bytes.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            bytes.push(mask_bit | 127);
            bytes.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    if masked {
        bytes.extend_from_slice(&KEY);
        bytes.extend(payload.iter().enumerate().map(|(i, byte)| byte ^ KEY[i % 4]));
    } else {
        bytes.extend_from_slice(payload);
    }
    bytes
}

/// A Close frame's body: the code, then the reason (§5.5.1).
pub(crate) fn close_body(code: u16, reason: &str) -> Vec<u8> {
    let mut body = code.to_be_bytes().to_vec();
    body.extend_from_slice(reason.as_bytes());
    body
}

/// An upgraded connection spoken byte for byte, every read bounded by the app's clock.
pub(crate) struct Raw<S> {
    stream: S,
    timer: Arc<dyn Timer>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Raw<S> {
    pub(crate) fn new(stream: S, timer: Arc<dyn Timer>) -> Self {
        Raw { stream, timer }
    }

    /// Writes `bytes`, one or more frames, at once.
    pub(crate) async fn write(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap_or_else(|error| panic!("writing a frame failed: {error}"));
        self.stream.flush().await.unwrap_or_else(|error| panic!("flushing a frame failed: {error}"));
    }

    /// Writes `bytes` where the server may already have ended the connection, as after a fault.
    pub(crate) async fn write_unless_ended(&mut self, bytes: &[u8]) {
        if self.stream.write_all(bytes).await.is_ok() {
            let _ = self.stream.flush().await;
        }
    }

    /// The next frame the server wrote, `None` once the connection has ended.
    pub(crate) async fn next_frame(&mut self, what: &str) -> Option<RawFrame> {
        let timer = Arc::clone(&self.timer);
        within(&*timer, what, read_frame(&mut self.stream)).await
    }

    /// Every frame the server writes until it ends the connection.
    pub(crate) async fn frames_to_end(&mut self, what: &str) -> Vec<RawFrame> {
        let timer = Arc::clone(&self.timer);
        within(&*timer, what, async {
            let mut frames = Vec::new();
            while let Some(frame) = read_frame(&mut self.stream).await {
                frames.push(frame);
            }
            frames
        })
        .await
    }
}

/// One frame off `stream`, `None` at the end of the connection, a reset included. A server frame
/// carries no mask (§5.1); one that does is unmasked here, so the scenario sees its payload.
async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Option<RawFrame> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).await.ok()?;
    let len = match head[1] & 0x7f {
        126 => {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext).await.ok()?;
            u64::from(u16::from_be_bytes(ext))
        }
        127 => {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext).await.ok()?;
            u64::from_be_bytes(ext)
        }
        len => u64::from(len),
    };
    let mask = if head[1] & 0x80 != 0 {
        let mut key = [0u8; 4];
        stream.read_exact(&mut key).await.ok()?;
        Some(key)
    } else {
        None
    };
    let mut payload = vec![0u8; usize::try_from(len).ok()?];
    stream.read_exact(&mut payload).await.ok()?;
    if let Some(key) = mask {
        payload.iter_mut().enumerate().for_each(|(i, byte)| *byte ^= key[i % 4]);
    }
    Some(RawFrame { fin: head[0] & 0x80 != 0, opcode: head[0] & 0x0f, payload })
}
