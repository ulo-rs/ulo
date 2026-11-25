//! Scenarios: what reaches the wire where independent servers were measured to disagree (the
//! wire table of the HTTP salvage record, W1, W5, W6, W9 to W12, W15), and the requests they agreed
//! on, kept as a floor. Each is raw HTTP/1.1, so the bytes are the evidence.

use crate::app::{LATIN1_VALUE, LEAK, UTF8_VALUE};
use crate::wire::{PATIENCE, Running, has_header, is_reference, start, status_of, within};
use crate::{Host, Mode};

/// The UTF-8 encoding of U+FFFD, which a lossy conversion writes in place of a byte it cannot read.
const REPLACEMENT: &[u8] = "\u{FFFD}".as_bytes();

/// `method target` as HTTP/1.1 with `Host` and `Connection: close`, then `extra` header lines.
fn request<H: Host>(host: &Running<H>, method: &str, path: &str, extra: &[&str]) -> String {
    let mut request = format!("{method} {} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n", host.target(path), host.authority());
    for line in extra {
        request.push_str(line);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    request
}

/// Everything the host writes for `request`, read until it closes the connection.
async fn exchange<H: Host>(host: &Running<H>, request: &[u8], what: &str) -> Vec<u8> {
    let mut raw = host.raw().await;
    raw.write(request).await;
    raw.read_to_close(what).await
}

/// The head of a raw response, up to the blank line.
fn head(raw: &[u8]) -> &[u8] {
    let end = raw.windows(4).position(|window| window == b"\r\n\r\n").unwrap_or(raw.len());
    &raw[..end]
}

/// The header lines of a raw head named `name`, without case, each as the bytes after the colon,
/// trimmed of spaces.
fn field_lines<'a>(raw: &'a [u8], name: &str) -> Vec<&'a [u8]> {
    head(raw)
        .split(|&byte| byte == b'\n')
        .skip(1)
        .filter_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let colon = line.iter().position(|&byte| byte == b':')?;
            let (found, value) = line.split_at(colon);
            found.eq_ignore_ascii_case(name.as_bytes()).then(|| value[1..].trim_ascii())
        })
        .collect()
}

/// Whether the body after the first head holds `bytes`, in one chunk where it is chunked.
fn carries(raw: &[u8], bytes: &[u8]) -> bool {
    let body = &raw[(head(raw).len() + 4).min(raw.len())..];
    body.windows(bytes.len()).any(|window| window == bytes)
}

fn text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// Two `Set-Cookie` values reach the wire as two field lines, never joined or one dropped (W9).
pub async fn two_set_cookie_lines<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let raw = exchange(&host, request(&host, "GET", "/cookies", &[]).as_bytes(), "GET /cookies").await;
    assert_eq!(status_of(&raw), Some(200), "{}", text(&raw));
    let cookies = field_lines(&raw, "set-cookie");
    assert_eq!(cookies, [b"a=1; Path=/".as_slice(), b"b=2; Path=/".as_slice()], "{}", text(&raw));
    host.stop().await;
}

/// A 204 a handler gave a body, of known length or streamed, and a 304 given one, reach the wire
/// with none: the next response on the same connection starts right after the head (W10,
/// RFC 9112 §6.3).
pub async fn no_content_carries_no_body<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for (path, status, body) in [("/no-content", 204, LEAK), ("/no-content/stream", 204, LEAK), ("/status/304", 304, "status")] {
        let first = format!("GET {} HTTP/1.1\r\nhost: {}\r\n\r\n", host.target(path), host.authority());
        let pipelined = first + &request(&host, "GET", "/hit", &[]);
        let raw = exchange(&host, pipelined.as_bytes(), path).await;
        assert_eq!(status_of(&raw), Some(status), "GET {path}: {}", text(&raw));
        let after = &raw[(head(&raw).len() + 4).min(raw.len())..];
        assert!(!after.starts_with(body.as_bytes()), "GET {path}: the {status}'s body reached the wire: {}", text(&raw));
        assert!(after.starts_with(b"HTTP/1.1 200"), "GET {path}: the pipelined response does not start after the {status}'s head: {}", text(&raw));
        assert!(field_lines(&raw, "content-length").iter().all(|value| *value == b"0"), "GET {path}: a {status} declares content: {}", text(&raw));
    }
    host.stop().await;
}

/// A status outside the registered ones, `599` and `299`, reaches the wire unchanged (W11).
pub async fn unregistered_status_unchanged<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for code in [599u16, 299] {
        let path = format!("/status/{code}");
        let raw = exchange(&host, request(&host, "GET", &path, &[]).as_bytes(), &path).await;
        assert_eq!(status_of(&raw), Some(code), "GET {path}: {}", text(&raw));
    }
    host.stop().await;
}

