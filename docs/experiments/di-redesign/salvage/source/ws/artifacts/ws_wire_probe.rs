//! AUDIT PROBE — not a suite. Raw-frame WebSocket conformance measurement.

#![allow(dead_code, unused_imports)]

use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use ulo::UloFactory;
use ulo::di::ModuleMetadata;
use ulo::dispatch::Items;
use ulo::ws::{WsClient, WsError, WsHandlerResult, WsMessage};
use ulo::{module, websocket_gateway};
use ulo_macros::{new, subscriptions};

// ── raw client ───────────────────────────────────────────────────────────────

fn b64(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub struct Raw {
    pub stream: TcpStream,
    pub response_head: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub fin: bool,
    pub rsv: u8,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn close_code(&self) -> Option<u16> {
        if self.opcode == 0x8 && self.payload.len() >= 2 {
            Some(u16::from_be_bytes([self.payload[0], self.payload[1]]))
        } else {
            None
        }
    }
    pub fn close_reason(&self) -> String {
        if self.opcode == 0x8 && self.payload.len() > 2 {
            String::from_utf8_lossy(&self.payload[2..]).to_string()
        } else {
            String::new()
        }
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.payload).to_string()
    }
}

impl Raw {
    pub async fn connect(addr: SocketAddr, path: &str, extra: &[(&str, &str)]) -> Self {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let key = b64(&[0x11u8; 16]);
        let mut req = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
        );
        for (k, v) in extra {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();

        // Read until the header terminator, byte at a time so no frame bytes are swallowed.
        let mut head = Vec::new();
        loop {
            let mut b = [0u8; 1];
            match tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut b)).await {
                Ok(Ok(_)) => {}
                _ => break,
            }
            head.push(b[0]);
            if head.len() >= 4 && &head[head.len() - 4..] == b"\r\n\r\n" {
                break;
            }
        }
        Self {
            stream,
            response_head: String::from_utf8_lossy(&head).to_string(),
        }
    }

    pub fn status(&self) -> String {
        self.response_head.lines().next().unwrap_or("").to_string()
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.response_head
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
    }

    /// Count of response header lines with this name — the MUST-NOT-appear-twice check.
    pub fn header_count(&self, name: &str) -> usize {
        self.response_head
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .count()
    }

    pub async fn send(&mut self, fin: bool, opcode: u8, payload: &[u8], masked: bool) {
        let mut out = Vec::new();
        out.push(if fin { 0x80 | opcode } else { opcode });
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
        let _ = self.stream.write_all(&out).await;
        let _ = self.stream.flush().await;
    }

    pub async fn send_text(&mut self, s: &str) {
        self.send(true, 0x1, s.as_bytes(), true).await;
    }

    /// Next frame, or `None` on timeout / EOF.
    pub async fn read_frame(&mut self, within: Duration) -> Option<Frame> {
        let fut = async {
            let mut h = [0u8; 2];
            self.stream.read_exact(&mut h).await.ok()?;
            let fin = h[0] & 0x80 != 0;
            let rsv = (h[0] & 0x70) >> 4;
            let opcode = h[0] & 0x0F;
            let masked = h[1] & 0x80 != 0;
            let mut len = (h[1] & 0x7F) as u64;
            if len == 126 {
                let mut e = [0u8; 2];
                self.stream.read_exact(&mut e).await.ok()?;
                len = u16::from_be_bytes(e) as u64;
            } else if len == 127 {
                let mut e = [0u8; 8];
                self.stream.read_exact(&mut e).await.ok()?;
                len = u64::from_be_bytes(e);
            }
            let mask = if masked {
                let mut m = [0u8; 4];
                self.stream.read_exact(&mut m).await.ok()?;
                Some(m)
            } else {
                None
            };
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload).await.ok()?;
            if let Some(m) = mask {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= m[i % 4];
                }
            }
            Some(Frame {
                fin,
                rsv,
                opcode,
                payload,
            })
        };
        tokio::time::timeout(within, fut).await.ok().flatten()
    }

    /// Read frames until one matches, discarding others. `None` on timeout.
    pub async fn read_until(
        &mut self,
        within: Duration,
        pred: impl Fn(&Frame) -> bool,
    ) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            let f = self.read_frame(left).await?;
            if pred(&f) {
                return Some(f);
            }
        }
    }

    /// True when the peer closed the TCP connection within the bound.
    pub async fn eof(&mut self, within: Duration) -> bool {
        let mut b = [0u8; 1];
        matches!(
            tokio::time::timeout(within, self.stream.read(&mut b)).await,
            Ok(Ok(0))
        )
    }
}

