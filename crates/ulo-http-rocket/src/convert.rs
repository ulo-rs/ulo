//! The conversion at rocket's edge, both directions. rocket 0.5 is on `http` 0.2 and carries its own
//! method, header and status types, so the head crosses as strings and bytes.

use std::io::{self, Cursor};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use http::StatusCode;
use http::header::{CONTENT_LENGTH, UPGRADE};
use http_body::Body as _;
use http_body_util::BodyExt;
use rocket::http::Status;
use tokio::io::{AsyncRead, ReadBuf};
use ulo_http::HttpBody;

use crate::upgrade::Handoff;

/// rocket's request head as the app's. rocket exposes no protocol version, so the head says
/// HTTP/1.1, which is what rocket serves without TLS.
pub(crate) fn head(request: &rocket::Request<'_>) -> Result<http::request::Parts, http::Error> {
    let mut builder = http::Request::builder()
        .method(request.method().as_str())
        .uri(request.uri().to_string())
        .version(http::Version::HTTP_11);
    for header in request.headers().iter() {
        builder = builder.header(header.name().as_str(), header.value());
    }
    Ok(builder.body(())?.into_parts().0)
}

/// The name of the first header in `headers` whose value holds obs-text bytes that are no UTF-8,
/// which RFC 9110 §5.5 admits and rocket 0.5, carrying a header value as a `str`, cannot hold.
fn not_text(headers: &http::HeaderMap) -> Option<&http::HeaderName> {
    headers.iter().find(|(_, value)| std::str::from_utf8(value.as_bytes()).is_err()).map(|(name, _)| name)
}

/// The app's response as rocket's. A body of known length is collected and handed over sized, so
/// rocket writes its `Content-Length`; any other streams. A 101 carries `handoff` as the I/O
/// handler for the protocol its `Upgrade` header names, and rocket then writes the switch itself.
///
/// rocket writes `Content-Length` from the size of the body it is handed and keeps every header
/// set beside it, so the app's own `Content-Length` is not copied: hyper refuses a response
/// carrying two. For a `HEAD` request the app's `Content-Length` is the length of the body it
/// omitted, handed over as the size of an empty sized body, which rocket keeps when it strips
/// the body of a `HEAD` answer.
///
/// A response carrying a header value that is no UTF-8 is answered with the app's 500 problem
/// document in its place, logged at `error` naming the header, as a value holding CR or LF is
/// refused where the reply is built: left out, a `Set-Cookie` or a `Content-Security-Policy`
/// would weaken the response unseen, and written lossily it would carry U+FFFD in place of a byte.
pub(crate) async fn response<'r>(response: ulo_http::Response, handoff: Option<Handoff>, head: bool) -> rocket::Response<'r> {
    let response = match not_text(response.headers()) {
        Some(name) => {
            tracing::error!(
                header = name.as_str(),
                "a response header value holds bytes that are no UTF-8, which rocket cannot carry; answered 500 in its place"
            );
            ulo_http::__private::internal_error()
        }
        None => response,
    };
    let (parts, body) = response.into_parts();
    let mut builder = rocket::Response::build();
    builder.status(Status::new(parts.status.as_u16()));
    for (name, value) in &parts.headers {
        if name == CONTENT_LENGTH {
            continue;
        }
        // Every value is UTF-8 once `not_text` has found none that is not.
        if let Ok(value) = std::str::from_utf8(value.as_bytes()) {
            builder.raw_header_adjoin(name.as_str().to_owned(), value.to_owned());
        }
    }
    if parts.status == StatusCode::SWITCHING_PROTOCOLS {
        let protocol = parts.headers.get(UPGRADE).and_then(|value| value.to_str().ok());
        if let (Some(handoff), Some(protocol)) = (handoff, protocol) {
            builder.upgrade(protocol.to_owned(), handoff);
        }
        return builder.finalize();
    }
    let omitted = parts.headers.get(CONTENT_LENGTH).and_then(|value| value.to_str().ok()?.parse::<usize>().ok());
    if let (true, Some(length)) = (head, omitted) {
        builder.sized_body(length, Cursor::new(Bytes::new()));
        return builder.finalize();
    }
    match body.size_hint().exact() {
        Some(0) => {}
        Some(_) => match body.collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                builder.sized_body(bytes.len(), Cursor::new(bytes));
            }
            Err(error) => {
                tracing::error!(%error, "the app's response body failed before rocket could send it; sending it empty");
            }
        },
        None => {
            builder.streamed_body(BodyReader { body, chunk: Bytes::new(), done: false });
        }
    }
    builder.finalize()
}

