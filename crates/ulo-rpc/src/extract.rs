//! The RPC extractors: [`Payload<T>`] and [`Inbound<T>`], each consuming the call's payload, a bare
//! `Bytes` read as `Payload<Bytes>` is, and `CallHeaders` (in `transport`).

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use ulo::BoxError;
use ulo_transport::{ExtractError, FieldViolation, FromCall, Validate};

use crate::codec::Codec;
use crate::frame::Data;
use crate::queue::Overflow;
use crate::transport::{Body, Rpc, RpcCx};

/// The name an extraction failure gives the payload.
const PAYLOAD: &str = "payload";

/// The call's payload, `d`, decoded by the link's codec: `Payload<NewInvoice>`. `Payload<Bytes>`
/// is raw bytes, refused in `prepare` on a link reporting `binary: false`. A JSON payload that does
/// not fit `T` fails as `Mismatched`, answered `unprocessable` as HTTP answers such a body 422,
/// and one that is not JSON, or CBOR that does not decode as `T`, as `Malformed`, answered
/// `bad_request`.
#[derive(Clone, Debug)]
pub struct Payload<T>(pub T);

impl<T> Payload<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Rpc> for Payload<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let data = take_data(cx)?;
        decode(cx, &data).map(Payload)
    }
}

/// `Valid<Payload<T>>` checks the `T`.
impl<T: Validate> Validate for Payload<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// The payload as raw bytes, the same read as `Payload<Bytes>`.
impl FromCall<Rpc> for Bytes {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let data = take_data(cx)?;
        decode(cx, &data)
    }
}

/// A call's one `d`. A streamed request never reaches a handler reading it: the server answers
/// `open` on such a pattern `bad_request` before dispatch.
fn take_data(cx: &RpcCx) -> Result<Data, ExtractError> {
    match cx.take_body() {
        Some(Body::Data(data)) => Ok(data),
        Some(Body::Stream(_)) | None => Err(ExtractError::missing(PAYLOAD)),
    }
}

fn decode<T: DeserializeOwned>(cx: &RpcCx, data: &Data) -> Result<T, ExtractError> {
    cx.codec().decode(data).map_err(|err| {
        let mismatch = fits_as_json(cx.codec(), data, &err);
        let source = cx.app().redact(err);
        if mismatch { ExtractError::mismatched(PAYLOAD, source) } else { ExtractError::malformed(PAYLOAD, source) }
    })
}

/// Whether a payload that failed to decode is JSON that does not fit the type, which is
/// `Mismatched`, rather than bytes that are not JSON, `Malformed`: HTTP's `Json` rule. serde_json
/// reports most misfits as `Data`, and a few as `Syntax` raised while it reads valid JSON for the
/// type, a map key that is not the number `T` asks for among them, so a `Syntax` failure fits
/// when the payload is UTF-8 JSON read for no type. CBOR's decode and parse are one step, and its
/// failures are `Malformed`.
fn fits_as_json(codec: Codec, data: &Data, error: &BoxError) -> bool {
    let Some(error) = error.downcast_ref::<serde_json::Error>().filter(|_| codec == Codec::Json) else { return false };
    match error.classify() {
        serde_json::error::Category::Data => true,
        serde_json::error::Category::Syntax => {
            std::str::from_utf8(data.as_bytes()).is_ok() && serde_json::from_slice::<serde::de::IgnoredAny>(data.as_bytes()).is_ok()
        }
        _ => false,
    }
}

/// A streamed request's items, `in` frames until `in_end`, each decoded as `T`: a parameter of
/// this type makes the handler's shape a streamed request. An item that does not decode yields
/// `Err` and ends the stream. A request whose items overflow the server's `max_queued_items` ends
/// the same way, with [`ExtractError::Overflowed`], and one whose items overflow its
/// `max_queued_bytes` with [`ExtractError::OverflowedBytes`], the items not yet read dropped, on
/// whatever task reads it; the call is answered `unavailable` with the reason `overflow` whatever
/// the handler returns. An item over `max_queued_bytes` on its own ends the request with
/// [`ExtractError::TooLarge`] naming the bound, and the call is answered `bad_request` with the
/// reason `payload_too_large`. A request still being read on another task when its call ends,
/// its handler having answered before `in_end`, yields there the items that had arrived, then
/// [`ExtractError::CallEnded`].
pub struct Inbound<T> {
    pub(crate) items: BoxStream<'static, Result<T, ExtractError>>,
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Rpc> for Inbound<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &RpcCx) -> Result<Self, ExtractError> {
        let Some(Body::Stream(receiver)) = cx.take_body() else {
            return Err(ExtractError::missing(PAYLOAD));
        };
        let cx = cx.clone();
        let items = futures_util::stream::unfold(Some(receiver), move |receiver| {
            let cx = cx.clone();
            async move {
                let mut receiver = receiver?;
                let Some(data) = receiver.next().await else {
                    let failed = match receiver.halted_at() {
                        Some(Overflow::Items { limit }) => {
                            ExtractError::overflowed(PAYLOAD, u64::try_from(limit).unwrap_or(u64::MAX))
                        }
                        Some(Overflow::Bytes { limit, .. }) => ExtractError::overflowed_bytes(PAYLOAD, limit),
                        Some(Overflow::TooLarge { limit, .. }) => ExtractError::too_large(PAYLOAD, limit),
                        None if receiver.cut_with_call() => ExtractError::call_ended(PAYLOAD),
                        None => return None,
                    };
                    return Some((Err(failed), None));
                };
                match decode::<T>(&cx, &data) {
                    Ok(item) => Some((Ok(item), Some(receiver))),
                    Err(err) => Some((Err(err), None)),
                }
            }
        });
        Ok(Inbound { items: Box::pin(items) })
    }
}