// ── gateway under test ───────────────────────────────────────────────────────

#[websocket_gateway("/ws")]
pub struct ProbeGateway {}

#[subscriptions]
impl ProbeGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self, msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text(format!("len={}", msg.as_text().unwrap_or("").len())).into())
    }
}

#[module(providers: [ProbeGateway])]
pub struct ProbeModule;

#[websocket_gateway("/ws", port = 19771)]
pub struct SepGateway {}

#[subscriptions]
impl SepGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self, msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text(format!("len={}", msg.as_text().unwrap_or("").len())).into())
    }
}

#[module(providers: [SepGateway])]
pub struct SepModule;

pub async fn start_http(adapter: impl ulo::http::HttpAdapter + 'static) -> SocketAddr {
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(ProbeModule).await.unwrap();
        app.use_http_adapter(adapter, ("127.0.0.1", 0)).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.http.unwrap());
        app.run().await;
    });
    rx.await.unwrap()
}

pub async fn start_tungstenite() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(SepModule).await.unwrap();
        app.use_websocket_adapter(ulo_ws_tungstenite::TungsteniteAdapter::new())
            .unwrap();
        app.use_websocket_listener(19771, listener).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.websocket.into_iter().next().unwrap());
        app.run().await;
    });
    rx.await.unwrap()
}

// ── P1: RFC 6455 §5.5.2 — Ping MUST be answered with a Pong ──────────────────

async fn case_ping_pong(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    if !c.status().contains("101") {
        return format!("HANDSHAKE FAILED: {}", c.status());
    }
    c.send(true, 0x9, b"probe-payload", true).await;
    match c
        .read_until(Duration::from_secs(3), |f| f.opcode == 0xA || f.opcode == 0x8)
        .await
    {
        Some(f) if f.opcode == 0xA => {
            if f.payload == b"probe-payload" {
                "PONG (payload identical)".to_string()
            } else {
                format!("PONG but payload differs: {:?}", f.text())
            }
        }
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => "NO PONG within 3s".to_string(),
    }
}

/// Falsification: the same probe against a server known NOT to pong.
async fn case_ping_pong_falsify() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                // Complete the handshake, then never speak again.
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
                    )
                    .await;
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
        }
    });
    case_ping_pong(addr).await
}

#[tokio::test]
async fn probe_ping_pong_all_adapters() {
    println!("\n=== P1  RFC 6455 §5.5.2 — Ping MUST be answered with a Pong ===");
    println!(
        "  falsify(silent server) : {}",
        case_ping_pong_falsify().await
    );
    println!(
        "  axum                   : {}",
        case_ping_pong(start_http(ulo_http_axum::AxumAdapter::new()).await).await
    );
    println!(
        "  salvo                  : {}",
        case_ping_pong(start_http(ulo_http_salvo::SalvoAdapter::new()).await).await
    );
    println!(
        "  poem                   : {}",
        case_ping_pong(start_http(ulo_http_poem::PoemAdapter::new()).await).await
    );
    println!(
        "  rocket                 : {}",
        case_ping_pong(start_http(ulo_http_rocket::RocketAdapter::new()).await).await
    );
    println!(
        "  tungstenite            : {}",
        case_ping_pong(start_tungstenite().await).await
    );
}

// ── P2: §5.5.1 — a Close received MUST be answered with a Close ──────────────

async fn case_close_echo(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    if !c.status().contains("101") {
        return format!("HANDSHAKE FAILED: {}", c.status());
    }
    let mut body = 1000u16.to_be_bytes().to_vec();
    body.extend_from_slice(b"bye");
    let t0 = std::time::Instant::now();
    c.send(true, 0x8, &body, true).await;
    // Drain every frame the peer writes until the socket ends or 3s pass, so a
    // close arriving after other traffic is not read as absent.
    let mut seen: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut closed = false;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match c.read_frame(left).await {
            Some(f) if f.opcode == 0x8 => {
                seen.push(format!(
                    "CLOSE code={:?} reason={:?} @{}ms",
                    f.close_code(),
                    f.close_reason(),
                    t0.elapsed().as_millis()
                ));
            }
            Some(f) => seen.push(format!("op={:#x} {:?}", f.opcode, f.text())),
            None => {
                closed = true;
                break;
            }
        }
    }
    format!(
        "frames={:?} socket_ended={} after={}ms",
        seen,
        closed,
        t0.elapsed().as_millis()
    )
}

