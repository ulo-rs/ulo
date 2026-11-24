//! The four compression cases of the gRPC project's interop test descriptions, which grpc-go's
//! interop client does not implement: `client_compressed_unary`, `server_compressed_unary`,
//! `client_compressed_streaming` and `server_compressed_streaming`. A tonic client with gzip
//! enabled, taking grpc-go's flags so `interop/run.py` runs it as it runs that client, and logging
//! `<Case> done` on a pass and `FATAL: <reason>` on a failure.
//!
//! Every assertion the descriptions make is made. The compressed flag of each reply message, which
//! a tonic client does not expose, is read off the reply body by [`Tap`]. The one step tonic's
//! client cannot write, an uncompressed message inside a call whose other messages are
//! compressed (`client_compressed_streaming`), is written as frames by hand over the same channel
//! and its reply read by tonic's `Streaming`, each message decoded as prost decodes it.

use std::future::Future;
use std::io::Write as _;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use flate2::Compression;
use flate2::write::GzEncoder;
use futures_util::{StreamExt, stream};
use tower::Service;
use ulo_grpc::prost::Message as _;
use ulo_grpc::tonic::codec::{CompressionEncoding, DecodeBuf, Decoder};
use ulo_grpc::tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use ulo_grpc::tonic::{Code, Status};
use ulo_grpc::{prost, tonic};
use ulo_grpc_interop::pb::test_service_client::TestServiceClient;
use ulo_grpc_interop::pb::{
    BoolValue, Payload, ResponseParameters, SimpleRequest, StreamingInputCallRequest, StreamingInputCallResponse,
    StreamingOutputCallRequest,
};
use ulo_grpc_interop::{Flags, Tap};

/// The cases this client runs, each with the name its `done` line carries.
const CASES: [(&str, &str); 4] = [
    ("client_compressed_unary", "ClientCompressedUnary"),
    ("server_compressed_unary", "ServerCompressedUnary"),
    ("client_compressed_streaming", "ClientCompressedStreaming"),
    ("server_compressed_streaming", "ServerCompressedStreaming"),
];

/// grpc-go's flags, as `run.py` passes them.
#[derive(Default)]
struct Args {
    host: String,
    port: String,
    case: String,
    tls: bool,
    ca_file: Option<String>,
    host_override: Option<String>,
}

