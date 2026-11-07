//! Scenarios: disconnect, mid-stream and before the response's head.

use std::time::Duration;

use ulo::CancelReason;
use ulo_http::embed::Disconnect;

use super::drain::h2_connect;
use crate::app::IDLE;
use crate::wire::{PATIENCE, Running, start, within};
use crate::{Host, Mode};

/// How long after the client leaves a host declaring `AtClose` has to observe it, well inside the
/// stream's idle period.
const AT_CLOSE: Duration = Duration::from_millis(300);

/// How long a host declaring `AtNextWrite` has, the stream writing again after its idle period.
const AT_NEXT_WRITE: Duration = Duration::from_secs(3);

/// A disconnect mid-stream firing `Disconnected` at the declared `Disconnect` moment. The stream
/// writes one event, idles for `IDLE`, then writes every 100 ms. The client leaves after the
/// first event: a host declaring `AtClose` must observe it during the idle period, and one
/// declaring `AtNextWrite` must not, but must once the stream writes again.
pub async fn mid_stream<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = host.raw().await;
    let request = format!("GET {} HTTP/1.1\r\nHost: suite\r\nAccept: text/event-stream\r\n\r\n", host.target("/endless"));
    raw.write(request.as_bytes()).await;
    raw.read_until(b"data: start", "the stream's first event").await.expect("the stream's first event arrives");
    drop(raw);

    host.timer().sleep(AT_CLOSE).await;
    let early = host.probe.disconnected();
    match H::limits().disconnect {
        Disconnect::AtClose => {
            assert!(early, "the host declares `AtClose` and did not observe the disconnect while the stream was idle");
        }
        Disconnect::AtNextWrite => {
            assert!(!early, "the host declares `AtNextWrite` and observed the disconnect before the next write: declare `AtClose`");
            assert!(AT_CLOSE < IDLE);
            let deadline = host.timer().now() + AT_NEXT_WRITE;
            while !host.probe.disconnected() {
                assert!(
                    host.timer().now() < deadline,
                    "the host declares `AtNextWrite` and did not observe the disconnect after the stream wrote again"
                );
                host.timer().sleep(Duration::from_millis(50)).await;
            }
        }
    }
    host.stop().await;
}

/// A client closing its HTTP/1.1 connection while `GET /held` waits, before any of the response
/// is produced: on a host declaring `pending_close` the call is dropped with the execution
/// cancelled `Disconnected`.
pub async fn before_head<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let mut raw = host.raw().await;
    let request = format!("GET {} HTTP/1.1\r\nHost: suite\r\n\r\n", host.target("/held"));
    raw.write(request.as_bytes()).await;
    entered(&host).await;
    drop(raw);
    left_before_head(&host, H::limits().pending_close(), "pending_close", "closed its connection").await;
    host.stop().await;
}

/// An HTTP/2 client resetting its stream with `CANCEL` while `GET /held` waits, the connection
/// left open: on a host declaring `pending_reset` the call is dropped with the execution
/// cancelled `Disconnected`.
pub async fn reset_before_head<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let send = h2_connect(&host).await;
    let mut send = within(host.timer(), PATIENCE, send.ready())
        .await
        .expect("the connection is ready within the patience")
        .expect("the connection is ready");
    let request = http::Request::get(host.url("/held")).body(()).expect("a request");
    let (response, mut stream) = send.send_request(request, true).expect("the stream opens");
    entered(&host).await;
    stream.send_reset(h2::Reason::CANCEL);
    left_before_head(&host, H::limits().pending_reset(), "pending_reset", "reset its stream").await;
    drop((response, send));
    host.stop().await;
}

/// Waits for `GET /held` to have entered its handler, so the client leaves a call in progress.
async fn entered<H: Host>(host: &Running<H>) {
    let entered = within(host.timer(), PATIENCE, async {
        while host.probe.held_entered() == 0 {
            host.timer().sleep(Duration::from_millis(10)).await;
        }
    });
    entered.await.unwrap_or_else(|| panic!("`GET /held` did not reach its handler within {PATIENCE:?}"));
}

/// What a host does with the held call once the client `left`: dropped with `Disconnected` within
/// the patience where it declares `ability` (`declared`); where it does not, still held after
/// `AT_CLOSE`, then released.
async fn left_before_head<H: Host>(host: &Running<H>, declared: bool, ability: &str, left: &str) {
    if declared {
        let dropped = within(host.timer(), PATIENCE, async {
            loop {
                if let Some(reason) = host.probe.held_dropped().first() {
                    return *reason;
                }
                host.timer().sleep(Duration::from_millis(10)).await;
            }
        });
        let reason = dropped.await.unwrap_or_else(|| {
            panic!("the host declares `{ability}` and did not drop the call within {PATIENCE:?} after the client {left}")
        });
        assert_eq!(reason, Some(CancelReason::Disconnected), "the client {left} before the response's head");
    } else {
        host.timer().sleep(AT_CLOSE).await;
        let dropped = host.probe.held_dropped();
        assert!(
            dropped.is_empty(),
            "the host declares no `{ability}` and dropped the call after the client {left}, with {dropped:?}: declare it"
        );
    }
    host.probe.release_held();
}