/// A header value holding CR LF fails the reply where it is built: the client reads the app's 500
/// problem document and no header line the value tried to add (W12).
pub async fn crlf_header_value_refused<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let raw = exchange(&host, request(&host, "GET", "/crlf", &[]).as_bytes(), "GET /crlf").await;
    assert_eq!(status_of(&raw), Some(500), "{}", text(&raw));
    assert!(has_header(&raw, "content-type", "application/problem+json"), "{}", text(&raw));
    assert!(field_lines(&raw, "injected").is_empty() && field_lines(&raw, "x-split").is_empty(), "{}", text(&raw));
    host.stop().await;
}

/// A header value beyond ASCII reaches the wire byte for byte, UTF-8 or not, never with U+FFFD in
/// place of a byte (W15, RFC 9110 §5.5 obs-text).
pub async fn obs_text_header_value_unchanged<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let raw = exchange(&host, request(&host, "GET", "/obs-text", &[]).as_bytes(), "GET /obs-text").await;
    assert_eq!(status_of(&raw), Some(200), "{}", text(&raw));
    assert!(!head(&raw).windows(REPLACEMENT.len()).any(|window| window == REPLACEMENT), "a byte was replaced: {}", text(&raw));
    assert_eq!(field_lines(&raw, "x-utf8"), [UTF8_VALUE], "{}", text(&raw));
    assert_eq!(field_lines(&raw, "x-latin1"), [LATIN1_VALUE], "{}", text(&raw));
    host.stop().await;
}

/// `//hit` is not `/hit`: an empty segment matches no route, 404, so a host collapsing `//` before
/// the app sees it would route past an entry matching the path as sent (W1).
pub async fn double_slash_is_no_route<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let raw = exchange(&host, request(&host, "GET", "//hit", &[]).as_bytes(), "GET //hit").await;
    assert_eq!(status_of(&raw), Some(404), "{}", text(&raw));
    host.stop().await;
}

/// A method no route declares, an extension method or `get` in lower case, methods being
/// case-sensitive, is 501 on a path a route matches (W5, W6, RFC 9110 §9.1).
pub async fn unknown_method_not_implemented<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for method in ["PROPFIND", "get"] {
        let raw = exchange(&host, request(&host, method, "/hit", &[]).as_bytes(), method).await;
        assert_eq!(status_of(&raw), Some(501), "{method} /hit: {}", text(&raw));
    }
    host.stop().await;
}

/// Requests the measured servers agreed on: an absolute-form target, HTTP/1.0 with no `Host`,
/// and a body sent with `Expect: 100-continue` are answered by the route; a request carrying two
/// different `Content-Length` values is 400.
pub async fn agreed_floor<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;

    let absolute = format!("GET http://{}{} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n\r\n", host.authority(), host.target("/hit"), host.authority());
    let raw = exchange(&host, absolute.as_bytes(), "an absolute-form target").await;
    assert_eq!(status_of(&raw), Some(200), "an absolute-form target: {}", text(&raw));
    assert!(carries(&raw, b"hit"), "an absolute-form target: {}", text(&raw));

    let old = format!("GET {} HTTP/1.0\r\n\r\n", host.target("/hit"));
    let raw = exchange(&host, old.as_bytes(), "HTTP/1.0").await;
    assert_eq!(status_of(&raw), Some(200), "HTTP/1.0: {}", text(&raw));
    assert!(carries(&raw, b"hit"), "HTTP/1.0: {}", text(&raw));

    let body = r#"{"name":"ada"}"#;
    let expect = request(&host, "POST", "/json", &["content-type: application/json", &format!("content-length: {}", body.len()), "expect: 100-continue"]) + body;
    let raw = exchange(&host, expect.as_bytes(), "Expect: 100-continue").await;
    let last = text(&raw);
    let final_status = last.split("HTTP/1.1 ").filter_map(|answer| answer.get(..3)?.parse::<u16>().ok()).find(|status| *status != 100);
    assert_eq!(final_status, Some(200), "Expect: 100-continue: {last}");
    assert!(last.contains(body), "Expect: 100-continue: {last}");

    let twice = request(&host, "POST", "/json", &["content-type: application/json", "content-length: 14", "content-length: 15"]) + body;
    let raw = exchange(&host, twice.as_bytes(), "two Content-Length values").await;
    assert_eq!(status_of(&raw), Some(400), "two Content-Length values: {}", text(&raw));

    host.stop().await;
}

