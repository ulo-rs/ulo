use std::any::type_name;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use http::StatusCode;
use http_body::Body as _;
use http_body_util::BodyExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::BoxError;
use ulo_transport::{ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Validate};

use crate::body::HttpBody;
use crate::cx::{HttpCx, NoBody};
use crate::extract::de::{Source, from_pairs, parse_form};
use crate::response::Response;
use crate::transport::Http;

const BODY: &str = "body";

/// A JSON body, and a JSON reply: `application/json`, or any `+json` media type on the way in.
/// Consumes the body.
///
/// An empty body is `ExtractError::Missing`, so `Option<Json<T>>` is `None` for a request carrying
/// none. Another media type is `UnsupportedMediaType` (415), a body over the route's limit
/// `TooLarge` (413) before any of it is decoded, a body that is not JSON `Malformed` (400), and
/// JSON that does not fit `T`, a field missing or of the wrong type, `Mismatched` (422, RFC 9110
/// §15.5.21). A body a guard or interceptor took first is `BodyTaken` (500), for `Option<Json<T>>`
/// too.
///
/// A handler reads the body once, so a second body extractor beside it fails to compile:
///
/// ```compile_fail
/// # use ulo::{injectable, routes};
/// # use ulo_http::{Bytes, Json};
/// # #[injectable]
/// # struct Notes;
/// #[routes]
/// impl Notes {
///     #[ulo_http::post("/notes")]
///     fn create(&self, note: Json<String>, raw: Bytes) -> String {
///         format!("{} {}", note.0, raw.len())
///     }
/// }
/// # fn main() {}
/// ```
///
/// The same handler with one of the two compiles:
///
/// ```
/// # use ulo::{injectable, routes};
/// # use ulo_http::{Bytes, Json};
/// # #[injectable]
/// # struct Notes;
/// #[routes]
/// impl Notes {
///     #[ulo_http::post("/notes")]
///     fn create(&self, note: Json<String>) -> String {
///         note.0
///     }
/// }
/// # fn main() {}
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Json<T>(pub T);

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Json<T> {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = take_present::<Self>(cx)?;
            if !media_type(cx).is_some_and(is_json) {
                return Err(ExtractError::UnsupportedMediaType { param: BODY, expected: "application/json" });
            }
            let bytes = collect(cx, body).await?;
            serde_json::from_slice(&bytes).map(Json).map_err(|error| decode_failure(cx, &bytes, error))
        }
    }
}

/// 200, `application/json`.
impl<T: Serialize + Send + 'static> IntoReply<Http> for Json<T> {
    fn into_reply(self, cx: &HttpCx) -> Result<Response, IntoReplyError> {
        let _ = cx;
        let bytes = serde_json::to_vec(&self.0).map_err(IntoReplyError::new)?;
        let mut response = Response::new(HttpBody::from_bytes(bytes));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(response)
    }
}

impl<T: Validate> Validate for Json<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// An `application/x-www-form-urlencoded` body. Consumes the body.
///
/// An empty body is `ExtractError::Missing`; another media type is `UnsupportedMediaType` (415);
/// a body over the route's limit is `TooLarge` (413); a body a guard or interceptor took first is
/// `BodyTaken` (500). Every byte sequence decodes as a form, so what fails is the fit to `T`: a
/// required field the form lacks, or a value that does not parse as its field, is `Mismatched`
/// (422) naming the field, and `Option<Form<T>>` is `None` only for an empty body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Form<T>(pub T);

impl<T> Deref for Form<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Form<T> {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = take_present::<Self>(cx)?;
            if media_type(cx).as_deref() != Some("application/x-www-form-urlencoded") {
                return Err(ExtractError::UnsupportedMediaType { param: BODY, expected: "application/x-www-form-urlencoded" });
            }
            let bytes = collect(cx, body).await?;
            let decoded = parse_form(&bytes);
            let pairs: Vec<(&str, &str)> = decoded.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            from_pairs::<T>(&pairs, Source::Form).map(Form).map_err(|error| error.into_mismatch(BODY, cx))
        }
    }
}