/// §5.5.2 — "unless it already received a Close frame": a Ping after the peer's Close.
async fn case_ping_after_close(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    c.send(true, 0x8, &1000u16.to_be_bytes(), true).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    c.send(true, 0x9, b"late", true).await;
    match c.read_until(Duration::from_secs(2), |f| f.opcode == 0xA).await {
        Some(_) => "PONGED after close (spec says it must not)".to_string(),
        None => "no pong after close (correct)".to_string(),
    }
}

#[tokio::test]
async fn probe_close_handshake() {
    println!("\n=== P2  §5.5.1 Close echo / §5.5.2 no-pong-after-close ===");
    for (name, addr) in adapters().await {
        println!("  {name:<12} close-echo  : {}", case_close_echo(addr).await);
        println!(
            "  {name:<12} ping-after  : {}",
            case_ping_after_close(addr).await
        );
    }
}

// ── P3: §4.2.2 / §11.3.4 — subprotocol negotiation ───────────────────────────

async fn case_subproto(addr: SocketAddr, header: &[(&str, &str)]) -> String {
    let c = Raw::connect(addr, "/ws", header).await;
    if !c.status().contains("101") {
        return format!("handshake {}", c.status().trim().to_string());
    }
    format!(
        "101 protocol={:?} (appears {}x)",
        c.header("sec-websocket-protocol"),
        c.header_count("sec-websocket-protocol")
    )
}

#[tokio::test]
async fn probe_subprotocol() {
    println!("\n=== P3  §4.2.2 — /subprotocol/ MUST be one of the client's values, ABNF token ===");
    for (name, addr) in adapters().await {
        println!(
            "  {name:<12} one token       : {}",
            case_subproto(addr, &[("Sec-WebSocket-Protocol", "chat")]).await
        );
        println!(
            "  {name:<12} two, one header : {}",
            case_subproto(addr, &[("Sec-WebSocket-Protocol", "chat, superchat")]).await
        );
        println!(
            "  {name:<12} two headers     : {}",
            case_subproto(
                addr,
                &[
                    ("Sec-WebSocket-Protocol", "chat"),
                    ("Sec-WebSocket-Protocol", "superchat")
                ]
            )
            .await
        );
        println!(
            "  {name:<12} nonsense token  : {}",
            case_subproto(addr, &[("Sec-WebSocket-Protocol", "no-such-protocol-v99")]).await
        );
        println!(
            "  {name:<12} none offered    : {}",
            case_subproto(addr, &[]).await
        );
    }
}

// ── P4: §5.4 — fragmentation ─────────────────────────────────────────────────

async fn case_fragmented(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    if !c.status().contains("101") {
        return format!("handshake {}", c.status());
    }
    let whole = r#"{"event":"echo","data":"0123456789"}"#;
    let (a, b) = whole.split_at(12);
    c.send(false, 0x1, a.as_bytes(), true).await; // fin=0, text
    c.send(true, 0x0, b.as_bytes(), true).await; // fin=1, continuation
    match c
        .read_until(Duration::from_secs(3), |f| f.opcode == 0x1 || f.opcode == 0x8)
        .await
    {
        Some(f) if f.opcode == 0x1 => format!(
            "reassembled -> {:?} (whole is {} bytes)",
            f.text(),
            whole.len()
        ),
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => "no answer".to_string(),
    }
}

/// §5.4: a control frame MAY be injected in the middle of a fragmented message.
async fn case_interleaved_ping(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    let whole = r#"{"event":"echo","data":"0123456789"}"#;
    let (a, b) = whole.split_at(12);
    c.send(false, 0x1, a.as_bytes(), true).await;
    c.send(true, 0x9, b"mid", true).await;
    c.send(true, 0x0, b.as_bytes(), true).await;
    let pong = c.read_until(Duration::from_secs(2), |f| f.opcode == 0xA).await;
    let text = c
        .read_until(Duration::from_secs(2), |f| f.opcode == 0x1 || f.opcode == 0x8)
        .await;
    format!(
        "pong={} answer={:?}",
        pong.is_some(),
        text.map(|f| (f.opcode, f.text()))
    )
}