fn args() -> Result<Args, String> {
    let mut args = Args::default();
    for arg in std::env::args().skip(1) {
        let Some((name, value)) = arg.strip_prefix("--").and_then(|flag| flag.split_once('=')) else {
            return Err(format!("expected `--name=value`, got `{arg}`"));
        };
        let value = value.to_owned();
        match name {
            "server_host" => args.host = value,
            "server_port" => args.port = value,
            "test_case" => args.case = value,
            "use_tls" => args.tls = value == "true",
            // The CA comes from `--ca_file`; the flag only says one is given.
            "use_test_ca" => {}
            "ca_file" => args.ca_file = Some(value),
            "server_host_override" => args.host_override = Some(value),
            other => return Err(format!("unknown flag `--{other}`")),
        }
    }
    if args.host.is_empty() || args.port.is_empty() || args.case.is_empty() {
        return Err("`--server_host`, `--server_port` and `--test_case` are required".into());
    }
    Ok(args)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let args = match args() {
        Ok(args) => args,
        Err(reason) => {
            eprintln!("FATAL: {reason}");
            return std::process::ExitCode::from(2);
        }
    };
    let Some((_, done)) = CASES.iter().find(|(case, _)| *case == args.case) else {
        eprintln!("FATAL: unknown test case `{}`", args.case);
        return std::process::ExitCode::from(2);
    };
    let outcome = match channel(&args).await {
        Ok(channel) => run(&args.case, channel).await,
        Err(reason) => Err(reason),
    };
    match outcome {
        Ok(()) => {
            println!("{done} done");
            std::process::ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("FATAL: {reason}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn channel(args: &Args) -> Result<Channel, String> {
    let scheme = if args.tls { "https" } else { "http" };
    let mut endpoint = Endpoint::from_shared(format!("{scheme}://{}:{}", args.host, args.port)).map_err(|error| error.to_string())?;
    if args.tls {
        let mut tls = ClientTlsConfig::new().domain_name(args.host_override.clone().unwrap_or_else(|| args.host.clone()));
        if let Some(ca) = &args.ca_file {
            let pem = std::fs::read(ca).map_err(|error| format!("reading `{ca}`: {error}"))?;
            tls = tls.ca_certificate(Certificate::from_pem(pem));
        }
        endpoint = endpoint.tls_config(tls).map_err(|error| error.to_string())?;
    }
    endpoint.connect().await.map_err(|error| format!("connecting: {error}"))
}

async fn run(case: &str, channel: Channel) -> Result<(), String> {
    match case {
        "client_compressed_unary" => client_compressed_unary(channel).await,
        "server_compressed_unary" => server_compressed_unary(channel).await,
        "client_compressed_streaming" => client_compressed_streaming(channel).await,
        _ => server_compressed_streaming(channel).await,
    }
}

fn zeros(size: usize) -> Option<Payload> {
    Some(Payload { body: vec![0; size], ..Default::default() })
}

fn flag(value: bool) -> Option<BoolValue> {
    Some(BoolValue { value })
}

fn failed(step: &str, status: Status) -> String {
    format!("{step}: the call failed: {:?} {}", status.code(), status.message())
}

/// The payload size of a successful reply, against `expected`.
fn sized(step: &str, payload: Option<&Payload>, expected: usize) -> Result<(), String> {
    let size = payload.map_or(0, |payload| payload.body.len());
    if size == expected { Ok(()) } else { Err(format!("{step}: the reply's payload is {size} bytes, expected {expected}")) }
}

async fn client_compressed_unary(channel: Channel) -> Result<(), String> {
    let request = |expect| SimpleRequest { expect_compressed: flag(expect), response_size: 314_159, payload: zeros(271_828), ..Default::default() };
    let mut plain = TestServiceClient::new(channel.clone());
    let mut compressing = TestServiceClient::new(channel).send_compressed(CompressionEncoding::Gzip);

    match plain.unary_call(request(true)).await {
        Err(status) if status.code() == Code::InvalidArgument => {}
        Err(status) => return Err(failed("the uncompressed probe", status)),
        Ok(_) => return Err("the uncompressed probe expecting compression succeeded, expected INVALID_ARGUMENT".into()),
    }
    let reply = compressing.unary_call(request(true)).await.map_err(|status| failed("the compressed call", status))?;
    sized("the compressed call", reply.get_ref().payload.as_ref(), 314_159)?;
    let reply = plain.unary_call(request(false)).await.map_err(|status| failed("the uncompressed call", status))?;
    sized("the uncompressed call", reply.get_ref().payload.as_ref(), 314_159)
}

async fn server_compressed_unary(channel: Channel) -> Result<(), String> {
    let flags = Flags::default();
    let mut client = TestServiceClient::new(Recorded { channel, flags: flags.clone() }).accept_compressed(CompressionEncoding::Gzip);
    for compressed in [true, false] {
        let request = SimpleRequest { response_compressed: flag(compressed), response_size: 314_159, payload: zeros(271_828), ..Default::default() };
        let step = format!("response_compressed {compressed}");
        let reply = client.unary_call(request).await.map_err(|status| failed(&step, status))?;
        sized(&step, reply.get_ref().payload.as_ref(), 314_159)?;
        match flags.take_all()[..] {
            [seen] if seen == compressed => {}
            [seen] => return Err(format!("{step}: the reply's compressed flag is {}", u8::from(seen))),
            ref other => return Err(format!("{step}: {} reply messages, expected one", other.len())),
        }
    }
    Ok(())
}

async fn client_compressed_streaming(channel: Channel) -> Result<(), String> {
    let request = |expect, size| StreamingInputCallRequest { expect_compressed: flag(expect), payload: zeros(size) };
    let mut plain = TestServiceClient::new(channel.clone());
    match plain.streaming_input_call(stream::iter([request(true, 27_182)])).await {
        Err(status) if status.code() == Code::InvalidArgument => {}
        Err(status) => return Err(failed("the uncompressed probe", status)),
        Ok(_) => return Err("the uncompressed probe expecting compression succeeded, expected INVALID_ARGUMENT".into()),
    }

    let compressed = gzip(&request(true, 27_182).encode_to_vec()).map_err(|error| format!("gzip: {error}"))?;
    let body = [frame(1, &compressed), frame(0, &request(false, 45_904).encode_to_vec())].concat();
    let reply: StreamingInputCallResponse = by_hand(channel, "/grpc.testing.TestService/StreamingInputCall", body).await?;
    match reply.aggregated_payload_size {
        73_086 => Ok(()),
        other => Err(format!("the aggregated payload size is {other}, expected 73086")),
    }
}

async fn server_compressed_streaming(channel: Channel) -> Result<(), String> {
    let flags = Flags::default();
    let mut client = TestServiceClient::new(Recorded { channel, flags: flags.clone() }).accept_compressed(CompressionEncoding::Gzip);
    let parameters = |compressed, size| ResponseParameters { compressed: flag(compressed), size, ..Default::default() };
    let request = StreamingOutputCallRequest { response_parameters: vec![parameters(true, 31_415), parameters(false, 92_653)], ..Default::default() };
    let replies = client.streaming_output_call(request).await.map_err(|status| failed("the call", status))?;
    let replies: Vec<_> = replies.into_inner().collect().await;
    let expected = [(true, 31_415), (false, 92_653)];
    if replies.len() != expected.len() {
        return Err(format!("{} responses, expected 2", replies.len()));
    }
    let seen = flags.take_all();
    for (index, (reply, (compressed, size))) in replies.into_iter().zip(expected).enumerate() {
        let step = format!("response {}", index + 1);
        let reply = reply.map_err(|status| failed(&step, status))?;
        sized(&step, reply.payload.as_ref(), size)?;
        if seen.get(index) != Some(&compressed) {
            let written = seen.get(index).map_or("absent".to_owned(), |flag| u8::from(*flag).to_string());
            let asked = if compressed { "compression" } else { "no compression" };
            return Err(format!("{step} asks for {asked}: its compressed flag is {written}"));
        }
    }
    Ok(())
}

fn gzip(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}

fn frame(flag: u8, message: &[u8]) -> Vec<u8> {
    let mut out = vec![flag];
    out.extend_from_slice(&u32::try_from(message.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(message);
    out
}

/// A client-streamed call to `path` whose body is `body`, written as it is under
/// `grpc-encoding: gzip`, and its one reply message.
async fn by_hand<T: prost::Message + Default + Send + 'static>(mut channel: Channel, path: &str, body: Vec<u8>) -> Result<T, String> {
    let request = http::Request::post(path)
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("grpc-encoding", "gzip")
        .body(tonic::body::Body::new(http_body_util::Full::new(bytes::Bytes::from(body))))
        .map_err(|error| error.to_string())?;
    std::future::poll_fn(|cx| channel.poll_ready(cx)).await.map_err(|error| error.to_string())?;
    let response = channel.call(request).await.map_err(|error| format!("the hand-written call failed: {error}"))?;
    if let Some(status) = Status::from_header_map(response.headers()).filter(|status| status.code() != Code::Ok) {
        return Err(failed("the hand-written call", status));
    }
    let (parts, body) = response.into_parts();
    let mut messages = tonic::codec::Streaming::<T>::new_response(Decode(PhantomData), body, parts.status, None, None);
    let reply = messages.message().await.map_err(|status| failed("the hand-written call", status))?;
    let trailing = messages.trailers().await.map_err(|status| failed("the hand-written call", status))?;
    let status = trailing.as_ref().and_then(|trailers| trailers.get("grpc-status")).and_then(|value| value.to_str().ok());
    match (reply, status) {
        (Some(reply), Some("0")) => Ok(reply),
        (reply, status) => Err(format!("the hand-written call ended with grpc-status {status:?} and {} message", if reply.is_some() { "a" } else { "no" })),
    }
}

/// Each reply message decoded as `T`.
struct Decode<T>(PhantomData<fn() -> T>);

impl<T: prost::Message + Default> Decoder for Decode<T> {
    type Item = T;
    type Error = Status;

    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> Result<Option<T>, Status> {
        T::decode(buf).map(Some).map_err(|error| Status::internal(format!("the reply message does not decode: {error}")))
    }
}

/// The channel with each reply body's message flags recorded in `flags`.
#[derive(Clone)]
struct Recorded {
    channel: Channel,
    flags: Flags,
}

impl Service<http::Request<tonic::body::Body>> for Recorded {
    type Response = http::Response<Tap<tonic::body::Body>>;
    type Error = tonic::transport::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.channel.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<tonic::body::Body>) -> Self::Future {
        let flags = self.flags.clone();
        let response = self.channel.call(request);
        Box::pin(async move { Ok(response.await?.map(|body| Tap::new(body, flags))) })
    }
}
