//! Scenarios: deadlines.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use ulo::{AppHandle, Bound, BoundAddr, BoxError, CancelReason, Runtime};
use ulo_rpc::{Capabilities, Frame, Link, Outbound, Pattern};
use ulo_rpc::link::Inbound;
use ulo_transport::{Count, ErrorKind};

use crate::Broker;
use crate::cases::app::{
    Fixture, Mounts, PAUSE, STALL, TICKS, addresses, client_over, eventually, failed, refused_at_startup, streams, within,
};

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

/// A stream's idle timeout bounds each silence and not the whole call: a stream read with a
/// one-second idle timeout, its ticks 20 ms apart, keeps yielding for two and a half seconds.
pub async fn stream_idle_timeout_is_per_frame<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let mut ticks = fixture.rpc().stream::<_, u64>(TICKS, &()).idle_timeout(Duration::from_secs(1)).await.expect("the stream opens");
    let opened = Instant::now();
    while opened.elapsed() < Duration::from_millis(2500) {
        let tick = within::<B, _>(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields");
        if let Err(error) = tick {
            panic!("the stream failed {:?} after it opened, its idle timeout one second: {error:?}", opened.elapsed());
        }
    }
    drop(ticks);
    fixture.stop().await;
}

/// The silence the scenarios below hold a stream in, between its two items.
const SILENCE: Duration = Duration::from_secs(4);

/// A stream given a one-second idle timeout, its server silent for four seconds after the first
/// item, ends with `Timeout` once the second has passed and before the silence ends.
pub async fn idle_timeout_ends_a_silent_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let idle = Duration::from_secs(1);
    let silence = SILENCE.as_millis() as u64;
    let mut replies = fixture.rpc().stream::<_, u64>(PAUSE, &silence).idle_timeout(idle).await.expect("the stream opens");
    let first = within::<B, _>(Duration::from_secs(10), "the first item", replies.next()).await.expect("the stream yields");
    assert_eq!(first.expect("the first item arrives"), 0);
    let silent = Instant::now();
    let ended = within::<B, _>(Duration::from_secs(10), "the idle timeout", replies.next()).await.expect("the stream yields its end");
    let waited = silent.elapsed();
    failed(ended, ErrorKind::Timeout);
    assert!(waited >= idle - Duration::from_millis(50), "the stream ended {waited:?} into the silence, before its idle timeout");
    assert!(waited < SILENCE, "the stream ended {waited:?} into the silence, not at its one-second idle timeout");
    assert!(replies.next().await.is_none(), "the stream yielded after its idle timeout");
    fixture.stop().await;
}

/// A stream given a 600 ms deadline carries it as `deadline-ms`, and the server ends the stream
/// there. The stream goes through a client link that never sends a `cancel`, so the `cancel` the
/// client sends when its own deadline passes does not reach the server.
pub async fn stream_deadline_is_the_server_s<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let deadline = Duration::from_millis(600);
    let link = MutedCancel(fixture.broker.client_link(&addresses(&[&fixture.server])));
    let client = client_over::<B, _>(link).await;
    let started = Instant::now();
    let mut ticks = client.rpc.stream::<_, u64>(TICKS, &()).deadline(deadline).await.expect("the stream opens");
    let mut arrived = 0;
    let ended = loop {
        match within::<B, _>(Duration::from_secs(10), "the stream's deadline", ticks.next()).await {
            Some(Ok(_)) => arrived += 1,
            Some(Err(error)) => break error,
            None => panic!("the stream ended cleanly {:?} after it opened, its deadline 600 ms", started.elapsed()),
        }
    };
    assert_eq!(ended.kind(), ErrorKind::Timeout, "what ended the stream: {ended:?}");
    assert!(arrived > 0, "no tick arrived before the stream's deadline");
    drop(ticks);
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(
        eventually::<B>(deadline + settle, || probe.stopped()).await,
        "the ticking stream was still running {:?} after a stream with a 600 ms deadline opened",
        started.elapsed()
    );
    assert_eq!(probe.cancelled(), Some(Some(CancelReason::Deadline)), "what ended the ticking stream");
    client.close().await;
    fixture.stop().await;
}

/// A stream given neither an idle timeout nor a deadline is bounded by neither, the client's
/// timeout included: on a client whose timeout is 300 ms, a stream silent for four seconds
/// between its two items yields both and ends cleanly.
pub async fn stream_with_neither_bound_runs_on<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let client = fixture.rpc().clone().timeout(Bound::After(Duration::from_millis(300))).expect("a nonzero timeout");
    let started = Instant::now();
    let silence = SILENCE.as_millis() as u64;
    let replies = client.stream::<_, u64>(PAUSE, &silence).await.expect("the stream opens");
    let items: Vec<_> = within::<B, _>(SILENCE + Duration::from_secs(10), "the paused stream", replies.collect()).await;
    let items: Vec<u64> = items
        .into_iter()
        .map(|item| item.unwrap_or_else(|error| panic!("the stream failed {:?} after it opened: {error:?}", started.elapsed())))
        .collect();
    assert_eq!(items, vec![0, 1], "the paused stream's items");
    assert!(started.elapsed() >= SILENCE, "the stream ended {:?} after it opened, before its silence", started.elapsed());
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

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let Outbound { send, replies } = self.0.connect(runtime).await?;
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
