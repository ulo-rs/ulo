//! Scenarios: extraction.

use http::Method;

use crate::wire::{Exchange, has_header, same_as_reference, start, status_of};
use crate::{Host, Mode};

/// Each extraction failure's status, 400, 413, 415 and 422, with its problem document.
pub async fn failures<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let json = "application/json";
    let cases = [
        (Exchange::get("/page?page=many"), 400),
        (
            Exchange::new(Method::POST, "/small").header("content-type", json).body(r#"{"name":"far-too-long-for-sixteen"}"#),
            413,
        ),
        (Exchange::new(Method::POST, "/json").header("content-type", "text/plain").body(r#"{"name":"a"}"#), 415),
        (Exchange::new(Method::POST, "/valid").header("content-type", json).body(r#"{"name":""}"#), 422),
    ];
    for (exchange, status) in cases {
        let path = exchange.path.clone();
        let reply = same_as_reference(&host, exchange).await;
        assert_eq!(reply.status, status, "{path}");
        assert!(
            reply.header("content-type").is_some_and(|value| value.starts_with("application/problem+json")),
            "{path} answers without a problem document"
        );
    }
    host.stop().await;
}

/// A request body with no `Content-Length`, sent chunked, is held to the route's limit as its
/// chunks arrive: 413 with a problem document for `{"name":"far-too-long-for-sixteen"}` against a
/// limit of 16 bytes, and the same route answers a chunked body within it.
pub async fn chunked_over_the_limit<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let over = r#"{"name":"far-too-long-for-sixteen"}"#;
    let within = r#"{"name":"ok"}"#;
    assert!(over.len() > 16 && within.len() <= 16);
    for (body, status) in [(within, 200), (over, 413)] {
        let mut raw = host.raw().await;
        let head = format!(
            "POST {} HTTP/1.1\r\nHost: suite\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            host.target("/small")
        );
        raw.write(head.as_bytes()).await;
        let (first, second) = body.split_at(body.len() / 2);
        for chunk in [first, second] {
            raw.write(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes()).await;
        }
        raw.write(b"0\r\n\r\n").await;
        let answer = raw.read_to_close("the answer to a chunked request").await;
        assert_eq!(status_of(&answer), Some(status), "a chunked body of {} bytes: {:?}", body.len(), String::from_utf8_lossy(&answer));
        if status == 413 {
            assert!(has_header(&answer, "content-type", "application/problem+json"), "{:?}", String::from_utf8_lossy(&answer));
        }
    }
    host.stop().await;
}
