use std::fmt;
use std::ops::Deref;

use http::HeaderMap;
use ulo_transport::{ExtractError, FromCall};

use crate::cx::HttpCx;
use crate::transport::Http;

const LAST_EVENT_ID: &str = "last-event-id";

/// One typed header from the `headers` crate: `Header<headers::Authorization<Bearer>>`. Absent is
/// `ExtractError::Missing` naming the header, so `Option<Header<H>>` is `None` then; present and
/// unparsable is `Malformed`.
#[derive(Clone, PartialEq, Eq)]
pub struct Header<H>(pub H);

/// The header's name only: `Authorization` and `Cookie` print their credentials through their own
/// `Debug`, and `{:?}` of an extractor lands in logs.
impl<H: headers::Header> fmt::Debug for Header<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Header").field(&H::name().as_str()).finish_non_exhaustive()
    }
}

impl<H> Deref for Header<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.0
    }
}

impl<H: headers::Header + Send + 'static> FromCall<Http> for Header<H> {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        let name = H::name();
        let param = name.as_str();
        let mut values = cx.headers().get_all(name).iter().peekable();
        if values.peek().is_none() {
            return Err(ExtractError::missing(param));
        }
        H::decode(&mut values).map(Header).map_err(|error| ExtractError::malformed(param, cx.app().redact(Box::new(error))))
    }
}

/// Every request header.
impl FromCall<Http> for HeaderMap {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        Ok(cx.headers().clone())
    }
}

/// The `Last-Event-ID` a reconnecting SSE client sends: `None` on a first connection. It hands over
/// what the client sent and resumes nothing; the handler decides what it means. A value that is
/// not UTF-8 is `ExtractError::Malformed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LastEventId(pub Option<String>);

impl FromCall<Http> for LastEventId {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        let Some(value) = cx.headers().get(LAST_EVENT_ID) else {
            return Ok(LastEventId(None));
        };
        match std::str::from_utf8(value.as_bytes()) {
            Ok(id) => Ok(LastEventId(Some(id.to_owned()))),
            Err(error) => Err(ExtractError::malformed(LAST_EVENT_ID, cx.app().redact(Box::new(error)))),
        }
    }
}

/// The context itself, as a handler parameter.
impl FromCall<Http> for HttpCx {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        Ok(cx.clone())
    }
}

#[cfg(test)]
mod tests {
    use headers::{Authorization, Cookie, HeaderMapExt};

    use super::*;

    #[test]
    fn a_typed_header_s_debug_output_carries_its_name_and_no_value() {
        let bearer = Header(Authorization::bearer("s3cr3t").expect("a valid token"));
        let mut map = HeaderMap::new();
        map.insert("cookie", "sid=abc".parse().expect("a valid value"));
        let cookie = Header(map.typed_get::<Cookie>().expect("the cookie parses"));
        for (text, name) in [(format!("{bearer:?}"), "authorization"), (format!("{cookie:#?}"), "cookie")] {
            assert!(!text.contains("s3cr3t") && !text.contains("sid=abc"), "a value appears in {text}");
            assert!(text.contains(name), "{name} is kept: {text}");
        }
    }
}
