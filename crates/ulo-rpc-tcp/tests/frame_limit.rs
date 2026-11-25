//! The TCP link's frame limit, 1 MiB unset, on both sides of a call: the client refuses a request
//! frame past its link's limit before writing it, `BadRequest` with `payload_too_large`, and a
//! server whose link keeps the default reads a larger frame's length prefix and its leading bytes
//! alone, then reads the rest of the frame's declared length and drops it. Where those bytes name
//! the call, `t` and `id` ahead of the payload as `RpcClient` writes them, the server answers the
//! call `BadRequest` with `payload_too_large`; where they do not, the payload written first, it
//! answers nothing. Either way the connection reads the next frame. `max_frame` set on both links
//! carries the larger payload.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

    /// `total_len` for the one test that reads [`HELD_ENDED`]: the count rises once the call's
    /// future is dropped, finished or cancelled.
    #[ulo_rpc::message("frame.held_total_len")]
    async fn held_total_len(&self, items: Inbound<String>) -> Result<usize, CallError> {
        let _ended = Ended;
        let mut items = items;
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item?.len();
        }
        Ok(total)
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

/// How many `held_total_len` calls have ended.
static HELD_ENDED: AtomicUsize = AtomicUsize::new(0);

/// Counts into [`HELD_ENDED`] as it drops.
struct Ended;

