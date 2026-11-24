//! A caller that goes away while its request body is read reaches the error handlers as
//! `ExtractError::Cancelled`, kind `Cancelled`, through every body extractor, and a body whose
//! bytes are broken is still `Malformed`, kind `BadRequest`: an error handler counting kinds counts
//! the two apart.
//!
//! The clients are raw HTTP/1.1 over a socket: one sends a `Content-Length` of 100 or an open
//! chunk, writes part of it and closes the connection, and the other sends a chunk size line that
//! does not parse. Over HTTP/2 a reset or a dropped connection drops the call before the read
//! ends, so no error handler runs there.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use ulo::{App, BoxError, ErrorHandler, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_http::multipart::Error as MultipartError;
use ulo_http::{BodyStream, Bytes, Http, HttpCx, Multipart, Response};
use ulo_transport::{CallError, ErrorKind, ExtractError};

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(5);

/// What the error handler saw, one entry per error: the kind, and whether the `ExtractError`
/// under it is `Cancelled` or `Malformed`.
static SEEN: Mutex<Vec<(ErrorKind, &'static str)>> = Mutex::new(Vec::new());

struct Count;

impl ErrorHandler<Http> for Count {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        let extract = match call.source_as::<ExtractError>() {
            Some(ExtractError::Cancelled { .. }) => "cancelled",
            Some(ExtractError::Malformed { .. }) => "malformed",
            _ => "other",
        };
        SEEN.lock().unwrap_or_else(PoisonError::into_inner).push((call.kind(), extract));
        Err(Box::new(call))
    }
}

#[controller]
struct Upload;

#[handlers]
#[error_handlers(value = Count)]
impl Upload {
    #[ulo_http::post("/bytes")]
    async fn bytes(&self, body: Bytes) -> String {
        body.len().to_string()
    }

    #[ulo_http::post("/stream")]
    async fn stream(&self, mut body: BodyStream) -> Result<String, BoxError> {
        let mut read = 0;
        while let Some(chunk) = body.next().await {
            read += chunk?.len();
        }
        Ok(read.to_string())
    }

    #[ulo_http::post("/multipart")]
    async fn multipart(&self, mut parts: Multipart) -> Result<String, MultipartError> {
        let mut seen = Vec::new();
        while let Some(field) = parts.next_field().await? {
            seen.push(field.text().await?);
        }
        Ok(seen.join(";"))
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Upload>();
    }
}

/// Writes `raw` and closes the connection.
async fn cut(addr: std::net::SocketAddr, raw: &str) {
    let mut socket = TcpStream::connect(addr).await.expect("the client connects");
    socket.write_all(raw.as_bytes()).await.expect("the request is written");
    socket.flush().await.expect("the request is flushed");
}

/// The entry the error handler records after the first `before`, waiting at most [`PATIENCE`].
async fn next_seen(before: usize) -> (ErrorKind, &'static str) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Some(&entry) = SEEN.lock().unwrap_or_else(PoisonError::into_inner).get(before) {
            return entry;
        }
        assert!(tokio::time::Instant::now() < deadline, "no error handler ran within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn seen() -> usize {
    SEEN.lock().unwrap_or_else(PoisonError::into_inner).len()
}

/// Planted breaks: `caller_left` answering `false`, which files every cut below as `Malformed`
/// and `BadRequest`; and the multipart mapping reading only `TooLarge` out of the read's error,
/// which files the multipart cut as `Malformed`.
#[tokio::test]
async fn a_caller_leaving_mid_body_is_cancelled_and_a_broken_body_is_a_bad_request() {
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the app listens");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the app is bound");
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });

    let head = |path: &str, framing: &str| format!("POST {path} HTTP/1.1\r\nHost: test\r\n{framing}\r\n");
    let multipart = "--XB\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\npart of the val";
    let cases = [
        ("a `Content-Length` cut short", format!("{}0123456789", head("/bytes", "Content-Length: 100\r\n"))),
        ("a chunk cut short", format!("{}a\r\n01234", head("/bytes", "Transfer-Encoding: chunked\r\n"))),
        ("a streamed body cut short", format!("{}0123456789", head("/stream", "Content-Length: 100\r\n"))),
        (
            "a multipart body cut short",
            format!("{}{multipart}", head("/multipart", "Content-Type: multipart/form-data; boundary=XB\r\nContent-Length: 400\r\n")),
        ),
    ];
    for (case, raw) in cases {
        let before = seen();
        cut(addr, &raw).await;
        assert_eq!(next_seen(before).await, (ErrorKind::Cancelled, "cancelled"), "{case}");
    }

    let before = seen();
    cut(addr, &format!("{}zz\r\n0123456789\r\n0\r\n\r\n", head("/bytes", "Transfer-Encoding: chunked\r\n"))).await;
    assert_eq!(next_seen(before).await, (ErrorKind::BadRequest, "malformed"), "a chunk size line that does not parse");

    let kinds: Vec<ErrorKind> = SEEN.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|&(kind, _)| kind).collect();
    let count = |kind| kinds.iter().filter(|&&seen| seen == kind).count();
    assert_eq!((count(ErrorKind::Cancelled), count(ErrorKind::BadRequest)), (4, 1), "the counter's tally: {kinds:?}");
    serving.abort();
}
