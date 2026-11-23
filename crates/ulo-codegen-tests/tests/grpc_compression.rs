//! gzip message compression on the server: a request decompressed per message, on unary and
//! streamed calls, from a tonic client with gzip enabled and from hand-written frames; a reply
//! compressed only when the caller's `grpc-accept-encoding` and the server's `send_encodings` both
//! name gzip; `grpc-accept-encoding` on every response; a compressed flag on a call naming no
//! encoding refused INTERNAL; a message past the 4 MiB limit, as it arrives or once inflated,
//! refused RESOURCE_EXHAUSTED, a bomb included, whose inflate stops at the limit. The refusal of
//! an encoding the server does not decompress is in `grpc_unsupported_encoding.rs`.
//!
//! A tonic client exposes no message's compressed flag, so the flags are read off a plain HTTP/2
//! client's frames.

mod support;

use std::io::{Read as _, Write as _};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use futures_util::{Stream, StreamExt, stream};
use http::{HeaderMap, Request};
use http_body_util::{BodyExt, Full};
use prost::Message as _;
use prost::bytes::Bytes;
use tonic::codec::CompressionEncoding;
use tonic::Status;
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_client::HealthClient;
use ulo::{Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::trailing_client::TrailingClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{CallError, Encoding, Message, Streaming};

use support::{Running, within};

const UPPER: &str = "/codegen.probe.v1.Known/Upper";
const SINCE: &str = "/codegen.probe.v1.Known/Since";
const CHAT: &str = "/codegen.probe.v1.Trailing/Chat";
const WATCH: &str = "/codegen.probe.v1.Shapes/Watch";

const LIMIT: usize = 4 * 1024 * 1024;

#[controller]
struct Known;

#[handlers]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }

    #[ulo_grpc::method(probe::known::Since)]
    fn since(&self, _at: Message<prost_types::Timestamp>) -> Result<prost_types::Duration, Status> {
        Err(Status::failed_precondition("no clock here"))
    }
}

#[controller]
struct Trailing;

#[handlers]
impl Trailing {
    /// One `Tick` per request message, its `n` the message's `count`; a request message that fails
    /// ends the reply with its status.
    #[ulo_grpc::method(probe::trailing::Chat)]
    fn chat(&self, requests: Streaming<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> + use<> {
        requests.map(|request| request.map(|ticks| Tick { n: ticks.count }).map_err(ulo_grpc::from_status))
    }
}

#[controller]
struct Shapes;

#[handlers]
impl Shapes {
    #[ulo_grpc::method(probe::shapes::Watch)]
    fn watch(&self, req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> + use<> {
        stream::iter((0..req.0.count).map(|n| Ok(Tick { n })))
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Known>();
        m.controller::<Trailing>();
        m.controller::<Shapes>();
    }
}

fn server() -> ulo_grpc::Server {
    ulo_grpc::Server::new("127.0.0.1:0").reflection(false)
}

fn sending() -> ulo_grpc::Server {
    server().send_encodings([Encoding::Gzip])
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes).unwrap_or_else(|error| panic!("gzip: {error}"));
    encoder.finish().unwrap_or_else(|error| panic!("gzip: {error}"))
}

/// One length-prefixed message: the compressed flag, the length, the bytes.
fn frame(flag: u8, message: &[u8]) -> Vec<u8> {
    let mut out = vec![flag];
    out.extend_from_slice(&u32::try_from(message.len()).unwrap_or_else(|_| panic!("a test message fits")).to_be_bytes());
    out.extend_from_slice(message);
    out
}

/// A reply read whole off a plain HTTP/2 client.
struct Raw {
    headers: HeaderMap,
    data: Bytes,
    trailers: Option<HeaderMap>,
}

impl Raw {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
    }

    /// `grpc-status`, from the trailers, or from the head of a trailers-only reply.
    fn status(&self) -> Option<String> {
        let trailers = self.trailers.as_ref().and_then(|trailers| trailers.get("grpc-status"));
        trailers.or_else(|| self.headers.get("grpc-status")).map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
    }

    /// Each message as its compressed flag and its bytes, inflated when flagged.
    fn messages(&self) -> Vec<(u8, Vec<u8>)> {
        let mut data = &self.data[..];
        let mut found = Vec::new();
        while let [flag, a, b, c, d, rest @ ..] = data {
            let len = u32::from_be_bytes([*a, *b, *c, *d]) as usize;
            let (message, tail) = rest.split_at(len);
            let bytes = match flag {
                0 => message.to_vec(),
                _ => {
                    let mut inflated = Vec::new();
                    GzDecoder::new(message).read_to_end(&mut inflated).unwrap_or_else(|error| panic!("a flagged message is not gzip: {error}"));
                    inflated
                }
            };
            found.push((*flag, bytes));
            data = tail;
        }
        assert!(data.is_empty(), "{} bytes after the last whole message", data.len());
        found
    }
}