impl Drop for Ended {
    fn drop(&mut self) {
        HELD_ENDED.fetch_add(1, Ordering::SeqCst);
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
/// on JSON and on CBOR, and the client's next call is answered. Planted break: the server closing
/// the connection alone.
#[tokio::test]
async fn a_server_at_the_default_answers_a_request_frame_past_one_mib_bad_request() {
    for codec in [Codec::Json, Codec::Cbor] {
        let running = Running::start(loopback().codec(codec)).await;
        let client = running.client(|link| link.codec(codec).max_frame(4 * MIB as u64));
        let refused = echo_len(&client, 2 * MIB).await.expect_err("the server read a frame past 1 MiB");
        assert!(payload_too_large(&refused), "{codec:?}: {refused:?}");
        assert_eq!(echo_len(&client, 16).await.expect("the next call is answered"), 16, "{codec:?}");
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

/// `frames` written behind their length prefixes over one raw socket, and the frames the server
/// writes back read until `answers` have arrived; the socket then closes. The connection carrying
/// every frame after an over-limit one is what a test reads off the answers.
async fn exchange(addr: SocketAddr, frames: &[String], answers: usize) -> Vec<Frame> {
    let mut stream = TcpStream::connect(addr).await.expect("the caller connects");
    for frame in frames {
        stream.write_all(&(frame.len() as u32).to_be_bytes()).await.expect("the caller writes a prefix");
        stream.write_all(frame.as_bytes()).await.expect("the caller writes a frame");
    }
    let mut answered = Vec::new();
    let read = tokio::time::timeout(PATIENCE, async {
        while answered.len() < answers {
            let mut prefix = [0u8; 4];
            stream.read_exact(&mut prefix).await?;
            let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut frame).await?;
            answered.push(Codec::Json.decode_frame(&frame).expect("the server's frame decodes"));
        }
        io::Result::Ok(())
    })
    .await;
    match read {
        Ok(Ok(())) => answered,
        Ok(Err(error)) => panic!("the connection ended after {} of {answers} answers ({error}): {answered:?}", answered.len()),
        Err(_) => panic!("{} of {answers} answers within {PATIENCE:?}: {answered:?}", answered.len()),
    }
}

/// A request the server answers, `id` and the length of its text: the frame after each over-limit
/// one, whose answer shows the connection carried it.
fn echo(id: u64) -> String {
    format!(r#"{{"t":"req","id":{id},"p":"frame.echo_len","h":{{}},"d":"abc"}}"#)
}

fn is_payload_too_large(frame: &Frame, call: u64) -> bool {
    let Frame::Err { id, error, .. } = frame else { return false };
    let reason = error.details.iter().any(|detail| matches!(detail, Detail::ErrorInfo { reason, .. } if reason == "payload_too_large"));
    *id == call && error.kind == ErrorKind::BadRequest && reason
}

fn is_echo(frame: &Frame, call: u64) -> bool {
    matches!(frame, Frame::Res { id, data, .. } if *id == call && data.as_bytes() == b"3")
}

/// Fields in an order of the caller's own, `id` ahead of the payload and `t` after it: the
/// server answers that `id` `bad_request` with `payload_too_large`, reads the rest of the frame's
/// declared length, and answers the request after it on the same connection. Planted breaks: the
/// server closing the connection after the answer; the scan reading `t` only as the first entry;
/// the bytes past the leading ones left unread.
#[tokio::test]
async fn a_frame_whose_id_precedes_its_payload_is_answered_and_the_connection_carries_the_next() {
    let running = Running::start(loopback()).await;
    let over = format!(r#"{{"id":7,"p":"frame.echo_len","h":{{}},"t":"req","d":"{}"}}"#, "x".repeat(2 * MIB));
    let answered = exchange(running.addr, &[over, echo(8)], 2).await;
    assert!(matches!(answered.as_slice(), [first, second] if is_payload_too_large(first, 7) && is_echo(second, 8)), "{answered:?}");
    running.stop().await;
}

/// A frame over a limit, shorter than the 4 KiB of leading bytes the server scans for its call, is
/// read to its own end and no further: its `id` is read behind a payload that ends within those
/// bytes, and a frame naming no call is dropped, the next frame read in each case. Planted break:
/// the server waiting for 4 KiB whatever the frame's length.
#[tokio::test]
async fn a_frame_shorter_than_the_leading_bytes_read_is_read_to_its_end() {
    let running = Running::start(loopback().max_frame(1024)).await;
    let payload = "x".repeat(2048);
    let answered = exchange(running.addr, &[format!(r#"{{"t":"req","d":"{payload}","p":"frame.echo_len","id":3}}"#), echo(4)], 2).await;
    assert!(matches!(answered.as_slice(), [first, second] if is_payload_too_large(first, 3) && is_echo(second, 4)), "{answered:?}");
    let answered = exchange(running.addr, &[format!(r#"{{"t":"evt","p":"frame.echo_len","d":"{payload}"}}"#), echo(5)], 1).await;
    assert!(matches!(answered.as_slice(), [only] if is_echo(only, 5)), "{answered:?}");
    running.stop().await;
}

/// A frame whose leading bytes name no call the server can answer is dropped with nothing written,
/// and the frame after it answered: the payload written ahead of `id`, an item of a streamed
/// request the connection never opened, and an event, which has no `id`. Planted break: the
/// server answering an `in` frame whose call is not in flight.
#[tokio::test]
async fn a_frame_whose_leading_bytes_name_no_call_is_dropped_and_the_next_answered() {
    let running = Running::start(loopback()).await;
    let payload = "x".repeat(2 * MIB);
    for frame in [
        format!(r#"{{"t":"req","d":"{payload}","p":"frame.echo_len","id":7}}"#),
        format!(r#"{{"t":"in","id":5,"d":"{payload}"}}"#),
        format!(r#"{{"t":"evt","p":"frame.echo_len","d":"{payload}"}}"#),
    ] {
        let start: String = frame.chars().take(32).collect();
        let answered = exchange(running.addr, &[frame, echo(9)], 1).await;
        assert!(matches!(answered.as_slice(), [only] if is_echo(only, 9)), "{start}: {answered:?}");
    }
    running.stop().await;
}

/// An item over the limit for a streamed request in flight is answered under the call's `id`, and
/// the call ends there: its handler is cancelled, nothing more is written for it, and the request
/// after the item is answered on the same connection. Planted breaks: the call left running, its
/// `in_end` dropped, which leaves its handler waiting; the call kept mapped, whose handler answers
/// its `in_end` with the total of its items.
#[tokio::test]
async fn an_over_limit_item_ends_its_call_and_the_connection_carries_the_next() {
    let running = Running::start(loopback()).await;
    let frames = [
        r#"{"t":"open","id":5,"p":"frame.held_total_len","h":{}}"#.to_owned(),
        r#"{"t":"in","id":5,"d":"abcd"}"#.to_owned(),
        format!(r#"{{"t":"in","id":5,"d":"{}"}}"#, "x".repeat(2 * MIB)),
        r#"{"t":"in_end","id":5}"#.to_owned(),
        echo(6),
    ];
    let answered = exchange(running.addr, &frames, 2).await;
    assert!(matches!(answered.as_slice(), [first, second] if is_payload_too_large(first, 5) && is_echo(second, 6)), "{answered:?}");
    let ended = tokio::time::timeout(PATIENCE, async {
        while HELD_ENDED.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    assert!(ended.await.is_ok(), "the call whose item was over the limit was still running {PATIENCE:?} after its answer");
    running.stop().await;
}
