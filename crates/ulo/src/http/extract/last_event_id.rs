use std::convert::Infallible;

use crate::extract::FromContext;
use crate::http::HttpContext;

/// The `Last-Event-ID` a reconnecting SSE client sends back.
///
/// A client that loses a stream reconnects on its own, carrying the `id` of the last event it
/// received. This hands that value to the handler and stops there: what is replayable is a
/// property of the stream, not of the framework, so nothing here resumes anything.
///
/// `None` when no readable `Last-Event-ID` arrived: a first connection, a reconnect where no
/// event had set an `id`, or a value that is not UTF-8, which is logged.
///
/// # Example
///
/// ```rust,ignore
/// #[get("/events")]
/// async fn events(&self, LastEventId(last): LastEventId) -> HttpResponse {
///     let Some(pending) = self.since(last.as_deref()) else {
///         // Nothing further for this client: 204 is what tells it not to return.
///         return HttpResponse::no_content().build();
///     };
///     Sse::new(pending).into()
/// }
/// ```
///
/// Setting the `id` a client sends back is [`SseEvent::id`](crate::http::SseEvent::id). What
/// arrives here is whatever the peer sent, which no event has to have carried — treat it as
/// client input and resolve it against something the server knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastEventId(pub Option<String>);

impl LastEventId {
    pub fn into_inner(self) -> Option<String> {
        self.0
    }

    /// The value as a `&str`, for comparing against a cursor without cloning.
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl FromContext<HttpContext> for LastEventId {
    // Absence is the ordinary first-connection case, not a failure.
    type Error = Infallible;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let Some(value) = ctx.request().headers.get("last-event-id") else {
            return Ok(LastEventId(None));
        };
        match std::str::from_utf8(value.as_bytes()) {
            Ok(id) => Ok(LastEventId(Some(id.to_owned()))),
            Err(_) => {
                // `SseEvent::id` accepts any text but CR, LF and NUL, so an id this server sent
                // can come back in an encoding that is not UTF-8. Reading it as absent restarts
                // the client from the beginning, which is worth saying rather than doing quietly.
                tracing::warn!(
                    "`Last-Event-ID` is not UTF-8 and is read as absent; a client resuming on it \
                     starts over"
                );
                Ok(LastEventId(None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::RequestPart;

    fn ctx_with(header: Option<&[u8]>) -> HttpContext {
        let mut builder = http::Request::builder().uri("/");
        if let Some(value) = header {
            builder = builder.header("last-event-id", value);
        }
        let (parts, _) = builder.body(()).unwrap().into_parts();
        HttpContext::from_parts(RequestPart::from(parts))
    }

    #[tokio::test]
    async fn an_absent_header_is_none() {
        let ctx = ctx_with(None);
        assert_eq!(LastEventId::extract(&ctx).await.unwrap(), LastEventId(None));
    }

    #[tokio::test]
    async fn a_value_is_handed_over_verbatim() {
        let ctx = ctx_with(Some(b"42"));
        assert_eq!(
            LastEventId::extract(&ctx).await.unwrap(),
            LastEventId(Some("42".to_string()))
        );
    }

    /// `SseEvent::id` rejects only CR, LF and NUL, so an id this server sent can come back as
    /// UTF-8 above the ASCII range. It reads, rather than being dropped for not being ASCII.
    #[tokio::test]
    async fn a_utf8_value_reads() {
        let ctx = ctx_with(Some("café".as_bytes()));
        assert_eq!(
            LastEventId::extract(&ctx).await.unwrap(),
            LastEventId(Some("café".to_string()))
        );
    }

    /// Anything else is absent, which restarts the client — the log says so.
    #[tokio::test]
    async fn a_non_utf8_value_is_absent() {
        let ctx = ctx_with(Some(&[0x63, 0x61, 0x66, 0xe9]));
        assert_eq!(LastEventId::extract(&ctx).await.unwrap(), LastEventId(None));
    }
}