#[tokio::test]
async fn probe_fragmentation() {
    println!("\n=== P4  §5.4 — fragmentation and interleaved control frames ===");
    for (name, addr) in adapters().await {
        println!("  {name:<12} two fragments : {}", case_fragmented(addr).await);
        println!(
            "  {name:<12} ping mid-frag : {}",
            case_interleaved_ping(addr).await
        );
    }
}

// ── P5: §7.4 / §5.5 — protocol errors ────────────────────────────────────────

async fn case_reserved_opcode(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    c.send(true, 0x3, b"x", true).await;
    match c.read_until(Duration::from_secs(3), |f| f.opcode == 0x8).await {
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => format!("no close; tcp_eof={}", c.eof(Duration::from_secs(2)).await),
    }
}

async fn case_bad_utf8(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    c.send(true, 0x1, &[0xF8, 0x88, 0x80, 0x80, 0x80], true).await;
    match c.read_until(Duration::from_secs(3), |f| f.opcode == 0x8).await {
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => format!("no close; tcp_eof={}", c.eof(Duration::from_secs(2)).await),
    }
}

async fn case_unmasked(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    c.send(true, 0x1, br#"{"event":"echo"}"#, false).await;
    match c.read_until(Duration::from_secs(3), |f| f.opcode == 0x8).await {
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => format!("no close; tcp_eof={}", c.eof(Duration::from_secs(2)).await),
    }
}

async fn case_oversized_control(addr: SocketAddr) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    c.send(true, 0x9, &vec![b'x'; 126], true).await;
    match c.read_until(Duration::from_secs(3), |f| f.opcode == 0x8).await {
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => format!("no close; tcp_eof={}", c.eof(Duration::from_secs(2)).await),
    }
}

#[tokio::test]
async fn probe_protocol_errors() {
    println!("\n=== P5  §5.2/§5.5/§7.4 — protocol errors expect 1002, bad UTF-8 expects 1007 ===");
    for (name, addr) in adapters().await {
        println!(
            "  {name:<12} reserved op 0x3 : {}",
            case_reserved_opcode(addr).await
        );
        println!("  {name:<12} bad utf-8 text  : {}", case_bad_utf8(addr).await);
        println!("  {name:<12} unmasked frame  : {}", case_unmasked(addr).await);
        println!(
            "  {name:<12} 126-byte ping   : {}",
            case_oversized_control(addr).await
        );
    }
}

// ── P6: payload limits ───────────────────────────────────────────────────────

async fn case_payload(addr: SocketAddr, bytes: usize) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    if !c.status().contains("101") {
        return format!("handshake {}", c.status());
    }
    let pad = "x".repeat(bytes);
    let msg = format!(r#"{{"event":"echo","data":"{pad}"}}"#);
    c.send(true, 0x1, msg.as_bytes(), true).await;
    match c
        .read_until(Duration::from_secs(20), |f| f.opcode == 0x1 || f.opcode == 0x8)
        .await
    {
        Some(f) if f.opcode == 0x1 => format!("accepted -> {}", f.text()),
        Some(f) => format!("CLOSE {:?} {:?}", f.close_code(), f.close_reason()),
        None => format!("no answer; tcp_eof={}", c.eof(Duration::from_secs(2)).await),
    }
}

#[tokio::test]
async fn probe_payload_limits() {
    println!("\n=== P6  payload ceilings — RFC 6455 sets none; each adapter chooses ===");
    for (name, addr) in adapters().await {
        for size in [(1 << 24) - 64, 1 << 24, (1 << 24) + 1024] {
            println!(
                "  {name:<12} {:>9} B : {}",
                size,
                case_payload(addr, size).await
            );
        }
    }
}

async fn adapters() -> Vec<(&'static str, SocketAddr)> {
    vec![
        ("axum", start_http(ulo_http_axum::AxumAdapter::new()).await),
        ("salvo", start_http(ulo_http_salvo::SalvoAdapter::new()).await),
        ("poem", start_http(ulo_http_poem::PoemAdapter::new()).await),
        (
            "rocket",
            start_http(ulo_http_rocket::RocketAdapter::new()).await,
        ),
        ("tungstenite", start_tungstenite().await),
    ]
}