/// The response after the first one in `raw`, from the next status line after the first's head;
/// `None` when there is none. The first's body is a problem document or text, which holds no
/// status line.
fn second_response(raw: &[u8]) -> Option<&[u8]> {
    let start = (head(raw).len() + 4).min(raw.len());
    let at = raw[start..].windows(9).position(|window| window == b"HTTP/1.1 ")?;
    Some(&raw[start + at..])
}

/// How many responses `raw` holds: each status line, `HTTP/1.x`, a space and a three-digit code,
/// wherever it starts, a response written right after a body sharing that body's last line. The
/// bodies the suite's routes write hold no status line.
fn responses(raw: &[u8]) -> usize {
    raw.windows(12)
        .filter(|window| window.starts_with(b"HTTP/1.") && window[8] == b' ' && window[9..].iter().all(u8::is_ascii_digit))
        .count()
}

/// A handler answering a final 1xx is answered 500 as a problem document, for 100, 101, 102 and
/// 103 alike, and the request pipelined behind it on the same connection is answered after it.
/// Written as it stands, a 101 hands the connection to whoever holds its upgrade future and a
/// 100, 102 or 103 reads as an interim answer, after which the client takes the next response
/// as this request's.
pub async fn informational_status_refused<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for code in [100u16, 101, 102, 103] {
        let path = format!("/status/{code}");
        let first = format!("GET {} HTTP/1.1\r\nhost: {}\r\n\r\n", host.target(&path), host.authority());
        let pipelined = first + &request(&host, "GET", "/hit", &[]);
        let raw = exchange(&host, pipelined.as_bytes(), &path).await;
        assert_eq!(status_of(&raw), Some(500), "GET {path}: {}", text(&raw));
        assert!(has_header(&raw, "content-type", "application/problem+json"), "GET {path}: {}", text(&raw));
        let second = second_response(&raw).unwrap_or_else(|| panic!("GET {path}: the pipelined request is not answered after the 500: {}", text(&raw)));
        assert!(second.starts_with(b"HTTP/1.1 200"), "GET {path}: the pipelined request is not answered after the 500: {}", text(&raw));
        assert!(carries(second, b"hit"), "GET {path}: {}", text(&raw));
    }
    host.stop().await;
}

/// The 400 a request whose framing or authority is ambiguous gets, read off the raw exchange:
/// with `Connection: close`, the connection closed after it, and the request pipelined behind it
/// never answered.
fn refused_and_closed(raw: &[u8], what: &str) {
    assert_eq!(status_of(raw), Some(400), "{what}: {}", text(raw));
    assert!(has_header(raw, "connection", "close"), "{what}: the 400 does not carry `Connection: close`: {}", text(raw));
    assert_eq!(responses(raw), 1, "{what}: the pipelined request was answered on the same connection: {}", text(raw));
}

/// A request carrying two `Host` lines is 400 with `Connection: close`, nothing pipelined behind
/// it answered (RFC 9112 §3.2), the two lines equal or not.
pub async fn duplicate_host_refused<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let pipelined = request(&host, "GET", "/hit", &[]);
    for second in [host.authority(), "other.example".to_owned()] {
        let twice = format!("GET {} HTTP/1.1\r\nhost: {}\r\nhost: {second}\r\n\r\n", host.target("/hit"), host.authority()) + &pipelined;
        let what = format!("two `Host` lines, the second `{second}`");
        refused_and_closed(&exchange(&host, twice.as_bytes(), &what).await, &what);
    }
    host.stop().await;
}