async fn raw(app: &Running, path: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Raw {
    let (client, _) = app.plain_http2();
    let mut request = Request::post(format!("{}{path}", app.origin())).header("content-type", "application/grpc").header("te", "trailers");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(tonic::body::Body::new(Full::new(Bytes::from(body)))).unwrap_or_else(|error| panic!("bad request: {error}"));
    let reply = within("the reply's head", client.request(request)).await.unwrap_or_else(|error| panic!("the call failed: {error}"));
    let (parts, body) = reply.into_parts();
    let collected = within("the reply's body", body.collect()).await.unwrap_or_else(|error| panic!("the body failed: {error}"));
    let trailers = collected.trailers().cloned();
    Raw { headers: parts.headers, data: collected.to_bytes(), trailers }
}

fn tick(bytes: &[u8]) -> u32 {
    Tick::decode(bytes).unwrap_or_else(|error| panic!("not a Tick: {error}")).n
}

/// A tonic client compressing its unary request and its streamed requests: the server inflates
/// each, and a reply the client accepts compressed decodes. Planted break: the extractors decoding
/// with no encoding answers the unary call INTERNAL and ends the stream on its first message.
#[tokio::test]
async fn a_tonic_client_compressing_its_messages_is_answered() {
    let app = Running::start(Root, sending()).await;
    let channel = app.channel().await;
    let text = "a".repeat(100_000);
    let mut known = KnownClient::new(channel.clone()).send_compressed(CompressionEncoding::Gzip).accept_compressed(CompressionEncoding::Gzip);
    let upper = within("the unary call", known.upper(text.clone())).await.unwrap_or_else(|status| panic!("the unary call failed: {status:?}"));
    assert_eq!(upper.into_inner(), text.to_uppercase());

    let mut trailing = TrailingClient::new(channel).send_compressed(CompressionEncoding::Gzip).accept_compressed(CompressionEncoding::Gzip);
    let requests = stream::iter([3, 1, 4].map(|count| Ticks { count, fail: false }));
    let replies = within("the bidi call", trailing.chat(requests)).await.unwrap_or_else(|status| panic!("the bidi call failed: {status:?}"));
    let replies: Vec<Result<Tick, Status>> = within("the bidi replies", replies.into_inner().collect()).await;
    let counts: Vec<u32> = replies.into_iter().map(|reply| reply.unwrap_or_else(|status| panic!("a reply failed: {status:?}")).n).collect();
    assert_eq!(counts, [3, 1, 4]);
    app.stop().await;
}

/// Each message of a request stream carries its own flag: gzip, none, gzip under one
/// `grpc-encoding: gzip` all decode. The flag is read by tonic's codec, the call's encoding being
/// what the server hands it.
#[tokio::test]
async fn a_request_stream_mixing_compressed_and_uncompressed_messages_is_read_whole() {
    let app = Running::start(Root, server()).await;
    let ticks = |count| Ticks { count, fail: false }.encode_to_vec();
    let body = [frame(1, &gzip(&ticks(7))), frame(0, &ticks(8)), frame(1, &gzip(&ticks(9)))].concat();
    let reply = raw(&app, CHAT, &[("grpc-encoding", "gzip")], body).await;
    assert_eq!(reply.status().as_deref(), Some("0"));
    let counts: Vec<u32> = reply.messages().iter().map(|(_, bytes)| tick(bytes)).collect();
    assert_eq!(counts, [7, 8, 9]);
    app.stop().await;
}

/// A reply is compressed when the caller lists gzip and the server sends it: `grpc-encoding: gzip`
/// on its head and every message flagged, a unary reply and each item of a stream; it is sent
/// uncompressed when the caller lists no encoding the server sends and when the server sends none,
/// its default. Planted breaks: `Encodings::finish` ignoring the agreed encoding fails the first
/// case; `Encodings::reply` ignoring the caller's list fails the second; `send_encodings`
/// defaulting to gzip fails the third.
#[tokio::test]
async fn a_reply_is_compressed_only_when_the_caller_and_the_server_both_name_gzip() {
    let request = frame(0, &"shout".to_owned().encode_to_vec());
    let watch = frame(0, &Ticks { count: 3, fail: false }.encode_to_vec());

    let app = Running::start(Root, sending()).await;
    let reply = raw(&app, UPPER, &[("grpc-accept-encoding", "identity,gzip")], request.clone()).await;
    assert_eq!(reply.header("grpc-encoding").as_deref(), Some("gzip"));
    assert_eq!(reply.messages(), [(1, "SHOUT".to_owned().encode_to_vec())]);
    let streamed = raw(&app, WATCH, &[("grpc-accept-encoding", "gzip")], watch.clone()).await;
    assert_eq!(streamed.header("grpc-encoding").as_deref(), Some("gzip"));
    let items: Vec<(u8, u32)> = streamed.messages().iter().map(|(flag, bytes)| (*flag, tick(bytes))).collect();
    assert_eq!(items, [(1, 0), (1, 1), (1, 2)]);

    for listed in [&[][..], &[("grpc-accept-encoding", "identity,deflate")]] {
        let reply = raw(&app, UPPER, listed, request.clone()).await;
        assert_eq!(reply.header("grpc-encoding"), None, "{listed:?}");
        assert_eq!(reply.messages(), [(0, "SHOUT".to_owned().encode_to_vec())], "{listed:?}");
    }
    app.stop().await;

    let app = Running::start(Root, server()).await;
    let reply = raw(&app, WATCH, &[("grpc-accept-encoding", "gzip")], watch).await;
    assert_eq!(reply.header("grpc-encoding"), None);
    assert!(reply.messages().iter().all(|(flag, _)| *flag == 0), "{:?}", reply.messages());
    app.stop().await;
}

/// Every response names the encodings the server decompresses: a reply carrying a message, a
/// handler's trailers-only error, a path no handler answers, a `grpc-timeout` off the grammar,
/// refused before an execution opens, and the health service's reply. Planted break:
/// `Encodings::finish` writing no `grpc-accept-encoding` fails every case.
#[tokio::test]
async fn every_response_names_the_encodings_the_server_accepts() {
    let upper = frame(0, &"x".to_owned().encode_to_vec());
    let since = frame(0, &prost_types::Timestamp::default().encode_to_vec());
    let check = frame(0, &HealthCheckRequest::default().encode_to_vec());
    for (server, advertised) in [(server(), "gzip,identity"), (server().accept_encodings([]), "identity")] {
        let app = Running::start(Root, server).await;
        let cases = [
            ("a reply", UPPER, None, upper.clone(), "0"),
            ("a handler's error", SINCE, None, since.clone(), "9"),
            ("no handler", "/codegen.probe.v1.Known/Nowhere", None, upper.clone(), "12"),
            ("a bad grpc-timeout", UPPER, Some(("grpc-timeout", "soon")), upper.clone(), "13"),
            ("the health service", "/grpc.health.v1.Health/Check", None, check.clone(), "0"),
        ];
        for (what, path, header, body, status) in cases {
            let reply = raw(&app, path, header.as_slice(), body).await;
            assert_eq!(reply.status().as_deref(), Some(status), "{what}");
            assert_eq!(reply.header("grpc-accept-encoding").as_deref(), Some(advertised), "{what}");
        }
        app.stop().await;
    }
}

/// A compressed flag on a call whose `grpc-encoding` names no compression, absent or `identity`,
/// is INTERNAL, which the compression specification requires, unary and streamed, while a
/// message framed whole that does not decode, compressed or not, stays INVALID_ARGUMENT. Planted
/// breaks: `first_message` reading tonic's INTERNAL as a message that does not decode answers the
/// flagged unary call INVALID_ARGUMENT; the unary extractor decoding with prost's decoder as it
/// stands answers the undecodable one INTERNAL.
#[tokio::test]
async fn a_compressed_flag_on_a_call_naming_no_compression_is_internal() {
    let app = Running::start(Root, server()).await;
    let unary = frame(1, &gzip(&"x".to_owned().encode_to_vec()));
    let streamed = frame(1, &gzip(&Ticks { count: 1, fail: false }.encode_to_vec()));
    for header in [&[][..], &[("grpc-encoding", "identity")]] {
        assert_eq!(raw(&app, UPPER, header, unary.clone()).await.status().as_deref(), Some("13"), "unary, {header:?}");
        assert_eq!(raw(&app, CHAT, header, streamed.clone()).await.status().as_deref(), Some("13"), "streamed, {header:?}");
    }
    let undecodable = [0xff, 0xff, 0xff];
    assert_eq!(raw(&app, UPPER, &[], frame(0, &undecodable)).await.status().as_deref(), Some("3"), "undecodable");
    let gzip_header = [("grpc-encoding", "gzip")];
    assert_eq!(raw(&app, UPPER, &gzip_header, frame(1, &gzip(&undecodable))).await.status().as_deref(), Some("3"), "undecodable, inflated");
    app.stop().await;
}

/// A message past 4 MiB is RESOURCE_EXHAUSTED whether its length prefix says so or it inflates
/// past it, unary and streamed. The length prefix alone is sent for the first: the server refuses
/// it on reading the prefix. Planted breaks: `to_status` without its `TooLarge` code answers the
/// unary cases INVALID_ARGUMENT; `Streaming` passing tonic's OUT_OF_RANGE through answers the
/// streamed prefix case OUT_OF_RANGE.
#[tokio::test]
async fn a_message_past_the_limit_is_resource_exhausted_as_it_arrives_and_once_inflated() {
    let app = Running::start(Root, server()).await;
    let announced = (u32::try_from(LIMIT).unwrap_or(u32::MAX) + 1).to_be_bytes();
    let prefix_only = [&[0u8][..], &announced].concat();
    let inflating = frame(1, &gzip(&"a".repeat(LIMIT + 1024).encode_to_vec()));
    let inflating_ticks = frame(1, &gzip(&Ticks { count: 1, fail: false }.encode_to_vec().into_iter().chain(std::iter::repeat_n(0, LIMIT)).collect::<Vec<u8>>()));
    let cases = [
        ("unary, announced", UPPER, &[][..], prefix_only.clone()),
        ("unary, inflated", UPPER, &[("grpc-encoding", "gzip")][..], inflating),
        ("streamed, announced", CHAT, &[][..], prefix_only),
        ("streamed, inflated", CHAT, &[("grpc-encoding", "gzip")][..], inflating_ticks),
    ];
    for (what, path, headers, body) in cases {
        assert_eq!(raw(&app, path, headers, body).await.status().as_deref(), Some("8"), "{what}");
    }
    app.stop().await;
}

/// How far past the limit the bomb below stays valid deflate.
const VALID_PAST_LIMIT: usize = 1 << 20;

/// A gzip message whose deflate stream inflates to `LIMIT + VALID_PAST_LIMIT` zero bytes and then
/// holds a block of the reserved type 3, which no inflater reads past: a sync flush leaves the
/// stream byte-aligned and every byte before it decodable, and `0x07` is a final block of type 3.
fn bomb() -> Vec<u8> {
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), Compression::fast());
    deflate.write_all(&vec![0; LIMIT + VALID_PAST_LIMIT]).unwrap_or_else(|error| panic!("deflate: {error}"));
    deflate.flush().unwrap_or_else(|error| panic!("deflate: {error}"));
    let header = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    [&header[..], deflate.get_ref(), &[0x07]].concat()
}

