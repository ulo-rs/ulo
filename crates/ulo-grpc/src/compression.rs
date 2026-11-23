//! Message compression (transports DESIGN §6.2, decision 334): the encodings a server decompresses
//! a request in and compresses a reply in, the request's `grpc-encoding` checked against the
//! first, the reply's encoding chosen from the caller's `grpc-accept-encoding` and the second, and
//! the reply's messages compressed one by one as its body is written.
//!
//! A request is decompressed by tonic's codec, `tonic::Streaming::new_request` given the call's
//! encoding, which honours each message's compressed flag and inflates into a buffer capped at the
//! message limit. A reply is compressed here rather than by tonic's `EncodeBody`: a reply is
//! encoded where it is built, by a handler, an error handler or the health and reflection
//! services, none of which knows the call's encoding, so the dispatcher compresses the frames the
//! body writes once the reply is chosen.

use std::fmt;
use std::io::Write as _;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{BufMut, Bytes, BytesMut};
use flate2::write::GzEncoder;
use http::HeaderMap;
use http::header::{CONTENT_LENGTH, HeaderName, HeaderValue};
use http_body::{Body, Frame};
use tonic::codec::CompressionEncoding;
use ulo::BoxError;
use ulo_http::{HttpBody, Response};

pub(crate) const GRPC_ENCODING: HeaderName = HeaderName::from_static("grpc-encoding");
pub(crate) const GRPC_ACCEPT_ENCODING: HeaderName = HeaderName::from_static("grpc-accept-encoding");

/// The compressed flag and the four-byte big-endian length ahead of every gRPC message.
const PREFIX: usize = 5;

/// A message compression gRPC names in `grpc-encoding` and `grpc-accept-encoding`. `identity`, no
/// compression, is not one: a server always accepts it, and sends it when no encoding is agreed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoding {
    /// `gzip` (RFC 1952).
    Gzip,
}

