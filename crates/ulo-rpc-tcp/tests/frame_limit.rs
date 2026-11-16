//! The TCP link's frame limit, 1 MiB unset, on both sides of a call: the client refuses a request
//! frame past its link's limit before writing it, `BadRequest` with `payload_too_large`, and a
//! server whose link keeps the default reads a larger frame's length prefix and its leading bytes
//! alone. Where those bytes name the call, `t` and `id` ahead of the payload as `RpcClient`
//! writes them, the server answers the call `BadRequest` with `payload_too_large` and then closes
//! the connection; where they do not, the payload written first, it closes the connection alone.
//! `max_frame` set on both links carries the larger payload.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use ulo::{App, AppHandle, Bound, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_rpc::{Codec, Frame, Inbound, RpcClient};
use ulo_rpc_tcp::Tcp;
use ulo_transport::{CallError, Detail, ErrorKind};

const MIB: usize = 1024 * 1024;

/// What a frame carries besides its payload, with room to spare: the envelope's fields and the
/// pattern.
const ENVELOPE: usize = 1024;

const PATIENCE: Duration = Duration::from_secs(10);

#[controller]
struct Echo;

#[handlers]
impl Echo {
    #[ulo_rpc::message("frame.echo_len")]
    async fn echo_len(&self, text: ulo_rpc::Payload<String>) -> Result<usize, CallError> {
        Ok(text.0.len())
    }

    #[ulo_rpc::message("frame.total_len")]
    async fn total_len(&self, items: Inbound<String>) -> Result<usize, CallError> {
        let mut items = items;
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item?.len();
        }
        Ok(total)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Echo>();
    }
}

struct Running {
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
}

impl Running {
    async fn start(link: Tcp) -> Running {
        let server = App::builder(Root)
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .expect("the server app wires")
            .connect()
            .await
            .expect("the server app connects")
            .bind(ulo_rpc::Server::new(link))
            .listen()
            .await
            .expect("the server listens");
        let addr = match server.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = server.handle();
        let serving = tokio::spawn(async move {
            let _ = server.serve(std::future::pending::<Signal>()).await;
        });
        Running { handle, serving, addr }
    }