// ── P7: raw byte dump — what actually leaves the server after a bad frame ────

async fn raw_tail(addr: SocketAddr, label: &str, f: impl FnOnce(&mut Raw) -> futures_util::future::BoxFuture<'_, ()>) -> String {
    let mut c = Raw::connect(addr, "/ws", &[]).await;
    if !c.status().contains("101") {
        return format!("handshake {}", c.status());
    }
    f(&mut c).await;
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let mut b = [0u8; 256];
        match tokio::time::timeout(left, c.stream.read(&mut b)).await {
            Ok(Ok(0)) => {
                return format!("{label}: bytes={:02x?} then FIN", out);
            }
            Ok(Ok(n)) => out.extend_from_slice(&b[..n]),
            Ok(Err(e)) => return format!("{label}: bytes={:02x?} then {}", out, e),
            Err(_) => break,
        }
    }
    format!("{label}: bytes={:02x?} socket still open", out)
}

#[tokio::test]
async fn probe_raw_bytes_after_error() {
    use futures_util::FutureExt;
    println!("\n=== P7  raw bytes the server writes after a protocol error ===");
    for (name, addr) in adapters().await {
        println!(
            "  {name:<12} {}",
            raw_tail(addr, "reserved-0x3", |c| async move {
                c.send(true, 0x3, b"x", true).await
            }
            .boxed())
            .await
        );
        println!(
            "  {name:<12} {}",
            raw_tail(addr, "bad-utf8    ", |c| async move {
                c.send(true, 0x1, &[0xF8, 0x88, 0x80, 0x80, 0x80], true).await
            }
            .boxed())
            .await
        );
        println!(
            "  {name:<12} {}",
            raw_tail(addr, "unmasked    ", |c| async move {
                c.send(true, 0x1, br#"{"event":"echo"}"#, false).await
            }
            .boxed())
            .await
        );
        println!(
            "  {name:<12} {}",
            raw_tail(addr, "close-1000  ", |c| async move {
                let mut body = 1000u16.to_be_bytes().to_vec();
                body.extend_from_slice(b"bye");
                c.send(true, 0x8, &body, true).await
            }
            .boxed())
            .await
        );
    }
}

// ── P8: ulo's own refusal frames — envelope + close, as ADR-0036 describes ──

use ulo::async_trait;
use ulo::enhancer::Guard;
use ulo::injectable;
use ulo::ws::WsContext;

#[injectable]
pub struct DenyGuard {}

#[async_trait]
impl Guard<WsContext> for DenyGuard {
    async fn can_activate(&self, _ctx: &WsContext) -> bool {
        false
    }
}

#[websocket_gateway("/deny")]
pub struct DenyGateway {}

#[subscriptions]
#[ulo_macros::use_guards(DenyGuard)]
impl DenyGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[websocket_gateway("/refuse")]
pub struct RefuseGateway {}

#[subscriptions]
impl RefuseGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// A handler that hands back a close frame carrying a code RFC 6455 §7.4.1
    /// says MUST NOT be set by an endpoint.
    #[subscribe_message("reserved")]
    async fn reserved(&self, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::close_with(1005, "reserved code").into())
    }

    /// A code outside every range the registry defines.
    #[subscribe_message("bogus")]
    async fn bogus(&self, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::close_with(7, "below 1000").into())
    }

    #[subscribe_message("refused")]
    async fn refused(&self, _msg: WsMessage) -> WsHandlerResult {
        Err(WsError::Refused {
            code: 4406,
            reason: "subprotocol not acceptable".into(),
        })
    }
}

#[module(providers: [DenyGuard, DenyGateway, RefuseGateway])]
pub struct RefusalModule;

pub async fn start_refusal(adapter: impl ulo::http::HttpAdapter + 'static) -> SocketAddr {
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(RefusalModule).await.unwrap();
        app.use_http_adapter(adapter, ("127.0.0.1", 0)).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.http.unwrap());
        app.run().await;
    });
    rx.await.unwrap()
}