/// On ulo's hyper backend, the one host here reading requests off the wire for the app: an
/// HTTP/1.1 request with no `Host` field is 400 with `Connection: close` and nothing pipelined
/// behind it is answered (RFC 9112 §3.2), an absolute-form target included; one whose `Host` is
/// empty, the form §3.2 gives a target with no authority, and an absolute-form target whose `Host`
/// names another authority are answered, the target's authority taking the place of `Host`
/// (§3.2.2), and so is an HTTP/2 request, which carries `:authority`. An embedding host owns its
/// wire policy and declares the scenario not applicable; run there, it fails at once.
pub async fn missing_host_refused<H: Host>(mode: Mode) {
    assert!(is_reference::<H>(), "an embedding host owns its wire policy, `Host` included: declare `missing_host_refused` not applicable");
    let host = start::<H>(mode).await;
    let pipelined = request(&host, "GET", "/hit", &[]);
    let bare = format!("GET {} HTTP/1.1\r\n\r\n", host.target("/hit")) + &pipelined;
    refused_and_closed(&exchange(&host, bare.as_bytes(), "no `Host`").await, "no `Host`");
    let absolute = format!("GET http://{}{} HTTP/1.1\r\n\r\n", host.authority(), host.target("/hit")) + &pipelined;
    let what = "an absolute-form target, no `Host`";
    refused_and_closed(&exchange(&host, absolute.as_bytes(), what).await, what);
    for (what, line) in [("an empty `Host`", "host:".to_owned()), ("an absolute-form target naming another `Host`", "host: other.example".to_owned())] {
        let target = if line == "host:" { host.target("/hit") } else { format!("http://{}{}", host.authority(), host.target("/hit")) };
        let answered = format!("GET {target} HTTP/1.1\r\n{line}\r\nconnection: close\r\n\r\n");
        let raw = exchange(&host, answered.as_bytes(), what).await;
        assert_eq!(status_of(&raw), Some(200), "{what}: {}", text(&raw));
    }
    let mut send = crate::cases::drain::h2_connect(&host).await;
    let request = http::Request::get(host.url("/hit")).body(()).expect("the request builds");
    let (response, _) = send.send_request(request, true).expect("the stream opens");
    let response = within(host.timer(), PATIENCE, response).await.expect("the stream answers within the patience").expect("the stream answers");
    assert_eq!(response.status().as_u16(), 200, "HTTP/2, `:authority` and no `Host`");
    host.stop().await;
}

/// The same refusal on an HTTP/1.0 request that asks to keep the connection alive: the 400 does
/// not answer `keep-alive` and the connection closes, the request pipelined behind it never
/// answered. An HTTP/1.0 connection persists only while the server answers `keep-alive` (RFC 9112
/// §9.3), so a server answering it as HTTP/1.1 has `Connection: close` alone to end it.
pub async fn duplicate_host_closes_an_http10_keep_alive<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let pipelined = format!("GET {} HTTP/1.0\r\nhost: {}\r\n\r\n", host.target("/hit"), host.authority());
    let twice = format!("GET {} HTTP/1.0\r\nconnection: keep-alive\r\nhost: {}\r\nhost: other.example\r\n\r\n", host.target("/hit"), host.authority()) + &pipelined;
    let what = "two `Host` lines on an HTTP/1.0 keep-alive request";
    let raw = exchange(&host, twice.as_bytes(), what).await;
    assert_eq!(status_of(&raw), Some(400), "{what}: {}", text(&raw));
    assert!(!has_header(&raw, "connection", "keep-alive"), "{what}: the 400 keeps the connection alive: {}", text(&raw));
    assert_eq!(responses(&raw), 1, "{what}: the pipelined request was answered on the same connection: {}", text(&raw));
    host.stop().await;
}

/// A request carrying `Content-Length` beside `Transfer-Encoding` is answered once and the
/// connection closed, the request pipelined behind it never answered: two readers of such a
/// message can disagree on where it ends, the shape of request smuggling, and RFC 9112 §6.3 has a
/// server close the connection after answering it. The answer is the service's 400 with
/// `Connection: close` where the host hands the app both headers. hyper drops the
/// `Content-Length` as it parses the head, so on hyper and the hosts built on it the app reads
/// the body by its `Transfer-Encoding` alone, which §6.3 also admits, and hyper closes the
/// connection after the answer. The two headers are sent in both orders.
pub async fn content_length_beside_transfer_encoding_refused<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let body = r#"{"name":"ada"}"#;
    let chunked = format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len());
    let length = format!("content-length: {}\r\n", chunked.len());
    // Both orders: hyper 0.14, under rocket, drops a `Content-Length` that follows
    // `Transfer-Encoding` and keeps one that precedes it.
    for (first, second) in [(length.as_str(), "transfer-encoding: chunked\r\n"), ("transfer-encoding: chunked\r\n", length.as_str())] {
        let both = format!(
            "POST {} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\n{first}{second}\r\n{chunked}",
            host.target("/json"),
            host.authority(),
        ) + &request(&host, "GET", "/hit", &[]);
        let what = format!("`{}` then `{}`", first.trim_end(), second.trim_end());
        let raw = exchange(&host, both.as_bytes(), &what).await;
        match status_of(&raw) {
            Some(400) => refused_and_closed(&raw, &what),
            Some(200) => {
                assert!(carries(&raw, body.as_bytes()), "{what}: a 200 that did not read the body by its `Transfer-Encoding`: {}", text(&raw));
                assert_eq!(responses(&raw), 1, "{what}: the pipelined request was answered on the same connection: {}", text(&raw));
            }
            _ => panic!("{what}: neither the 400 nor the body read by its `Transfer-Encoding`: {}", text(&raw)),
        }
    }
    host.stop().await;
}