impl<T: Validate> Validate for Form<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// The whole body, up to the route's limit. Consumes the body. A request without a body extracts
/// as empty `Bytes`; a body a guard or interceptor took first is `BodyTaken` (500).
impl FromCall<Http> for Bytes {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = take::<Self>(cx)?;
            collect(cx, body).await
        }
    }
}

/// The body as a stream of chunks, read as it arrives; a chunk past the route's limit ends the
/// stream with an error. Consumes the body.
///
/// The error that ends it at the limit is an `ExtractError::TooLarge`, boxed, and the stream
/// returns `None` after it. A body a guard or interceptor took first fails extraction with
/// `BodyTaken` (500).
pub struct BodyStream {
    pub(crate) body: HttpBody,
    pub(crate) limit: u64,
    pub(crate) read: u64,
}

impl Stream for BodyStream {
    type Item = Result<Bytes, BoxError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.read > this.limit {
            return Poll::Ready(None);
        }
        loop {
            let Some(frame) = ready!(Pin::new(&mut this.body).poll_frame(cx)) else {
                return Poll::Ready(None);
            };
            let data = match frame {
                Ok(frame) => match frame.into_data() {
                    Ok(data) => data,
                    Err(_trailers) => continue,
                },
                Err(error) => return Poll::Ready(Some(Err(error))),
            };
            this.read = this.read.saturating_add(data.len() as u64);
            if this.read > this.limit {
                return Poll::Ready(Some(Err(BoxError::from(ExtractError::TooLarge { param: BODY, limit: this.limit }))));
            }
            return Poll::Ready(Some(Ok(data)));
        }
    }
}

impl FromCall<Http> for BodyStream {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = take::<Self>(cx)?;
            let limit = body_limit(cx);
            refuse_declared_excess(cx, limit)?;
            Ok(BodyStream { body, limit, read: 0 })
        }
    }
}

/// A `multipart/form-data` body, its parts read one at a time. Consumes the body.
///
/// An empty body is `ExtractError::Missing`; a body that is not `multipart/form-data` with a
/// boundary is `UnsupportedMediaType` (415); a declared length over the route's limit is
/// `TooLarge` (413); a body a guard or interceptor took first is `BodyTaken` (500). A body that turns out longer than the limit while its parts are read fails
/// that `next_field` call, or the read of the part under way, with a `multer::Error`; a handler
/// that returns it unclaimed answers 413 as well. Any other `multer::Error` a handler returns, such
/// as a body that breaks the boundary, is `Malformed` (400) naming `body`.
pub struct Multipart {
    pub(crate) inner: multer::Multipart<'static>,
}

impl Multipart {
    /// The next part, `None` after the last.
    pub async fn next_field(&mut self) -> Result<Option<multer::Field<'static>>, multer::Error> {
        self.inner.next_field().await
    }
}

impl FromCall<Http> for Multipart {
    const CONSUMES_BODY: bool = true;

    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let body = take_present::<Self>(cx)?;
            let boundary = cx
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| multer::parse_boundary(value).ok())
                .ok_or(ExtractError::UnsupportedMediaType { param: BODY, expected: "multipart/form-data" })?;
            let limit = body_limit(cx);
            refuse_declared_excess(cx, limit)?;
            Ok(Multipart { inner: multer::Multipart::new(BodyStream { body, limit, read: 0 }, boundary) })
        }
    }
}

/// The route's body limit; the server's for a request that matched no route.
fn body_limit(cx: &HttpCx) -> u64 {
    cx.matched().map_or(cx.inner.config.body_limit, |route| route.body_limit)
}