async fn drain(c: &mut Raw, within: Duration) -> String {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return format!("{:?} socket still open", out);
        }
        match c.read_frame(left).await {
            Some(f) if f.opcode == 0x8 => out.push(format!(
                "CLOSE code={:?} reason={:?}",
                f.close_code(),
                f.close_reason()
            )),
            Some(f) => out.push(format!("op={:#x} {:?}", f.opcode, f.text())),
            None => return format!("{:?} then FIN/timeout", out),
        }
    }
}

#[tokio::test]
async fn probe_refusal_frames() {
    println!("\n=== P8  ulo's refusal frames on the wire ===");
    for (name, ctor) in refusal_adapters().await {
        let addr = ctor;
        let mut c = Raw::connect(addr, "/deny", &[]).await;
        println!(
            "  {name:<12} connect guard says no : {}",
            drain(&mut c, Duration::from_secs(2)).await
        );

        let mut c = Raw::connect(addr, "/refuse", &[]).await;
        c.send_text(r#"{"event":"reserved"}"#).await;
        println!(
            "  {name:<12} handler close 1005    : {}",
            drain(&mut c, Duration::from_secs(2)).await
        );

        let mut c = Raw::connect(addr, "/refuse", &[]).await;
        c.send_text(r#"{"event":"bogus"}"#).await;
        println!(
            "  {name:<12} handler close 7       : {}",
            drain(&mut c, Duration::from_secs(2)).await
        );

        let mut c = Raw::connect(addr, "/refuse", &[]).await;
        c.send_text(r#"{"event":"refused"}"#).await;
        println!(
            "  {name:<12} WsError::Refused 4406 : {}",
            drain(&mut c, Duration::from_secs(2)).await
        );

        let mut c = Raw::connect(addr, "/refuse", &[]).await;
        c.send_text(r#"{"event":"no-such-event"}"#).await;
        println!(
            "  {name:<12} unrouted event        : {}",
            drain(&mut c, Duration::from_secs(2)).await
        );

        let mut c = Raw::connect(addr, "/refuse", &[]).await;
        c.send(true, 0x2, b"\x00\x01\x02", true).await;
        println!(
            "  {name:<12} binary frame          : {}",
            drain(&mut c, Duration::from_secs(2)).await
        );
    }
}

async fn refusal_adapters() -> Vec<(&'static str, SocketAddr)> {
    vec![
        (
            "axum",
            start_refusal(ulo_http_axum::AxumAdapter::new()).await,
        ),
        (
            "salvo",
            start_refusal(ulo_http_salvo::SalvoAdapter::new()).await,
        ),
        (
            "poem",
            start_refusal(ulo_http_poem::PoemAdapter::new()).await,
        ),
        (
            "rocket",
            start_refusal(ulo_http_rocket::RocketAdapter::new()).await,
        ),
    ]
}

// ── P9: after a handler's own Close, and what the handshake carries ─────────

#[websocket_gateway("/after")]
pub struct AfterGateway {}

#[subscriptions]
impl AfterGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("bye")]
    async fn bye(&self, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::close_with(1000, "server done").into())
    }

    #[subscribe_message("who")]
    async fn who(&self, client: WsClient) -> WsHandlerResult {
        let h = &client.handshake;
        let mut q: Vec<_> = h.query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        q.sort();
        Ok(WsMessage::text(format!(
            "remote_addr={:?} query=[{}] has_proto_header={}",
            h.remote_addr,
            q.join("&"),
            h.headers.contains_key("sec-websocket-protocol")
        ))
        .into())
    }
}

#[module(providers: [AfterGateway])]
pub struct AfterModule;

pub async fn start_after(adapter: impl ulo::http::HttpAdapter + 'static) -> SocketAddr {
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(AfterModule).await.unwrap();
        app.use_http_adapter(adapter, ("127.0.0.1", 0)).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.http.unwrap());
        app.run().await;
    });
    rx.await.unwrap()
}