/// A WebSocket handshake carrying `Transfer-Encoding` is answered 400 with `Connection: close` and
/// never switches protocols, `Content-Length` beside it or not and `websocket` among other
/// `Upgrade` tokens or alone: a handshake has no body, and a 101 hands the connection over with
/// the body's bytes unread. Where the host takes upgrades the connection closes after the 400 and
/// the request pipelined behind it is not answered. An upgrade to `h2c` or `websocketx` carrying
/// `Transfer-Encoding`, which the suite's echo answers 101, never switches either: where the host
/// takes upgrades it is answered the same way. A host declared without `upgrades` switches
/// nothing and frames the body itself, and actix answers a request it has already read behind the
/// 400, so there the pipelined request is not held.
pub async fn handshake_with_transfer_encoding_refused<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let pipelined = request(&host, "GET", "/hit", &[]);
    let cases = [
        ("`Transfer-Encoding: chunked`", "websocket", "transfer-encoding: chunked\r\n"),
        ("`Content-Length` then `Transfer-Encoding`", "websocket", "content-length: 5\r\ntransfer-encoding: chunked\r\n"),
        ("`Transfer-Encoding` then `Content-Length`", "websocket", "transfer-encoding: chunked\r\ncontent-length: 5\r\n"),
        ("`websocket` among other `Upgrade` tokens", "h2c,WebSocket", "transfer-encoding: chunked\r\n"),
    ];
    for (what, upgrade, framing) in cases {
        let handshake = format!(
            "GET {} HTTP/1.1\r\nhost: {}\r\nconnection: Upgrade\r\nupgrade: {upgrade}\r\n{framing}\r\n0\r\n\r\n",
            host.target("/echo"),
            host.authority(),
        ) + &pipelined;
        let raw = exchange(&host, handshake.as_bytes(), what).await;
        assert_eq!(status_of(&raw), Some(400), "{what}: {}", text(&raw));
        assert!(has_header(&raw, "connection", "close"), "{what}: the 400 does not carry `Connection: close`: {}", text(&raw));
        if H::limits().upgrades() {
            assert_eq!(responses(&raw), 1, "{what}: the pipelined request was answered on the same connection: {}", text(&raw));
        }
    }
    // An upgrade to another protocol passes the head check and reaches the suite's echo, which
    // answers any upgrade 101: the service replaces that 101 with the same 400 and close (F702).
    for (what, upgrade) in [("`Upgrade: h2c`", "h2c"), ("`Upgrade: websocketx`", "websocketx")] {
        let what = format!("{what} with `Transfer-Encoding: chunked`");
        let request = format!(
            "POST {} HTTP/1.1\r\nhost: {}\r\nconnection: Upgrade\r\nupgrade: {upgrade}\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n",
            host.target("/echo"),
            host.authority(),
        ) + &pipelined;
        let raw = exchange(&host, request.as_bytes(), &what).await;
        assert_ne!(status_of(&raw), Some(101), "{what}: the request switched protocols: {}", text(&raw));
        if H::limits().upgrades() {
            assert_eq!(status_of(&raw), Some(400), "{what}: {}", text(&raw));
            assert!(has_header(&raw, "connection", "close"), "{what}: the 400 does not carry `Connection: close`: {}", text(&raw));
            assert_eq!(responses(&raw), 1, "{what}: the pipelined request was answered on the same connection: {}", text(&raw));
        }
    }
    host.stop().await;
}

/// A 205 a handler gave a body reaches the wire with none and `Content-Length: 0`, the form every
/// host writes and RFC 9110 §15.3.6 admits, whether the handler left the framing to the host or set
/// `Content-Length` or `Transfer-Encoding` itself, and the request pipelined behind it is answered
/// right after its head.
pub async fn reset_content_carries_no_body<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for path in ["/status/205", "/reset/length", "/reset/transfer-encoding"] {
        let first = format!("GET {} HTTP/1.1\r\nhost: {}\r\n\r\n", host.target(path), host.authority());
        let raw = exchange(&host, (first + &request(&host, "GET", "/hit", &[])).as_bytes(), path).await;
        assert_eq!(status_of(&raw), Some(205), "GET {path}: {}", text(&raw));
        assert_eq!(field_lines(&raw, "content-length"), [b"0".as_slice()], "GET {path}: the 205's length: {}", text(&raw));
        assert!(field_lines(&raw, "transfer-encoding").is_empty(), "GET {path}: the 205 is chunked: {}", text(&raw));
        let after = &raw[(head(&raw).len() + 4).min(raw.len())..];
        assert!(after.starts_with(b"HTTP/1.1 200"), "GET {path}: the pipelined response does not start after the 205's head: {}", text(&raw));
    }
    host.stop().await;
}