/// A message that inflates past the limit is refused once the inflate reaches the limit, never
/// after inflating the whole: the bomb inflates `LIMIT + 1 MiB` bytes before its invalid block, so
/// an inflate that read on, to check the size after, or to any point past that 1 MiB, would fail
/// INTERNAL on the block instead of RESOURCE_EXHAUSTED. The bomb is first checked against a full
/// inflate, which must fail at the block after every valid byte. Planted break: the extractors
/// decoding with a limit of `usize::MAX` answer both calls INTERNAL.
#[tokio::test]
async fn a_bomb_is_refused_when_its_inflate_reaches_the_limit() {
    let bomb = bomb();
    let mut full = GzDecoder::new(&bomb[..]);
    let mut inflated = 0;
    let mut chunk = vec![0; 64 * 1024];
    let failure = loop {
        match full.read(&mut chunk) {
            Ok(0) => panic!("the bomb inflated whole, {inflated} bytes, without reaching its invalid block"),
            Ok(read) => inflated += read,
            Err(error) => break error,
        }
    };
    // A read reaching the block reports the failure in place of the bytes it inflated before it.
    assert!(
        (LIMIT + VALID_PAST_LIMIT - chunk.len()..=LIMIT + VALID_PAST_LIMIT).contains(&inflated),
        "a full inflate fails at the block, after every valid byte, not after {inflated}: {failure}"
    );

    let app = Running::start(Root, server()).await;
    let gzip = [("grpc-encoding", "gzip")];
    assert_eq!(raw(&app, UPPER, &gzip, frame(1, &bomb)).await.status().as_deref(), Some("8"), "unary");
    assert_eq!(raw(&app, CHAT, &gzip, frame(1, &bomb)).await.status().as_deref(), Some("8"), "streamed");
    app.stop().await;
}

/// The health service decompresses a gzip request, configured with the server's encodings.
/// Planted break: the health service built without `accept_compressed` answers UNIMPLEMENTED.
#[tokio::test]
async fn the_health_service_accepts_a_gzip_request() {
    let app = Running::start(Root, server()).await;
    let mut health = HealthClient::new(app.channel().await).send_compressed(CompressionEncoding::Gzip);
    let checked = within("the health check", health.check(HealthCheckRequest::default())).await;
    assert_eq!(checked.map(|reply| reply.into_inner().status).map_err(|status| status.code()), Ok(1), "SERVING");
    app.stop().await;
}