    fn client(&self, link: impl FnOnce(Tcp) -> Tcp) -> RpcClient {
        RpcClient::new(link(Tcp::new(self.addr)), Arc::new(ulo_runtime_tokio::Tokio::current()))
            .expect("the link is usable")
            .timeout(Bound::After(PATIENCE))
            .expect("a nonzero timeout")
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

fn loopback() -> Tcp {
    Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}

async fn echo_len(client: &RpcClient, bytes: usize) -> Result<usize, ulo_rpc::RpcError> {
    client.request::<_, usize>("frame.echo_len", &"x".repeat(bytes)).await
}

fn payload_too_large(error: &ulo_rpc::RpcError) -> bool {
    (error.kind(), error.reason()) == (ErrorKind::BadRequest, Some("payload_too_large"))
}

#[tokio::test]
async fn a_request_frame_past_one_mib_is_refused_by_the_client_and_one_under_it_is_answered() {
    let running = Running::start(loopback()).await;
    let client = running.client(|link| link);
    let under = MIB - ENVELOPE;
    assert_eq!(echo_len(&client, under).await.expect("a frame under 1 MiB is carried"), under);
    let refused = echo_len(&client, MIB).await.expect_err("a frame past 1 MiB was carried");
    assert!(payload_too_large(&refused), "{refused:?}");
    running.stop().await;
}

/// `RpcClient` writes `t` and `id` first, so a server at the default answers its call over 1 MiB,
/// on JSON and on CBOR, and the client's next call connects again. Planted break: the server
/// closing the connection alone.
#[tokio::test]
async fn a_server_at_the_default_answers_a_request_frame_past_one_mib_bad_request() {
    for codec in [Codec::Json, Codec::Cbor] {
        let running = Running::start(loopback().codec(codec)).await;
        let client = running.client(|link| link.codec(codec).max_frame(4 * MIB as u64));
        let refused = echo_len(&client, 2 * MIB).await.expect_err("the server read a frame past 1 MiB");
        assert!(payload_too_large(&refused), "{codec:?}: {refused:?}");
        assert_eq!(echo_len(&client, 16).await.expect("the next call connects again"), 16, "{codec:?}");
        running.stop().await;
    }
}

/// A streamed request's item over the server's limit names a call in flight, which is answered.
/// Planted break: an `in` frame closing the connection alone.
#[tokio::test]
async fn a_server_at_the_default_answers_a_streamed_request_item_past_one_mib_bad_request() {
    let running = Running::start(loopback()).await;
    let client = running.client(|link| link.max_frame(4 * MIB as u64));
    let items = stream::iter(vec!["x".repeat(16), "x".repeat(2 * MIB)]);
    let refused = client.send_stream::<_, usize>("frame.total_len", items).await.expect_err("the server read an item past 1 MiB");
    assert!(payload_too_large(&refused), "{refused:?}");
    running.stop().await;
}

#[tokio::test]
async fn max_frame_on_both_links_carries_a_larger_payload() {
    let running = Running::start(loopback().max_frame(4 * MIB as u64)).await;
    let client = running.client(|link| link.max_frame(4 * MIB as u64));
    assert_eq!(echo_len(&client, 2 * MIB).await.expect("both links allow 4 MiB"), 2 * MIB);
    running.stop().await;
}

/// How many times over the caller's granted socket buffers it writes past its frame: more than
/// the two sockets' buffers hold, so those writes complete only while the server reads them.
const OVER_BUFFERS: usize = 16;

/// What a caller that is not `RpcClient` saw: the frames the server wrote back until it ended
/// the connection, and how the caller's writes ended.
struct Seen {
    frames: Vec<Frame>,
    wrote: io::Result<()>,
}

/// `frame` written behind its length prefix over a raw socket, then `past` times the socket's
/// buffers of bytes past it, the socket held open until the server ends the connection.
async fn raw(addr: SocketAddr, frame: String, past: usize) -> Seen {
    let stream = TcpStream::connect(addr).await.expect("the caller connects");
    let socket = socket2::SockRef::from(&stream);
    let buffered = socket.send_buffer_size().expect("the caller's SO_SNDBUF") + socket.recv_buffer_size().expect("the caller's SO_RCVBUF");
    let (mut read, mut write) = stream.into_split();
    let writing = tokio::spawn(async move {
        let wrote = async {
            write.write_all(&(frame.len() as u32).to_be_bytes()).await?;
            write.write_all(frame.as_bytes()).await?;
            write.write_all(&vec![b' '; past * buffered]).await
        }
        .await;
        (wrote, write)
    });
    let mut answered = Vec::new();
    // A reset after the server's last byte ends the read as its FIN does, the bytes kept.
    let read = tokio::time::timeout(PATIENCE, read.read_to_end(&mut answered)).await;
    assert!(read.is_ok(), "the server did not end the connection within {PATIENCE:?}");
    let (wrote, _open) = tokio::time::timeout(PATIENCE, writing)
        .await
        .expect("the caller's writes did not end within the patience")
        .expect("the writing task ran");
    let mut frames = Vec::new();
    let mut rest = answered.as_slice();
    while let Some((prefix, after)) = rest.split_first_chunk::<4>() {
        let (frame, after) = after.split_at(u32::from_be_bytes(*prefix) as usize);
        frames.push(Codec::Json.decode_frame(frame).expect("the server's frame decodes"));
        rest = after;
    }
    assert!(rest.is_empty(), "the server's bytes end inside a frame: {rest:?}");
    Seen { frames, wrote }
}

fn is_payload_too_large(frame: &Frame, call: u64) -> bool {
    let Frame::Err { id, error } = frame else { return false };
    let reason = error.details.iter().any(|detail| matches!(detail, Detail::ErrorInfo { reason, .. } if reason == "payload_too_large"));
    *id == call && error.kind == ErrorKind::BadRequest && reason
}

/// Fields in an order of the caller's own, `id` ahead of the payload and `t` after it: the
/// server answers that `id` `bad_request` with `payload_too_large` and then ends the connection,
/// reading and discarding what the caller still writes, so the caller's writes past the frame
/// complete and no reset can discard the answer. Planted breaks: the server closing the
/// connection alone; the scan reading `t` only as the first entry; the connection's reader
/// dropped rather than lingering.
#[tokio::test]
async fn a_frame_whose_id_precedes_its_payload_is_answered_then_closed() {
    let running = Running::start(loopback()).await;
    let frame = format!(r#"{{"id":7,"p":"frame.echo_len","h":{{}},"t":"req","d":"{}"}}"#, "x".repeat(2 * MIB));
    let seen = raw(running.addr, frame, OVER_BUFFERS).await;
    assert!(matches!(seen.frames.as_slice(), [frame] if is_payload_too_large(frame, 7)), "{:?}", seen.frames);
    assert!(seen.wrote.is_ok(), "the caller's writes past the answered frame failed: {:?}", seen.wrote);
    running.stop().await;
}

/// A frame over a limit, shorter than the 4 KiB of leading bytes the server reads for its call, is
/// read to its own end and no further: its `id` is read behind a payload that ends within those
/// bytes, and a frame naming no call ends the connection rather than waiting for bytes past it.
/// The caller writes nothing past either frame. Planted break: the server waiting for 4 KiB
/// whatever the frame's length.
#[tokio::test]
async fn a_frame_shorter_than_the_leading_bytes_read_is_read_to_its_end() {
    let running = Running::start(loopback().max_frame(1024)).await;
    let payload = "x".repeat(2048);
    let seen = raw(running.addr, format!(r#"{{"t":"req","d":"{payload}","p":"frame.echo_len","id":3}}"#), 0).await;
    assert!(matches!(seen.frames.as_slice(), [frame] if is_payload_too_large(frame, 3)), "{:?}", seen.frames);
    let seen = raw(running.addr, format!(r#"{{"t":"evt","p":"frame.echo_len","d":"{payload}"}}"#), 0).await;
    assert!(seen.frames.is_empty(), "{:?}", seen.frames);
    running.stop().await;
}

/// A frame whose leading bytes name no call the server can answer ends the connection with
/// nothing written: the payload written ahead of `id`, an item of a streamed request the
/// connection never opened, and an event, which has no `id`. Planted break: the server answering
/// an `in` frame whose call is not in flight.
#[tokio::test]
async fn a_frame_whose_leading_bytes_name_no_call_closes_the_connection_alone() {
    let running = Running::start(loopback()).await;
    let payload = "x".repeat(2 * MIB);
    for frame in [
        format!(r#"{{"t":"req","d":"{payload}","p":"frame.echo_len","id":7}}"#),
        format!(r#"{{"t":"in","id":5,"d":"{payload}"}}"#),
        format!(r#"{{"t":"evt","p":"frame.echo_len","d":"{payload}"}}"#),
    ] {
        let start: String = frame.chars().take(32).collect();
        let seen = raw(running.addr, frame, OVER_BUFFERS).await;
        assert!(seen.frames.is_empty(), "{start}: {:?}", seen.frames);
    }
    running.stop().await;
}