/// The app's body as the `AsyncRead` rocket streams, a read answering as soon as one frame has
/// arrived, so an SSE event is written when the app yields it.
struct BodyReader {
    body: HttpBody,
    chunk: Bytes,
    done: bool,
}

impl AsyncRead for BodyReader {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while this.chunk.is_empty() {
            if this.done {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut this.body).poll_frame(cx)) {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        this.chunk = data;
                    }
                }
                Some(Err(error)) => {
                    this.done = true;
                    return Poll::Ready(Err(io::Error::other(error)));
                }
                None => {
                    this.done = true;
                    return Poll::Ready(Ok(()));
                }
            }
        }
        let n = this.chunk.len().min(buf.remaining());
        buf.put_slice(&this.chunk.split_to(n));
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use http::HeaderValue;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata};
    use ulo_http::HttpBody;

    /// A UTF-8 value beyond ASCII crosses byte for byte.
    #[tokio::test]
    async fn a_utf8_header_value_crosses_unchanged() {
        let mut app = ulo_http::Response::new(HttpBody::from("obs-text"));
        app.headers_mut().insert("x-utf8", HeaderValue::from_bytes("café".as_bytes()).expect("a valid value"));
        app.headers_mut().append("x-utf8", HeaderValue::from_static("plain"));

        let rocket = super::response(app, None, false).await;
        assert_eq!(rocket.status().code, 200);
        assert_eq!(rocket.headers().get("x-utf8").collect::<Vec<_>>(), ["café", "plain"]);
    }

    /// A value holding a byte that is no UTF-8 refuses the whole response: the app's 500 problem
    /// document goes out in its place, none of the refused response's headers or body with it, and
    /// one `error` names the header.
    #[tokio::test]
    async fn a_header_value_that_is_no_utf8_answers_500_logged_at_error_naming_it() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let _default = tracing::subscriber::set_default(Errors(Arc::clone(&lines)));
        let mut app = ulo_http::Response::new(HttpBody::from("obs-text"));
        app.headers_mut().insert("set-cookie", HeaderValue::from_static("session=1; Secure"));
        app.headers_mut().insert("x-latin1", HeaderValue::from_bytes(b"caf\xe9").expect("obs-text is a valid value"));

        let mut rocket = super::response(app, None, false).await;
        assert_eq!(rocket.status().code, 500);
        assert_eq!(rocket.headers().get_one("content-type"), Some("application/problem+json"));
        assert_eq!(rocket.headers().get("set-cookie").count() + rocket.headers().get("x-latin1").count(), 0, "a header of the refused response crossed");
        let body = rocket.body_mut().to_string().await.expect("the document reads");
        assert!(body.contains("\"status\":500") && !body.contains("obs-text"), "{body}");
        let logged = lines.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(logged[0].starts_with("ERROR ") && logged[0].contains("header=x-latin1"), "{logged:?}");
    }

    /// Every `error` event on the thread it is the default subscriber of, as its level, its
    /// message, then `name=value` for each other field.
    struct Errors(Arc<Mutex<Vec<String>>>);

    impl tracing::Subscriber for Errors {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() <= Level::ERROR
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut line = format!("{} ", event.metadata().level());
            event.record(&mut Fields(&mut line));
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push(line);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    struct Fields<'a>(&'a mut String);

    impl Visit for Fields<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push_str(&format!(" {}={value}", field.name()));
        }
    }
}