impl<T> Stream for Inbound<T> {
    type Item = Result<T, ExtractError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Whether a payload `codec` fails to decode as `T` reads as a misfit.
    fn misfit<T: DeserializeOwned>(codec: Codec, bytes: &[u8]) -> bool {
        let data = Data::new(bytes.to_vec());
        let error = codec.decode::<T>(&data).err().expect("the payload does not decode");
        fits_as_json(codec, &data, &error)
    }

    /// Q34-38: HTTP's rule on the JSON codec, JSON that does not fit `T` a misfit, a map key that
    /// is not the number `T` asks for among them, and bytes that are not JSON not one; CBOR's
    /// failures never are. Planted break: `Syntax` read as a misfit whatever the bytes.
    #[test]
    fn a_json_payload_that_parses_and_does_not_fit_is_a_misfit() {
        assert!(misfit::<u32>(Codec::Json, b"\"four\""), "a string into a `u32`");
        assert!(misfit::<HashMap<u32, u32>>(Codec::Json, br#"{"a":1}"#), "a key that is not the number asked for");
        assert!(!misfit::<u32>(Codec::Json, b"nope"), "bytes that are not JSON");
        assert!(!misfit::<String>(Codec::Json, b"\"cut off"), "JSON cut off");
        assert!(!misfit::<u32>(Codec::Json, b"12 34"), "JSON with bytes after it");
        let mut cbor = Vec::new();
        ciborium::into_writer(&"four", &mut cbor).expect("CBOR encodes");
        assert!(!misfit::<u32>(Codec::Cbor, &cbor), "a CBOR string into a `u32`");
    }
}

#[cfg(test)]
mod checker_tests {
    use std::collections::HashMap;

    use super::*;

    /// HTTP's `decode_failure` predicate as `ulo-http` writes it, for the comparison.
    fn http_mismatch(bytes: &[u8], error: &serde_json::Error) -> bool {
        match error.classify() {
            serde_json::error::Category::Data => true,
            serde_json::error::Category::Syntax => std::str::from_utf8(bytes).is_ok() && serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok(),
            _ => false,
        }
    }

    fn row<T: DeserializeOwned>(what: &str, bytes: &[u8]) -> (String, bool, bool) {
        let data = Data::new(bytes.to_vec());
        let error = Codec::Json.decode::<T>(&data).err().expect("the payload does not decode");
        let rpc = fits_as_json(Codec::Json, &data, &error);
        let http = http_mismatch(bytes, error.downcast_ref::<serde_json::Error>().expect("a serde_json error"));
        (what.to_owned(), rpc, http)
    }

    /// The RPC JSON codec's classification against HTTP's `Json`, row by row.
    #[test]
    fn rpc_json_classification_against_http() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let rows = vec![
            row::<u32>("a string into u32", b"\"four\""),
            row::<HashMap<u32, u32>>("a key not the number", br#"{"a":1}"#),
            row::<u32>("not JSON", b"nope"),
            row::<String>("cut off (EOF)", b"\"cut off"),
            row::<u32>("trailing bytes", b"12 34"),
            row::<String>("invalid UTF-8 in a string", b"\"\xff\""),
            row::<String>("a lone surrogate escape", br#""\ud800""#),
            row::<f64>("a number past f64", b"1e400"),
            row::<Vec<()>>("nesting past 128", deep.as_bytes()),
            row::<u32>("empty", b""),
            row::<u32>("whitespace only", b"   "),
            row::<(u32, u32)>("an array too short", b"[1]"),
        ];
        for (what, rpc, http) in &rows {
            eprintln!("{what:28} rpc Mismatched={rpc} http Mismatched={http}");
        }
        let differing: Vec<_> = rows.iter().filter(|(_, rpc, http)| rpc != http).map(|(what, ..)| what.as_str()).collect();
        assert!(differing.is_empty(), "the RPC JSON codec classifies these unlike HTTP's `Json`: {differing:?}");
    }
}