#[tokio::test]
async fn probe_after_close_and_handshake() {
    println!("\n=== P9  life after a handler's Close, and what WsHandshake carries ===");
    for (name, addr) in [
        ("axum", start_after(ulo_http_axum::AxumAdapter::new()).await),
        (
            "salvo",
            start_after(ulo_http_salvo::SalvoAdapter::new()).await,
        ),
        ("poem", start_after(ulo_http_poem::PoemAdapter::new()).await),
        (
            "rocket",
            start_after(ulo_http_rocket::RocketAdapter::new()).await,
        ),
    ] {
        let mut c = Raw::connect(addr, "/after", &[]).await;
        c.send_text(r#"{"event":"bye"}"#).await;
        let first = c.read_frame(Duration::from_secs(2)).await;
        // §5.5.1: after sending a Close, the endpoint MUST NOT send more data frames.
        c.send_text(r#"{"event":"who"}"#).await;
        let after = c.read_frame(Duration::from_secs(2)).await;
        println!(
            "  {name:<8} close-then-serve : sent={:?} then={:?}",
            first.as_ref().map(|f| (f.opcode, f.close_code())),
            after.map(|f| (f.opcode, f.text()))
        );

        let mut c = Raw::connect(
            addr,
            "/after?room=front%20desk&tag=a%2Bb&flag",
            &[("Sec-WebSocket-Protocol", "chat")],
        )
        .await;
        c.send_text(r#"{"event":"who"}"#).await;
        println!(
            "  {name:<8} handshake        : {:?}",
            c.read_frame(Duration::from_secs(2)).await.map(|f| f.text())
        );
    }
}

// ── P10: the cross-process broadcast wire format ────────────────────────────

#[test]
fn probe_wsmessage_serde_shape() {
    println!("\n=== P10  what ulo-ws-redis puts on the Redis channel ===");
    for m in [
        WsMessage::text("hi"),
        WsMessage::binary(vec![0u8, 1, 2, 250]),
        WsMessage::Ping(vec![1, 2]),
        WsMessage::close_with(1008, "nope"),
        WsMessage::close(),
    ] {
        println!("  {:?}", serde_json::to_string(&m).unwrap());
    }
    let big = WsMessage::binary(vec![0xABu8; 1024]);
    println!(
        "  1 KiB binary serialises to {} bytes of JSON",
        serde_json::to_string(&big).unwrap().len()
    );
    // What a peer running an older/newer ulo would do with an unknown variant.
    let unknown = r#"{"Compressed":"x"}"#;
    println!(
        "  unknown variant deserialises to: {:?}",
        serde_json::from_str::<WsMessage>(unknown).map(|_| "ok").map_err(|e| e.to_string())
    );
}

// ── P11: §9.1 — Sec-WebSocket-Extensions ────────────────────────────────────

#[tokio::test]
async fn probe_extensions() {
    println!("\n=== P11  §9.1 — extension negotiation ===");
    for (name, addr) in adapters().await {
        let c = Raw::connect(
            addr,
            "/ws",
            &[("Sec-WebSocket-Extensions", "permessage-deflate")],
        )
        .await;
        println!(
            "  {name:<12} offered permessage-deflate -> {} ext={:?}",
            c.status().trim(),
            c.header("sec-websocket-extensions")
        );
    }
}

// ── P12: §10.2 Origin — the only defence a browser socket has ──────────────

#[tokio::test]
async fn probe_origin() {
    println!("\n=== P12  §10.2 — Origin on the upgrade, with and without CorsMiddleware ===");
    for (name, addr) in adapters().await {
        let c = Raw::connect(addr, "/ws", &[("Origin", "https://evil.example")]).await;
        println!("  {name:<12} plain app        : {}", c.status().trim());
    }
    // The same upgrade through a CORS chain restricted to one origin.
    let mut factory = UloFactory::new();
    factory.use_global_middleware(std::sync::Arc::new(
        ulo::http::middleware::CorsMiddleware::new(ulo::http::middleware::CorsOptions {
            origins: ulo::http::middleware::AllowedOrigins::List(vec![
                "https://good.example".to_string(),
            ]),
            ..ulo::http::middleware::CorsOptions::default()
        }),
    ));
    let (tx, rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    tokio::spawn(async move {
        let mut app = factory.create_with(ProbeModule).await.unwrap();
        app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.http.unwrap());
        app.run().await;
    });
    let addr = rx.await.unwrap();
    let c = Raw::connect(addr, "/ws", &[("Origin", "https://evil.example")]).await;
    println!("  axum+CORS(list)  disallowed origin: {}", c.status().trim());
    let c = Raw::connect(addr, "/ws", &[("Origin", "https://good.example")]).await;
    println!("  axum+CORS(list)  allowed origin   : {}", c.status().trim());
}