/// The body for extractor `E`. One taken before `E` ran is `BodyTaken`, logged at `warn` naming
/// `E`: the request carried a body and the server consumed it, so the fault is the server's. A
/// context built with no body slot, as the error handlers' are, is `Missing`.
fn take<E>(cx: &HttpCx) -> Result<HttpBody, ExtractError> {
    match cx.take_unread() {
        Ok(body) => Ok(body),
        Err(NoBody::Taken) => {
            let extractor = type_name::<E>();
            tracing::warn!(
                extractor,
                route = cx.route().unwrap_or(""),
                "the request body was taken before the extractor ran, by a guard or interceptor calling `HttpCx::take_body`; answered 500"
            );
            Err(ExtractError::BodyTaken { param: BODY, extractor })
        }
        Err(NoBody::Absent) => Err(ExtractError::Missing { param: BODY }),
    }
}

/// The body for an extractor `E` that requires one, as [`take`] answers it, and `Missing` when the
/// request carries none, so an `Option` of the extractor is `None` then. A request carries none
/// when its body has ended before it was read or it declares `Content-Length: 0`.
fn take_present<E>(cx: &HttpCx) -> Result<HttpBody, ExtractError> {
    let body = take::<E>(cx)?;
    if body.is_end_stream() || declared_length(cx) == Some(0) {
        return Err(ExtractError::Missing { param: BODY });
    }
    Ok(body)
}

/// A JSON body that failed to decode: `Mismatched` when it is JSON that does not fit the type, and
/// `Malformed` when it is not JSON. serde_json reports most fits as `Data`, but a few as `Syntax`
/// raised while it reads valid JSON for the type, a map key that is not the number `T` asks for
/// among them, so a `Syntax` failure is a fit when the body is UTF-8 JSON read for no type.
fn decode_failure(cx: &HttpCx, bytes: &[u8], error: serde_json::Error) -> ExtractError {
    let mismatch = match error.classify() {
        serde_json::error::Category::Data => true,
        serde_json::error::Category::Syntax => {
            std::str::from_utf8(bytes).is_ok() && serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok()
        }
        _ => false,
    };
    let source = cx.app().redact(Box::new(error));
    if mismatch { ExtractError::Mismatched { param: BODY, source } } else { ExtractError::Malformed { param: BODY, source } }
}

/// The essence of `Content-Type`, the type and subtype lowercased without parameters.
fn media_type(cx: &HttpCx) -> Option<String> {
    let value = cx.headers().get(CONTENT_TYPE)?.to_str().ok()?;
    let essence = value.split(';').next().unwrap_or("").trim();
    Some(essence.to_ascii_lowercase())
}

/// `application/json`, or a structured syntax suffix `+json` (RFC 6839) on any type.
fn is_json(essence: String) -> bool {
    essence == "application/json" || essence.split_once('/').is_some_and(|(_, subtype)| subtype.ends_with("+json"))
}

fn declared_length(cx: &HttpCx) -> Option<u64> {
    cx.headers().get(CONTENT_LENGTH)?.to_str().ok()?.trim().parse().ok()
}

/// 413 from `Content-Length` alone, before any of the body is read.
fn refuse_declared_excess(cx: &HttpCx, limit: u64) -> Result<(), ExtractError> {
    match declared_length(cx) {
        Some(length) if length > limit => Err(ExtractError::TooLarge { param: BODY, limit }),
        _ => Ok(()),
    }
}

/// The whole body, counted against the route's limit as it arrives: `TooLarge` the moment it
/// passes, whatever `Content-Length` claimed. A failed read is `Malformed`.
async fn collect(cx: &HttpCx, mut body: HttpBody) -> Result<Bytes, ExtractError> {
    let limit = body_limit(cx);
    refuse_declared_excess(cx, limit)?;
    let mut buffer = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| ExtractError::Malformed { param: BODY, source: cx.app().redact(error) })?;
        if let Ok(data) = frame.into_data() {
            if (buffer.len() as u64).saturating_add(data.len() as u64) > limit {
                return Err(ExtractError::TooLarge { param: BODY, limit });
            }
            buffer.extend_from_slice(&data);
        }
    }
    Ok(buffer.freeze())
}
