//! Scenarios: deadlines.

use std::future::Future;
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use ulo::{AppHandle, BoundAddr, BoxError, CancelReason};
use ulo_rpc::{Capabilities, Frame, Link, Outbound, Pattern};
use ulo_rpc::link::Inbound;
use ulo_transport::{Count, ErrorKind};

use crate::Broker;
use crate::cases::app::{Fixture, Mounts, STALL, TICKS, addresses, client_over, eventually, failed, refused_at_startup, streams, within};

/// `deadline-ms` fires `Deadline` and renders `timeout`.
pub async fn deadline_ms<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let outcome = fixture.rpc().request::<_, ()>(STALL, &()).header("deadline-ms", "300").timeout(Duration::from_secs(10)).await;
    failed(outcome, ErrorKind::Timeout);
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(eventually::<B>(settle, || probe.stalled().is_some()).await, "the stalled handler was not ended");
    assert_eq!(probe.stalled(), Some(Some(CancelReason::Deadline)));
    fixture.stop().await;
}

/// The client's own timeout, as `Timeout`.
pub async fn client_timeout<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let started = Instant::now();
    let outcome = fixture.rpc().request::<_, ()>(STALL, &()).timeout(Duration::from_millis(300)).await;
    failed(outcome, ErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3), "the client's timeout took {:?}", started.elapsed());
    fixture.stop().await;
}

/// A call given a 300 ms timeout carries it as its deadline, and the server ends the call there.
/// The call goes through a client link that never sends a `cancel`, so the timeout's `cancel`
/// does not reach the server and only the deadline the call carried can end it.
pub async fn timeout_is_the_server_s_deadline<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let link = MutedCancel(fixture.broker.client_link(&addresses(&[&fixture.server])));
    let client = client_over::<B, _>(link).await;
    let started = Instant::now();
    let outcome = client.rpc.request::<_, ()>(STALL, &()).timeout(Duration::from_millis(300)).await;
    failed(outcome, ErrorKind::Timeout);
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(
        eventually::<B>(Duration::from_millis(300) + settle, || probe.stalled().is_some()).await,
        "the stalled handler was still running {:?} after a call with a 300 ms timeout began",
        started.elapsed()
    );
    assert_eq!(probe.stalled(), Some(Some(CancelReason::Deadline)), "what ended the stalled handler");
    client.close().await;
    fixture.stop().await;
}

/// A stream's timeout bounds the wait for each frame and is not the call's deadline: a stream read
/// with a one-second timeout, its ticks 20 ms apart, keeps yielding for two and a half seconds.
pub async fn stream_timeout_is_per_frame<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let mut ticks = fixture.rpc().stream::<_, u64>(TICKS, &()).timeout(Duration::from_secs(1)).await.expect("the stream opens");
    let opened = Instant::now();
    while opened.elapsed() < Duration::from_millis(2500) {
        let tick = within::<B, _>(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields");
        if let Err(error) = tick {
            panic!("the stream failed {:?} after it opened, its timeout one second per frame: {error:?}", opened.elapsed());
        }
    }
    drop(ticks);
    fixture.stop().await;
}

/// A client link that sends every frame but a `cancel`.
struct MutedCancel<L>(L);

impl<L: Link> Link for MutedCancel<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.0.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.0.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.0.max_inflight(calls);
    }

    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        self.0.listen(patterns)
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let Outbound { send, replies } = self.0.connect().await?;
        Ok(Outbound {
            send: Box::new(move |pattern, frame, reply_to| match frame {
                Frame::Cancel { .. } => Box::pin(async { Ok(()) }),
                frame => send(pattern, frame, reply_to),
            }),
            replies,
        })
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.0.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.0.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.0.bound()
    }
}