impl Encoding {
    /// The name the headers carry: `gzip`.
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Gzip => "gzip",
        }
    }

    fn named(name: &str) -> Option<Encoding> {
        match name {
            "gzip" => Some(Encoding::Gzip),
            _ => None,
        }
    }

    pub(crate) fn codec(self) -> CompressionEncoding {
        match self {
            Encoding::Gzip => CompressionEncoding::Gzip,
        }
    }

    /// `message` in this encoding.
    fn compress(self, message: &[u8]) -> std::io::Result<Vec<u8>> {
        match self {
            Encoding::Gzip => {
                let mut encoder = GzEncoder::new(Vec::with_capacity(message.len() / 2), flate2::Compression::default());
                encoder.write_all(message)?;
                encoder.finish()
            }
        }
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The server's compression settings as each call reads them.
#[derive(Clone, Debug)]
pub(crate) struct Encodings {
    accepted: Vec<Encoding>,
    sent: Vec<Encoding>,
    /// `grpc-accept-encoding` as every response carries it: the accepted encodings, then
    /// `identity`.
    advertised: HeaderValue,
}

impl Encodings {
    pub(crate) fn new(accepted: &[Encoding], sent: &[Encoding]) -> Self {
        let accepted = distinct(accepted);
        let names: Vec<&str> = accepted.iter().map(|encoding| encoding.as_str()).chain(["identity"]).collect();
        let advertised = HeaderValue::from_str(&names.join(",")).unwrap_or_else(|_| HeaderValue::from_static("identity"));
        Encodings { accepted, sent: distinct(sent), advertised }
    }

    /// The encodings a request may be compressed in, as tonic's codec names them, for the health
    /// and reflection services.
    pub(crate) fn accepted_codecs(&self) -> impl Iterator<Item = CompressionEncoding> + '_ {
        self.accepted.iter().map(|encoding| encoding.codec())
    }

    /// The encoding a request's messages are compressed in: `None` for no `grpc-encoding` and for
    /// `identity`, `Err` with the refusal's message for an encoding the server does not accept.
    pub(crate) fn request(&self, headers: &HeaderMap) -> Result<Option<CompressionEncoding>, String> {
        let Some(value) = headers.get(GRPC_ENCODING) else { return Ok(None) };
        let name = value.to_str().unwrap_or_default();
        if name == "identity" {
            return Ok(None);
        }
        match Encoding::named(name).filter(|encoding| self.accepted.contains(encoding)) {
            Some(encoding) => Ok(Some(encoding.codec())),
            None => Err(format!("this server does not decompress `{}`", String::from_utf8_lossy(value.as_bytes()))),
        }
    }

    /// The encoding a reply's messages are compressed in: the first the caller's
    /// `grpc-accept-encoding` lists that the server sends, every line of the header read as one
    /// list; `None` when there is none.
    pub(crate) fn reply(&self, headers: &HeaderMap) -> Option<Encoding> {
        if self.sent.is_empty() {
            return None;
        }
        headers
            .get_all(GRPC_ACCEPT_ENCODING)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|list| list.split(','))
            .filter_map(|name| Encoding::named(name.trim()))
            .find(|encoding| self.sent.contains(encoding))
    }

    /// `response` as it is written: `grpc-accept-encoding` on its headers unless it carries one,
    /// and, when `encoding` is agreed and the response carries messages, `grpc-encoding` and a
    /// body compressing each one. A response marked [`Uncompressed`], one with a `grpc-encoding`
    /// of its own, and one with a `content-length`, which a compressed body would contradict, are
    /// left uncompressed.
    pub(crate) fn finish(&self, mut response: Response, encoding: Option<Encoding>) -> Response {
        let headers = response.headers_mut();
        headers.entry(GRPC_ACCEPT_ENCODING).or_insert_with(|| self.advertised.clone());
        let Some(encoding) = encoding else { return response };
        if response.body().is_end_stream()
            || response.extensions().get::<Uncompressed>().is_some()
            || response.headers().contains_key(GRPC_ENCODING)
            || response.headers().contains_key(CONTENT_LENGTH)
        {
            return response;
        }
        response.headers_mut().insert(GRPC_ENCODING, HeaderValue::from_static(encoding.as_str()));
        response.map(|body| HttpBody::new(Compressed::new(body, encoding)))
    }
}

/// In a reply's extensions: the reply a [`Response`](crate::Response) marked
/// [`uncompressed`](crate::Response::uncompressed) built, which [`Encodings::finish`] sends
/// uncompressed whatever encoding the call agreed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Uncompressed;

fn distinct(encodings: &[Encoding]) -> Vec<Encoding> {
    let mut kept: Vec<Encoding> = Vec::with_capacity(encodings.len());
    for encoding in encodings {
        if !kept.contains(encoding) {
            kept.push(*encoding);
        }
    }
    kept
}

/// A reply body whose uncompressed messages are written compressed in `encoding`, each with its
/// compressed flag set. A message already flagged, the trailers and an error pass as they are. A
/// message's bytes are held until the message is whole; bytes left over when the body ends, or
/// when its trailers arrive, are written as they came.
pub(crate) struct Compressed<B> {
    inner: B,
    encoding: Encoding,
    pending: BytesMut,
    /// Trailers that arrived behind an incomplete message, written after it.
    held: Option<Frame<Bytes>>,
    ended: bool,
}

impl<B> Compressed<B> {
    pub(crate) fn new(inner: B, encoding: Encoding) -> Self {
        Compressed { inner, encoding, pending: BytesMut::new(), held: None, ended: false }
    }

