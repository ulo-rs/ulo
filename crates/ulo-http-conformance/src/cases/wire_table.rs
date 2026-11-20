//! Scenarios: what reaches the wire where independent servers were measured to disagree (the
//! wire table of the HTTP salvage record, W1, W5, W6, W9 to W12, W15), and the requests they agreed
//! on, kept as a floor. Each is raw HTTP/1.1, so the bytes are the evidence.

use crate::app::{LATIN1_VALUE, LEAK, UTF8_VALUE};
use crate::wire::{Running, has_header, start, status_of};
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