    /// Every whole message at the front of `pending`, compressed, or `None` when there is none.
    fn whole(&mut self) -> std::io::Result<Option<Bytes>> {
        let mut out = BytesMut::new();
        while let [flag, a, b, c, d, ..] = self.pending[..] {
            let len = u32::from_be_bytes([a, b, c, d]) as usize;
            if self.pending.len() - PREFIX < len {
                break;
            }
            let message = self.pending.split_to(PREFIX + len);
            if flag != 0 {
                out.extend_from_slice(&message);
                continue;
            }
            let compressed = self.encoding.compress(&message[PREFIX..])?;
            match u32::try_from(compressed.len()) {
                Ok(compressed_len) => {
                    out.reserve(PREFIX + compressed.len());
                    out.put_u8(1);
                    out.put_u32(compressed_len);
                    out.extend_from_slice(&compressed);
                }
                // Past what a length prefix holds: the message goes out as it came.
                Err(_) => out.extend_from_slice(&message),
            }
        }
        Ok((!out.is_empty()).then(|| out.freeze()))
    }
}

impl<B> Body for Compressed<B>
where
    B: Body<Data = Bytes, Error = BoxError> + Unpin,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        loop {
            match this.whole() {
                Ok(Some(data)) => return Poll::Ready(Some(Ok(Frame::data(data)))),
                Ok(None) => {}
                Err(error) => return Poll::Ready(Some(Err(Box::new(error)))),
            }
            if let Some(trailers) = this.held.take() {
                return Poll::Ready(Some(Ok(trailers)));
            }
            if this.ended {
                if this.pending.is_empty() {
                    return Poll::Ready(None);
                }
                return Poll::Ready(Some(Ok(Frame::data(this.pending.split().freeze()))));
            }
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => this.pending.extend_from_slice(&data),
                    Err(frame) if this.pending.is_empty() => return Poll::Ready(Some(Ok(frame))),
                    Err(frame) => {
                        this.held = Some(frame);
                        return Poll::Ready(Some(Ok(Frame::data(this.pending.split().freeze()))));
                    }
                },
                Some(Err(error)) => return Poll::Ready(Some(Err(error))),
                None => this.ended = true,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.pending.is_empty() && self.held.is_none() && (self.ended || self.inner.is_end_stream())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use flate2::read::GzDecoder;

    use super::*;

    fn framed(flag: u8, message: &[u8]) -> Vec<u8> {
        let mut out = vec![flag];
        out.extend_from_slice(&u32::try_from(message.len()).expect("a test message fits").to_be_bytes());
        out.extend_from_slice(message);
        out
    }

    /// Each message in `data`, as `(flag, the message's bytes inflated when flagged)`.
    fn messages(mut data: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut found = Vec::new();
        while let [flag, a, b, c, d, rest @ ..] = data {
            let len = u32::from_be_bytes([*a, *b, *c, *d]) as usize;
            let (message, tail) = rest.split_at(len);
            let message = match flag {
                0 => message.to_vec(),
                _ => {
                    let mut inflated = Vec::new();
                    GzDecoder::new(message).read_to_end(&mut inflated).expect("a gzip message");
                    inflated
                }
            };
            found.push((*flag, message));
            data = tail;
        }
        assert!(data.is_empty(), "{} bytes after the last whole message", data.len());
        found
    }

    /// A body yielding `frames` in order.
    struct Frames(std::collections::VecDeque<Frame<Bytes>>);

    impl Body for Frames {
        type Data = Bytes;
        type Error = BoxError;

        fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
            Poll::Ready(self.get_mut().0.pop_front().map(Ok))
        }
    }

    async fn written(frames: Vec<Frame<Bytes>>) -> (Vec<u8>, Option<HeaderMap>) {
        let mut body = Compressed::new(Frames(frames.into()), Encoding::Gzip);
        let mut data = Vec::new();
        let mut trailers = None;
        while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
            match frame.expect("no error").into_data() {
                Ok(bytes) => {
                    assert!(trailers.is_none(), "data after the trailers");
                    data.extend_from_slice(&bytes);
                }
                Err(frame) => trailers = frame.into_trailers().ok(),
            }
        }
        assert!(body.is_end_stream());
        (data, trailers)
    }

    /// Messages split across frames, two in one frame, and one already flagged: each uncompressed
    /// message is written compressed with its flag set, the flagged one as it came, and the
    /// trailers last. Planted break: `whole` copying a flag-0 message as it came fails the flags.
    #[tokio::test]
    async fn each_uncompressed_message_is_compressed_and_flagged() {
        let first = vec![b'a'; 300];
        let second = b"second".to_vec();
        let mut joined = framed(0, &first);
        joined.extend(framed(0, &second));
        let already = framed(1, &Encoding::Gzip.compress(b"third").expect("compresses"));
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
        let frames = vec![
            Frame::data(Bytes::copy_from_slice(&joined[..7])),
            Frame::data(Bytes::copy_from_slice(&joined[7..])),
            Frame::data(Bytes::from(already)),
            Frame::trailers(trailers.clone()),
        ];
        let (data, written_trailers) = written(frames).await;
        assert_eq!(messages(&data), vec![(1, first), (1, second), (1, b"third".to_vec())]);
        assert_eq!(written_trailers, Some(trailers));
    }

    /// A message cut short by the trailers or by the body's end is written as it came, ahead of
    /// what follows it.
    #[tokio::test]
    async fn a_message_cut_short_is_written_as_it_came() {
        let cut = &framed(0, b"whole message")[..9];
        let (data, trailers) = written(vec![Frame::data(Bytes::copy_from_slice(cut)), Frame::trailers(HeaderMap::new())]).await;
        assert_eq!((data.as_slice(), trailers), (cut, Some(HeaderMap::new())));
        let (data, trailers) = written(vec![Frame::data(Bytes::copy_from_slice(cut))]).await;
        assert_eq!((data.as_slice(), trailers), (cut, None));
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    /// The reply's encoding is one the caller lists and the server sends, read across every line
    /// of the header and around spaces; none when either side lacks it.
    #[test]
    fn a_reply_is_compressed_only_in_an_encoding_both_sides_name() {
        let sending = Encodings::new(&[Encoding::Gzip], &[Encoding::Gzip]);
        let silent = Encodings::new(&[Encoding::Gzip], &[]);
        for listed in [&[("grpc-accept-encoding", "gzip")][..], &[("grpc-accept-encoding", "identity, gzip")], &[("grpc-accept-encoding", "deflate"), ("grpc-accept-encoding", " gzip ")]] {
            assert_eq!(sending.reply(&headers(listed)), Some(Encoding::Gzip), "{listed:?}");
            assert_eq!(silent.reply(&headers(listed)), None, "{listed:?}");
        }
        for unlisted in [&[][..], &[("grpc-accept-encoding", "identity")], &[("grpc-accept-encoding", "deflate,zstd")], &[("grpc-accept-encoding", "GZIP")]] {
            assert_eq!(sending.reply(&headers(unlisted)), None, "{unlisted:?}");
        }
    }

    /// `identity` and no header are no compression; an encoding the server lists is the codec's,
    /// and any other, `gzip` included once it is off, is refused naming it.
    #[test]
    fn a_request_encoding_is_identity_one_accepted_or_refused() {
        let accepting = Encodings::new(&[Encoding::Gzip, Encoding::Gzip], &[]);
        let refusing = Encodings::new(&[], &[]);
        assert_eq!(accepting.advertised, "gzip,identity");
        assert_eq!(refusing.advertised, "identity");
        for encodings in [&accepting, &refusing] {
            assert_eq!(encodings.request(&headers(&[])), Ok(None));
            assert_eq!(encodings.request(&headers(&[("grpc-encoding", "identity")])), Ok(None));
            assert_eq!(encodings.request(&headers(&[("grpc-encoding", "zstd")])), Err("this server does not decompress `zstd`".to_owned()));
        }
        assert_eq!(accepting.request(&headers(&[("grpc-encoding", "gzip")])), Ok(Some(CompressionEncoding::Gzip)));
        assert_eq!(refusing.request(&headers(&[("grpc-encoding", "gzip")])), Err("this server does not decompress `gzip`".to_owned()));
    }

    /// A trailers-only response is not compressed and gains no `grpc-encoding`; every response
    /// carries `grpc-accept-encoding`, one it already holds kept.
    #[test]
    fn finish_advertises_on_every_response_and_compresses_only_a_body() {
        let encodings = Encodings::new(&[Encoding::Gzip], &[Encoding::Gzip]);
        let trailers_only = encodings.finish(Response::new(HttpBody::empty()), Some(Encoding::Gzip));
        assert_eq!(trailers_only.headers().get(GRPC_ACCEPT_ENCODING).map(HeaderValue::as_bytes), Some(&b"gzip,identity"[..]));
        assert!(!trailers_only.headers().contains_key(GRPC_ENCODING));
        let mut kept = Response::new(HttpBody::empty());
        kept.headers_mut().insert(GRPC_ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        assert_eq!(encodings.finish(kept, None).headers().get(GRPC_ACCEPT_ENCODING).map(HeaderValue::as_bytes), Some(&b"identity"[..]));
        let carrying = encodings.finish(Response::new(HttpBody::from_bytes(framed(0, b"m"))), Some(Encoding::Gzip));
        assert_eq!(carrying.headers().get(GRPC_ENCODING).map(HeaderValue::as_bytes), Some(&b"gzip"[..]));
        let uncompressed = encodings.finish(Response::new(HttpBody::from_bytes(framed(0, b"m"))), None);
        assert!(!uncompressed.headers().contains_key(GRPC_ENCODING));
    }

    /// `response`'s body read whole, data frames only.
    async fn body_of(response: Response) -> Vec<u8> {
        let mut body = response.into_body();
        let mut data = Vec::new();
        while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
            if let Ok(bytes) = frame.expect("no error").into_data() {
                data.extend_from_slice(&bytes);
            }
        }
        data
    }

    fn marked(mut response: Response) -> Response {
        response.extensions_mut().insert(Uncompressed);
        response
    }

    /// With gzip agreed, a reply marked `Uncompressed` and one carrying a `content-length` are
    /// written as they came, flag 0 and no `grpc-encoding`, while an unmarked one is compressed;
    /// the marker on a trailers-only reply changes nothing there. Planted breaks: `finish` ignoring
    /// the marker, or the `content-length`, compresses that reply.
    #[tokio::test]
    async fn a_marked_reply_and_one_with_a_length_are_sent_uncompressed() {
        let encodings = Encodings::new(&[Encoding::Gzip], &[Encoding::Gzip]);
        let message = framed(0, &[b'm'; 64]);
        let mut lengthed = Response::new(HttpBody::from_bytes(message.clone()));
        lengthed.headers_mut().insert(CONTENT_LENGTH, HeaderValue::from(message.len()));
        for (what, response) in [("marked", marked(Response::new(HttpBody::from_bytes(message.clone())))), ("a content-length", lengthed)] {
            let written = encodings.finish(response, Some(Encoding::Gzip));
            assert!(!written.headers().contains_key(GRPC_ENCODING), "{what}");
            assert_eq!(written.headers().get(GRPC_ACCEPT_ENCODING).map(HeaderValue::as_bytes), Some(&b"gzip,identity"[..]), "{what}");
            assert_eq!(messages(&body_of(written).await), vec![(0, vec![b'm'; 64])], "{what}");
        }

        let unmarked = encodings.finish(Response::new(HttpBody::from_bytes(message)), Some(Encoding::Gzip));
        assert_eq!(unmarked.headers().get(GRPC_ENCODING).map(HeaderValue::as_bytes), Some(&b"gzip"[..]));
        assert_eq!(messages(&body_of(unmarked).await), vec![(1, vec![b'm'; 64])]);

        let trailers_only = encodings.finish(marked(Response::new(HttpBody::empty())), Some(Encoding::Gzip));
        assert!(!trailers_only.headers().contains_key(GRPC_ENCODING));
        assert_eq!(trailers_only.headers().get(GRPC_ACCEPT_ENCODING).map(HeaderValue::as_bytes), Some(&b"gzip,identity"[..]));
        assert!(body_of(trailers_only).await.is_empty());
    }
}
