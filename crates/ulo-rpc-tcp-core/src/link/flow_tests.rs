//! A server connection's bounds, run on an executor the test steps by hand and a scripted clock,
//! so "nothing more happens" is a fact the executor reports rather than a wait: `run` polls until
//! no task is woken. The peer's halves are scripted: a reader the test feeds or that floods, and
//! a writer that records or never takes a byte.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use futures_channel::mpsc as channel;
use futures_util::StreamExt;
use ulo::testing::ScriptedTimer;
use ulo::{BoxFuture, Refused, RuntimeTask, Spawn, TaskHandle, Timer};
use ulo_rpc::{Codec, Data, ErrorBody, Frame};
use ulo_transport::{Details, ErrorKind};

use super::*;

/// Runs spawned tasks when [`run`](Manual::run) is called, until none is woken.
#[derive(Clone)]
struct Manual {
    ready: Arc<Mutex<VecDeque<Arc<Job>>>>,
    clock: ScriptedTimer,
}

struct Job {
    fut: Mutex<Option<BoxFuture<'static, ()>>>,
    queued: AtomicBool,
    done: AtomicBool,
    ended: Mutex<Option<Waker>>,
    ready: Weak<Mutex<VecDeque<Arc<Job>>>>,
}

impl Wake for Job {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.queued.swap(true, AtomicOrdering::SeqCst)
            && let Some(ready) = self.ready.upgrade()
        {
            lock(&ready).push_back(Arc::clone(self));
        }
    }
}

impl Job {
    fn finish(&self) {
        self.done.store(true, AtomicOrdering::SeqCst);
        if let Some(waker) = lock(&self.ended).take() {
            waker.wake();
        }
    }
}

impl Manual {
    fn new() -> Manual {
        Manual { ready: Arc::new(Mutex::new(VecDeque::new())), clock: ScriptedTimer::new() }
    }

    /// Polls every woken task until none is.
    fn run(&self) {
        loop {
            let Some(job) = lock(&self.ready).pop_front() else { return };
            job.queued.store(false, AtomicOrdering::SeqCst);
            let waker = Waker::from(Arc::clone(&job));
            let finished = {
                let mut slot = lock(&job.fut);
                match slot.as_mut() {
                    Some(fut) => fut.as_mut().poll(&mut Context::from_waker(&waker)).is_ready(),
                    None => false,
                }
            };
            if finished {
                let fut = lock(&job.fut).take();
                drop(fut);
                job.finish();
            }
        }
    }

    /// Moves the clock by `by`, then runs what it woke.
    fn advance(&self, by: Duration) {
        self.clock.advance(by);
        self.run();
    }
}

struct ManualTask(Arc<Job>);

impl RuntimeTask for ManualTask {
    fn abort(&mut self) {
        let fut = lock(&self.0.fut).take();
        drop(fut);
        self.0.finish();
    }

    fn poll_ended(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.done.load(AtomicOrdering::SeqCst) {
            return Poll::Ready(());
        }
        *lock(&self.0.ended) = Some(cx.waker().clone());
        if self.0.done.load(AtomicOrdering::SeqCst) { Poll::Ready(()) } else { Poll::Pending }
    }

    fn detach(self: Box<Self>) {}
}

impl Spawn for Manual {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        TaskHandle::launch(fut, |wrapped| {
            let job = Arc::new(Job {
                fut: Mutex::new(Some(wrapped)),
                queued: AtomicBool::new(false),
                done: AtomicBool::new(false),
                ended: Mutex::new(None),
                ready: Arc::downgrade(&self.ready),
            });
            job.wake_by_ref();
            ManualTask(job)
        })
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        Ok(self.spawn(fut))
    }
}

impl Timer for Manual {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        self.clock.try_sleep(d)
    }
}

/// A peer's sending half the test feeds: what it pushes, then `Pending` until more is pushed or it
/// closes. Reads are counted.
#[derive(Clone, Default)]
struct Fed(Arc<Mutex<FedState>>);

#[derive(Default)]
struct FedState {
    bytes: VecDeque<u8>,
    closed: bool,
    waker: Option<Waker>,
    reads: usize,
    dropped: bool,
}

/// The half the server reads, which records its drop: the connection closes once both halves drop.
struct FedHalf(Fed);

impl Drop for FedHalf {
    fn drop(&mut self) {
        lock(&(self.0).0).dropped = true;
    }
}

impl AsyncRead for FedHalf {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl Fed {
    fn push(&self, bytes: &[u8]) {
        let mut state = lock(&self.0);
        state.bytes.extend(bytes);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    fn close(&self) {
        let mut state = lock(&self.0);
        state.closed = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    fn reads(&self) -> usize {
        lock(&self.0).reads
    }

    fn dropped(&self) -> bool {
        lock(&self.0).dropped
    }
}

impl AsyncRead for Fed {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut state = lock(&self.0);
        if state.bytes.is_empty() {
            if state.closed {
                return Poll::Ready(Ok(0));
            }
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let read = buf.len().min(state.bytes.len());
        for (slot, byte) in buf.iter_mut().zip(state.bytes.drain(..read)) {
            *slot = byte;
        }
        state.reads += 1;
        Poll::Ready(Ok(read))
    }
}

/// A peer that sends `left` request frames as fast as they are read, then nothing, never closing.
struct Flood {
    left: u64,
    next: u64,
    frame: Vec<u8>,
    at: usize,
    sent: Arc<AtomicU64>,
}

impl AsyncRead for Flood {
    fn poll_read(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if self.at == self.frame.len() {
            if self.left == 0 {
                return Poll::Pending;
            }
            self.left -= 1;
            let id = self.next;
            self.next += 1;
            self.frame = framed(&request(id));
            self.at = 0;
            self.sent.fetch_add(1, AtomicOrdering::SeqCst);
        }
        let read = buf.len().min(self.frame.len() - self.at);
        buf[..read].copy_from_slice(&self.frame[self.at..self.at + read]);
        self.at += read;
        Poll::Ready(Ok(read))
    }
}

/// A peer's receiving half: `Recorded` keeps every byte and whether the server shut its half;
/// `Stuck` takes no byte, as a peer that never reads once the socket's buffers are full.
#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<RecordedState>>);

#[derive(Default)]
struct RecordedState {
    bytes: Vec<u8>,
    shut: bool,
    dropped: bool,
}

struct RecordedHalf(Recorded);

impl Drop for RecordedHalf {
    fn drop(&mut self) {
        lock(&(self.0).0).dropped = true;
    }
}

impl Recorded {
    fn frames(&self) -> Vec<Frame> {
        let state = lock(&self.0);
        let mut frames = Vec::new();
        let mut at = 0;
        while at + 4 <= state.bytes.len() {
            let len = u32::from_be_bytes(state.bytes[at..at + 4].try_into().expect("four bytes")) as usize;
            frames.push(Codec::Json.decode_frame(&state.bytes[at + 4..at + 4 + len]).expect("a frame the server wrote decodes"));
            at += 4 + len;
        }
        frames
    }

    fn shut(&self) -> bool {
        lock(&self.0).shut
    }

    fn dropped(&self) -> bool {
        lock(&self.0).dropped
    }
}

impl AsyncWrite for RecordedHalf {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        lock(&(self.0).0).bytes.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        lock(&(self.0).0).shut = true;
        Poll::Ready(Ok(()))
    }
}

struct Stuck;

impl AsyncWrite for Stuck {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

/// The listener: the connections the test hands it, each counted as it is accepted.
struct Handed {
    incoming: channel::UnboundedReceiver<Incoming>,
    accepted: Arc<AtomicUsize>,
}

impl Accept for Handed {
    async fn accept(&mut self) -> io::Result<Incoming> {
        match self.incoming.next().await {
            Some(incoming) => {
                self.accepted.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(incoming)
            }
            None => pending().await,
        }
    }
}

fn framed(frame: &[u8]) -> Vec<u8> {
    let mut bytes = (frame.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(frame);
    bytes
}

fn peer() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9))
}

fn request(id: u64) -> Vec<u8> {
    Codec::Json
        .encode_frame(&Frame::Req { id, pattern: "p".into(), headers: Default::default(), data: Data::default() })
        .expect("a request encodes")
        .to_vec()
}

fn refusal() -> ErrorBody {
    ErrorBody::new(ErrorKind::Unavailable, "refused", Details::new())
}

/// How the test's server answers each request.
#[derive(Clone, Copy)]
enum Answer {
    /// From a task of its own per request, as `ulo-rpc` writes a refusal.
    Refused,
    /// Not until the test releases it.
    Held,
}

/// A prepared, listening server on `manual`, its link set up by `settings`, answering as `answer`
/// says. Answers the handle for new connections, the accept count, and the requests held.
struct Running {
    core: Core,
    connect: channel::UnboundedSender<Incoming>,
    accepted: Arc<AtomicUsize>,
    held: Arc<Mutex<Vec<(u64, ReplyPath)>>>,
}

impl Running {
    fn start(manual: &Manual, answer: Answer, settings: impl FnOnce(&mut Core)) -> Running {
        let mut core = Core::new(EndpointSpec::from(SocketAddr::from(([127, 0, 0, 1], 0))));
        settings(&mut core);
        core.prepare(Ok(()), Some(Arc::new(manual.clone()) as Arc<dyn Timer>)).expect("the link prepares");
        let (connect, incoming) = channel::unbounded();
        let accepted = Arc::new(AtomicUsize::new(0));
        let handed = Handed { incoming, accepted: Arc::clone(&accepted) };
        let mut inbound = core.listen(Arc::new(manual.clone()) as Arc<dyn Runtime>, move |_, _| Ok(handed)).expect("the link listens");
        let held = Arc::new(Mutex::new(Vec::new()));
        let holding = Arc::clone(&held);
        let spawner = manual.clone();
        drop(manual.spawn(Box::pin(async move {
            while let Some(delivery) = inbound.next().await {
                let (Frame::Req { id, .. }, Some(reply)) = (delivery.frame, delivery.reply) else { continue };
                match answer {
                    Answer::Refused => drop(spawner.spawn(Box::pin(async move {
                        let _ = reply.send(Frame::Err { id, error: refusal() }).await;
                    }))),
                    Answer::Held => lock(&holding).push((id, reply)),
                }
            }
        })));
        manual.run();
        Running { core, connect, accepted, held }
    }

    fn connect(&self, reader: impl AsyncRead + Send + 'static, writer: impl AsyncWrite + Send + 'static) {
        self.connect.unbounded_send(Incoming::plain(peer(), Box::pin(reader), Box::pin(writer))).expect("the listener takes the connection");
    }

    /// Answers every held request `res`.
    fn release(&self, manual: &Manual) {
        for (id, reply) in lock(&self.held).drain(..) {
            drop(manual.spawn(Box::pin(async move {
                let _ = reply.send(Frame::Res { id, data: Data::default() }).await;
            })));
        }
    }

    fn queued(&self) -> Vec<usize> {
        lock(&self.core.state.connections).values().map(ordered::Sender::len).collect()
    }
}

/// The 2d checker's measurement, made deterministic (Q34-48): a peer writing 400,000 requests and
/// reading nothing, each request refused from a task of its own as `ulo-rpc` refuses one. The
/// server stops reading once the connection's writer has no room, so what it read, and what waits
/// in the writer's queue, stays within the queue's bound and what was on its way to the server.
/// Planted break: the reader not waiting for room, which reads all 400,000 and queues as many
/// refusals.
#[test]
fn a_peer_that_reads_nothing_stops_being_read() {
    const SENT: u64 = 400_000;
    // The writer's queue, the deliveries channel and the server's inbound channel, each with its
    // senders' slots, and one frame read before the queue filled.
    const BOUND: u64 = (WRITE_QUEUE + 2 * (DELIVERY_QUEUE + 2) + 1) as u64;
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |_| {});
    let sent = Arc::new(AtomicU64::new(0));
    running.connect(Flood { left: SENT, next: 1, frame: Vec::new(), at: 0, sent: Arc::clone(&sent) }, Stuck);
    manual.run();
    let read = sent.load(AtomicOrdering::SeqCst);
    let queued = running.queued();
    eprintln!("the server read {read} of {SENT} requests; {queued:?} refusals wait in its writer's queue");
    assert!(read <= BOUND, "the server read {read} of {SENT} requests from a peer reading nothing, over {BOUND}");
    assert!(read > WRITE_QUEUE as u64, "the server read {read} requests, fewer than its writer's queue holds");
    assert_eq!(queued.len(), 1, "one connection is open");
    assert!(queued[0] as u64 <= BOUND, "{} refusals wait in the writer's queue, over {BOUND}", queued[0]);
}

/// Q34-43: a connection holding no call and moving no frame is sent `goaway` and closed once
/// `idle_timeout` passes, 60 s unset, and not a moment before; the peer's later bytes are read
/// and dropped until `CUT_GRACE` after the expiry, when the connection drops. Planted break: no
/// idle timer, which leaves the connection open at 60 s.
#[test]
fn an_idle_connection_is_sent_goaway_and_closed_at_the_timeout() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |_| {});
    let (fed, written) = (Fed::default(), Recorded::default());
    running.connect(FedHalf(fed.clone()), RecordedHalf(written.clone()));
    manual.run();
    assert_eq!(manual.clock.live(DEFAULT_IDLE_TIMEOUT), 1, "the connection waits on its idle timeout");
    manual.advance(DEFAULT_IDLE_TIMEOUT - Duration::from_millis(1));
    assert!(written.frames().is_empty() && !written.shut(), "the connection closed before its idle timeout");
    manual.advance(Duration::from_millis(1));
    assert_eq!(written.frames(), vec![Frame::Goaway], "an idle connection is sent `goaway`");
    assert!(written.shut(), "the server shuts its half of an idle connection");
    let reads = fed.reads();
    fed.push(&framed(&request(7)));
    manual.run();
    assert!(fed.reads() > reads, "the peer's bytes after the expiry are read");
    assert_eq!(written.frames(), vec![Frame::Goaway], "a request after the expiry is not served");
    assert!(!fed.dropped(), "the connection's reader dropped before its grace");
    assert_eq!(manual.clock.live(CUT_GRACE), 1, "the linger waits out `CUT_GRACE`");
    manual.advance(CUT_GRACE);
    assert!(fed.dropped() && written.dropped(), "the connection is dropped `CUT_GRACE` after its idle expiry");
}

/// A frame read or written moves the expiry: idleness runs from the latest one. Planted break: the
/// expiry timed from the connection's open alone, which closes it at 60 s.
#[test]
fn a_frame_moves_the_idle_expiry() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |_| {});
    let (fed, written) = (Fed::default(), Recorded::default());
    running.connect(FedHalf(fed.clone()), RecordedHalf(written.clone()));
    manual.run();
    manual.advance(Duration::from_secs(30));
    fed.push(&framed(&request(1)));
    manual.run();
    assert!(matches!(written.frames()[..], [Frame::Err { id: 1, .. }]), "the request is answered: {:?}", written.frames());
    manual.advance(Duration::from_secs(30));
    assert!(!written.shut(), "a connection that carried a frame 30 s ago closed as idle");
    assert_eq!(manual.clock.live(Duration::from_secs(30)), 1, "the idle wait is armed for what is left");
    manual.advance(Duration::from_secs(30));
    assert!(written.shut(), "the connection closes 60 s after its last frame");
}

/// A call in flight keeps its connection open however long it runs: idleness starts at its end.
/// Planted break: idleness that ignores the calls in flight, which closes the connection at 60 s
/// with the call unanswered.
#[test]
fn a_call_in_flight_is_not_idle() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Held, |_| {});
    let (fed, written) = (Fed::default(), Recorded::default());
    running.connect(FedHalf(fed.clone()), RecordedHalf(written.clone()));
    manual.run();
    fed.push(&framed(&request(3)));
    manual.run();
    assert_eq!(lock(&running.held).len(), 1, "the request reached the server");
    manual.advance(Duration::from_secs(600));
    assert!(written.frames().is_empty() && !written.shut(), "a connection holding a call closed as idle");
    running.release(&manual);
    manual.run();
    assert!(matches!(written.frames()[..], [Frame::Res { id: 3, .. }]), "the call is answered: {:?}", written.frames());
    assert_eq!(manual.clock.live(DEFAULT_IDLE_TIMEOUT), 1, "idleness is timed from the call's end");
    manual.advance(DEFAULT_IDLE_TIMEOUT);
    assert!(written.shut(), "the connection closes `idle_timeout` after its call ended");
}

/// A frame still arriving counts once it is whole. Planted break: a read that counts as activity,
/// which keeps a connection open through a frame that never ends.
#[test]
fn a_frame_still_arriving_does_not_keep_a_connection_open() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |_| {});
    let (fed, written) = (Fed::default(), Recorded::default());
    running.connect(FedHalf(fed.clone()), RecordedHalf(written.clone()));
    manual.run();
    let frame = framed(&request(4));
    for byte in &frame[..frame.len() - 1] {
        manual.advance(Duration::from_secs(5));
        fed.push(&[*byte]);
        manual.run();
        if written.shut() {
            break;
        }
    }
    assert!(written.shut(), "a frame trickling in for longer than the timeout kept the connection open");
}

/// `Bound::Unbounded` times no idleness. Planted break: `Unbounded` read as the default.
#[test]
fn an_unbounded_idle_timeout_keeps_a_connection_open() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |core| core.set_idle_timeout(Bound::Unbounded));
    let (fed, written) = (Fed::default(), Recorded::default());
    running.connect(FedHalf(fed), RecordedHalf(written.clone()));
    manual.run();
    assert_eq!(manual.clock.live(DEFAULT_IDLE_TIMEOUT), 0, "an unbounded timeout arms no wait");
    manual.advance(Duration::from_secs(86_400));
    assert!(!written.shut(), "an unbounded idle timeout closed a connection");
}

/// At `max_connections` the server accepts nothing until a connection ends. Planted break: the
/// accept loop ignoring the bound, which accepts the second connection at once.
#[test]
fn the_server_accepts_no_connection_past_max_connections() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |core| core.set_max_connections(Count::Max(1)));
    let first = Fed::default();
    running.connect(FedHalf(first.clone()), RecordedHalf(Recorded::default()));
    running.connect(FedHalf(Fed::default()), RecordedHalf(Recorded::default()));
    manual.run();
    assert_eq!(running.accepted.load(AtomicOrdering::SeqCst), 1, "the server accepted past `max_connections(1)`");
    first.close();
    manual.run();
    assert_eq!(running.accepted.load(AtomicOrdering::SeqCst), 2, "the server accepts again once a connection ends");
}

/// Q34-52: `prepare` refuses a `max_frame` below the size of the largest answer to a frame over
/// it, and takes that size itself, on both codecs; the answer at that limit encodes within it.
/// Planted break: no floor, which prepares a link whose `payload_too_large` answer cannot be sent.
#[test]
fn a_max_frame_below_its_own_answer_is_refused() {
    for codec in [Codec::Json, Codec::Cbor] {
        let floor = (1..4096).find(|&limit| largest_over_limit_answer(codec, limit) <= limit).expect("a limit carries its own answer");
        let prepared = |limit| {
            let mut core = Core::new(EndpointSpec::from(SocketAddr::from(([127, 0, 0, 1], 0))));
            core.set_codec(codec);
            core.set_max_frame(limit);
            core.prepare(Ok(()), None).map_err(|error| error.to_string())
        };
        let refused = prepared(floor - 1).expect_err("a limit below its own answer prepares");
        assert!(refused.contains(&format!("is below {floor} bytes")), "{codec:?}: {refused}");
        prepared(floor).unwrap_or_else(|error| panic!("{codec:?} at {floor}: {error}"));
        encode(codec, floor, &over_limit_answer(u64::MAX, u64::from(u32::MAX), floor)).expect("the largest answer fits the floor");
        eprintln!("{codec:?}: the smallest `max_frame` is {floor} bytes");
    }
}

/// A zero `idle_timeout` and a zero `max_connections` are refused in `prepare`, each named.
#[test]
fn zero_bounds_are_refused() {
    let mut core = Core::new(EndpointSpec::from(SocketAddr::from(([127, 0, 0, 1], 0))));
    core.set_idle_timeout(Bound::After(Duration::ZERO));
    core.set_max_connections(Count::Max(0));
    let refused = core.prepare(Ok(()), None).expect_err("zero bounds prepare").to_string();
    assert!(refused.contains("`.idle_timeout(Bound::After(Duration::ZERO))`"), "{refused}");
    assert!(refused.contains("`.max_connections(Count::Max(0))`"), "{refused}");
}

/// The room wait against a writer that fails, an idle expiry with the writer stuck, a half-close
/// behind a slow writer, a `cancel` waiting behind a full writer, and what moves the idle expiry.
mod checker {
    use super::*;

    /// A server answering each delivery through `answer`, on `manual`.
    fn start_with(manual: &Manual, settings: impl FnOnce(&mut Core), answer: impl Fn(&Manual, Delivery) + Send + 'static) -> Running {
        let mut core = Core::new(EndpointSpec::from(SocketAddr::from(([127, 0, 0, 1], 0))));
        settings(&mut core);
        core.prepare(Ok(()), Some(Arc::new(manual.clone()) as Arc<dyn Timer>)).expect("the link prepares");
        let (connect, incoming) = channel::unbounded();
        let accepted = Arc::new(AtomicUsize::new(0));
        let handed = Handed { incoming, accepted: Arc::clone(&accepted) };
        let mut inbound = core.listen(Arc::new(manual.clone()) as Arc<dyn Runtime>, move |_, _| Ok(handed)).expect("the link listens");
        let spawner = manual.clone();
        drop(manual.spawn(Box::pin(async move {
            while let Some(delivery) = inbound.next().await {
                answer(&spawner, delivery);
            }
        })));
        manual.run();
        Running { core, connect, accepted, held: Arc::new(Mutex::new(Vec::new())) }
    }

    fn refuse(manual: &Manual, delivery: Delivery) {
        let (Frame::Req { id, .. }, Some(reply)) = (delivery.frame, delivery.reply) else { return };
        drop(manual.spawn(Box::pin(async move {
            let _ = reply.send(Frame::Err { id, error: refusal() }).await;
        })));
    }

    /// Records its drop.
    struct Dropping<T>(T, Arc<AtomicBool>);

    impl<T> Drop for Dropping<T> {
        fn drop(&mut self) {
            self.1.store(true, AtomicOrdering::SeqCst);
        }
    }

    impl<T: AsyncRead + Unpin> AsyncRead for Dropping<T> {
        fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.0).poll_read(cx, buf)
        }
    }

    impl<T: AsyncWrite + Unpin> AsyncWrite for Dropping<T> {
        fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.0).poll_write(cx, buf)
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.0).poll_flush(cx)
        }
        fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.0).poll_close(cx)
        }
    }

    impl Unpin for Flood {}

    fn flood(left: u64) -> (Flood, Arc<AtomicU64>) {
        let sent = Arc::new(AtomicU64::new(0));
        (Flood { left, next: 1, frame: Vec::new(), at: 0, sent: Arc::clone(&sent) }, sent)
    }

    /// A peer's receiving half that takes `budget` bytes, then waits for more budget, and records
    /// what it took; `fail` makes every later write fail as a reset connection.
    #[derive(Clone, Default)]
    struct Gated(Arc<Mutex<GatedState>>);

    #[derive(Default)]
    struct GatedState {
        budget: usize,
        taken: Vec<u8>,
        failed: bool,
        waker: Option<Waker>,
    }

    impl Gated {
        fn open(&self, bytes: usize) {
            let mut state = lock(&self.0);
            state.budget += bytes;
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }
        fn fail(&self) {
            let mut state = lock(&self.0);
            state.failed = true;
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }
        fn frames(&self) -> Vec<Frame> {
            let state = lock(&self.0);
            let mut frames = Vec::new();
            let mut at = 0;
            while at + 4 <= state.taken.len() {
                let len = u32::from_be_bytes(state.taken[at..at + 4].try_into().expect("four bytes")) as usize;
                if at + 4 + len > state.taken.len() {
                    break;
                }
                frames.push(Codec::Json.decode_frame(&state.taken[at + 4..at + 4 + len]).expect("a frame decodes"));
                at += 4 + len;
            }
            frames
        }
    }

    impl AsyncWrite for Gated {
        fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
            let mut state = lock(&self.0);
            if state.failed {
                return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
            }
            if state.budget == 0 {
                state.waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let n = buf.len().min(state.budget);
            state.budget -= n;
            state.taken.extend_from_slice(&buf[..n]);
            Poll::Ready(Ok(n))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            if lock(&self.0).failed { Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())) } else { Poll::Ready(Ok(())) }
        }
        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// A peer reading nothing, its writer stuck, is still closed at the idle timeout: the reader
    /// waiting for room is not a call in flight, and the linger drops both halves at `CUT_GRACE`.
    #[test]
    fn a_peer_that_reads_nothing_is_closed_at_its_idle_timeout() {
        let manual = Manual::new();
        let running = start_with(&manual, |_| {}, refuse);
        let (reader_dropped, writer_dropped) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (peer, sent) = flood(5_000);
        running.connect(Dropping(peer, Arc::clone(&reader_dropped)), Dropping(Stuck, Arc::clone(&writer_dropped)));
        manual.run();
        let read = sent.load(AtomicOrdering::SeqCst);
        assert!(read < 5_000, "the reader read every request from a peer reading nothing: {read}");
        manual.advance(DEFAULT_IDLE_TIMEOUT - Duration::from_millis(1));
        assert!(!writer_dropped.load(AtomicOrdering::SeqCst), "closed before the idle timeout");
        manual.advance(Duration::from_millis(1));
        assert!(!writer_dropped.load(AtomicOrdering::SeqCst), "the writer dropped before the grace");
        manual.advance(CUT_GRACE);
        assert!(writer_dropped.load(AtomicOrdering::SeqCst) && reader_dropped.load(AtomicOrdering::SeqCst), "a stuck connection outlived its idle timeout and grace");
    }

    /// Q35-13: a streamed reply to a peer that reads nothing is a call in flight, and its writer
    /// holds a frame it cannot write: once it has held it for `idle_timeout`, the connection is
    /// idle whatever it holds, its calls are cancelled `Disconnected`, and the linger drops it at
    /// `CUT_GRACE`. Planted break: idleness that waits for the calls to end, which leaves the
    /// connection open, its writer stuck, for as long as the handler produces.
    #[test]
    fn a_writer_stuck_for_the_idle_timeout_closes_its_connection_and_cancels_its_calls() {
        let manual = Manual::new();
        let cancelled = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&cancelled);
        let running = start_with(&manual, |_| {}, move |manual, delivery| match (delivery.frame, delivery.reply) {
            (Frame::Req { id, .. }, Some(reply)) => drop(manual.spawn(Box::pin(async move {
                while reply.send(Frame::Item { id, data: Data::default() }).await.is_ok() {}
            }))),
            (Frame::Cancel { .. }, None) => {
                counted.fetch_add(1, AtomicOrdering::SeqCst);
            }
            _ => {}
        });
        let (fed, writer_dropped) = (Fed::default(), Arc::new(AtomicBool::new(false)));
        running.connect(FedHalf(fed.clone()), Dropping(Stuck, Arc::clone(&writer_dropped)));
        manual.run();
        fed.push(&framed(&request(1)));
        manual.run();
        manual.advance(DEFAULT_IDLE_TIMEOUT - Duration::from_millis(1));
        assert_eq!(cancelled.load(AtomicOrdering::SeqCst), 0, "the call was cancelled before the idle timeout");
        manual.advance(Duration::from_millis(1));
        assert_eq!(cancelled.load(AtomicOrdering::SeqCst), 1, "the streamed reply's call is cancelled `Disconnected` at the idle timeout");
        assert!(!writer_dropped.load(AtomicOrdering::SeqCst), "the writer dropped before the grace");
        manual.advance(CUT_GRACE);
        assert!(writer_dropped.load(AtomicOrdering::SeqCst) && fed.dropped(), "the stuck connection outlived its idle timeout and grace");
    }

    /// A writer that keeps writing keeps a connection holding a call open past the idle timeout,
    /// and a writer holding nothing is no stall.
    /// Planted break: the mark left set once its frame is written, which closes the connection
    /// 60 s after its last item.
    #[test]
    fn a_writer_making_progress_keeps_its_call_s_connection_open() {
        let manual = Manual::new();
        let held: Arc<Mutex<Option<(u64, ReplyPath)>>> = Arc::default();
        let holding = Arc::clone(&held);
        let cancelled = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&cancelled);
        let running = start_with(&manual, |_| {}, move |_, delivery| match (delivery.frame, delivery.reply) {
            (Frame::Req { id, .. }, Some(reply)) => *lock(&holding) = Some((id, reply)),
            (Frame::Cancel { .. }, None) => {
                counted.fetch_add(1, AtomicOrdering::SeqCst);
            }
            _ => {}
        });
        let (fed, written) = (Fed::default(), Recorded::default());
        running.connect(FedHalf(fed.clone()), RecordedHalf(written.clone()));
        manual.run();
        fed.push(&framed(&request(1)));
        manual.run();
        let (id, reply) = lock(&held).take().expect("the request reached the server");
        for minute in 1..=5u32 {
            manual.advance(Duration::from_secs(50));
            let item = reply.send(Frame::Item { id, data: Data::default() });
            drop(manual.spawn(Box::pin(async move {
                let _ = item.await;
            })));
            manual.run();
            assert_eq!(written.frames().len(), minute as usize, "the item written at {} s", minute * 50);
        }
        assert_eq!(cancelled.load(AtomicOrdering::SeqCst), 0, "a call whose writer wrote every 50 s was cancelled as idle");
        // A writer with nothing left to write is no stall: the call in flight holds the connection.
        manual.advance(Duration::from_secs(600));
        assert_eq!(cancelled.load(AtomicOrdering::SeqCst), 0, "a call was cancelled with its writer holding nothing");
        assert!(!written.shut(), "a call's connection closed with its writer holding nothing");
        drop(reply);
    }

    /// A peer reading 1 KiB a second is progress: a streamed item of 200 KiB, which takes it over
    /// three minutes, is written whole and its call is not cancelled, the stall timed from the
    /// writer's last write the socket took bytes from. Planted break: the stall timed from the
    /// frame the writer took, which cancels the call at 60 s with the item a third written.
    #[test]
    fn a_peer_reading_slowly_keeps_a_large_item_s_call() {
        const ITEM: usize = 200 * 1024;
        let manual = Manual::new();
        let cancelled = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&cancelled);
        let item = Data::new(format!("\"{}\"", "a".repeat(ITEM)).into_bytes());
        let running = start_with(&manual, |_| {}, move |manual, delivery| match (delivery.frame, delivery.reply) {
            (Frame::Req { id, .. }, Some(reply)) => {
                let item = item.clone();
                drop(manual.spawn(Box::pin(async move {
                    let _ = reply.send(Frame::Item { id, data: item }).await;
                    pending::<()>().await;
                })));
            }
            (Frame::Cancel { .. }, None) => {
                counted.fetch_add(1, AtomicOrdering::SeqCst);
            }
            _ => {}
        });
        let (fed, gated) = (Fed::default(), Gated::default());
        running.connect(FedHalf(fed.clone()), gated.clone());
        manual.run();
        fed.push(&framed(&request(1)));
        manual.run();
        for _ in 0..(ITEM / 1024 + 4) {
            gated.open(1024);
            manual.advance(Duration::from_secs(1));
        }
        assert_eq!(cancelled.load(AtomicOrdering::SeqCst), 0, "a call whose peer read 1 KiB a second was cancelled as stalled");
        assert_eq!(gated.frames().len(), 1, "the item was written whole");
    }

    /// A writer that fails, as a reset peer's does, ends a reader waiting for room at once.
    #[test]
    fn a_failing_writer_ends_a_reader_waiting_for_room() {
        let manual = Manual::new();
        let running = start_with(&manual, |_| {}, refuse);
        let (reader_dropped, gated) = (Arc::new(AtomicBool::new(false)), Gated::default());
        let (peer, sent) = flood(5_000);
        running.connect(Dropping(peer, Arc::clone(&reader_dropped)), gated.clone());
        manual.run();
        let read = sent.load(AtomicOrdering::SeqCst);
        assert!(read < 5_000, "{read}");
        assert!(!reader_dropped.load(AtomicOrdering::SeqCst), "the reader ended before the writer failed");
        gated.fail();
        manual.run();
        assert!(reader_dropped.load(AtomicOrdering::SeqCst), "a reader waiting for room outlived its failed writer");
        assert!(running.queued().is_empty(), "the connection is still registered: {:?}", running.queued());
        assert_eq!(sent.load(AtomicOrdering::SeqCst), read, "the reader read on after its writer failed");
    }

    /// A peer that writes its requests, closes its half and reads slowly: every request the
    /// server read is either answered on the wire or, still in flight when the server reads the
    /// FIN, cancelled; none is lost between the two.
    #[test]
    fn a_half_closed_slow_reader_loses_no_request_unaccounted() {
        const N: u64 = 300;
        let manual = Manual::new();
        let cancels = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&cancels);
        let running = start_with(&manual, |_| {}, move |manual, delivery| match &delivery.frame {
            Frame::Cancel { .. } => {
                counted.fetch_add(1, AtomicOrdering::SeqCst);
            }
            _ => refuse(manual, delivery),
        });
        let (fed, gated) = (Fed::default(), Gated::default());
        let mut bytes = Vec::new();
        for id in 1..=N {
            bytes.extend(framed(&request(id)));
        }
        fed.push(&bytes);
        fed.close();
        running.connect(FedHalf(fed.clone()), gated.clone());
        manual.run();
        for _ in 0..2_000 {
            gated.open(64);
            manual.run();
        }
        let answered = gated.frames().iter().filter(|frame| matches!(frame, Frame::Err { .. })).count();
        let cancelled = cancels.load(AtomicOrdering::SeqCst);
        eprintln!("{answered} answered and {cancelled} cancelled of {N}");
        assert_eq!(answered + cancelled, N as usize, "a request was neither answered nor cancelled");
        assert!(fed.dropped(), "the FIN was never read");
    }

    /// A `cancel` behind a full writer waits for room: the call it names runs until then.
    #[test]
    fn a_cancel_behind_a_full_writer_waits_for_room() {
        let manual = Manual::new();
        let cancels = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&cancels);
        let held = Arc::new(Mutex::new(Vec::new()));
        let holding = Arc::clone(&held);
        let running = start_with(&manual, |_| {}, move |manual, delivery| match (delivery.frame, delivery.reply) {
            (Frame::Req { id, .. }, Some(reply)) if lock(&holding).is_empty() => lock(&holding).push((id, reply, delivery.ack)),
            (Frame::Req { id, .. }, Some(reply)) => drop(manual.spawn(Box::pin(async move {
                let _ = reply.send(Frame::Err { id, error: refusal() }).await;
            }))),
            (Frame::Cancel { .. }, _) => {
                counted.fetch_add(1, AtomicOrdering::SeqCst);
            }
            _ => {}
        });
        let (fed, gated) = (Fed::default(), Gated::default());
        let mut bytes = framed(&request(1));
        for id in 2..=3_000 {
            bytes.extend(framed(&request(id)));
        }
        bytes.extend(framed(&Codec::Json.encode_frame(&Frame::Cancel { id: 1 }).expect("a cancel encodes")));
        fed.push(&bytes);
        running.connect(FedHalf(fed.clone()), gated.clone());
        manual.run();
        assert_eq!(cancels.load(AtomicOrdering::SeqCst), 0, "the cancel was read past a full writer");
        for _ in 0..20_000 {
            gated.open(4096);
            manual.run();
        }
        assert_eq!(cancels.load(AtomicOrdering::SeqCst), 1, "the cancel was never read once room opened");
    }

    /// A frame written whole moves the expiry, however long after its call ended: an answer the
    /// peer read at 50 s keeps the connection open until 110 s. Planted break: no touch on a frame
    /// written, which closes it at 60 s.
    #[test]
    fn a_frame_written_late_moves_the_idle_expiry() {
        let manual = Manual::new();
        let running = start_with(&manual, |_| {}, refuse);
        let (fed, gated) = (Fed::default(), Gated::default());
        running.connect(FedHalf(fed.clone()), gated.clone());
        manual.run();
        fed.push(&framed(&request(1)));
        manual.run();
        manual.advance(Duration::from_secs(50));
        gated.open(1 << 20);
        manual.run();
        assert!(matches!(gated.frames()[..], [Frame::Err { id: 1, .. }]), "{:?}", gated.frames());
        manual.advance(Duration::from_secs(10));
        assert_eq!(gated.frames().len(), 1, "closed 60 s after the call's end though its answer was written at 50 s: {:?}", gated.frames());
        manual.advance(Duration::from_secs(50));
        assert!(matches!(gated.frames()[..], [Frame::Err { .. }, Frame::Goaway]), "not closed 60 s after the last frame written: {:?}", gated.frames());
    }

    /// A call that ends with nothing written, its answer queued behind a writer that cannot
    /// write, moves the expiry to its end. Planted break: no touch when the count of calls
    /// changes, which closes the connection as the call ends, 600 s after its frame was read.
    #[test]
    fn a_call_s_end_moves_the_idle_expiry_with_nothing_written() {
        let manual = Manual::new();
        let held = Arc::new(Mutex::new(Vec::new()));
        let holding = Arc::clone(&held);
        let running = start_with(&manual, |_| {}, move |_, delivery| {
            if let (Frame::Req { id, .. }, Some(reply)) = (delivery.frame, delivery.reply) {
                lock(&holding).push((id, reply));
            }
        });
        let (fed, gated) = (Fed::default(), Gated::default());
        running.connect(FedHalf(fed.clone()), gated.clone());
        manual.run();
        fed.push(&framed(&request(1)));
        manual.run();
        manual.advance(Duration::from_secs(600));
        for (id, reply) in lock(&held).drain(..) {
            drop(manual.spawn(Box::pin(async move {
                let _ = reply.send(Frame::Res { id, data: Data::default() }).await;
            })));
        }
        manual.run();
        manual.advance(DEFAULT_IDLE_TIMEOUT - Duration::from_secs(1));
        manual.advance(CUT_GRACE);
        assert!(!fed.dropped(), "the connection closed less than `idle_timeout` after its call ended");
        manual.advance(Duration::from_secs(1));
        manual.advance(CUT_GRACE);
        assert!(fed.dropped(), "the connection outlived `idle_timeout` after its call ended");
    }

    /// A frame's declared length is not handed to the reader as a buffer: a peer declaring 1 GiB
    /// and sending ten bytes is read into buffers of a few KiB. Planted break: the body allocated
    /// at its declared length and read with `read_exact`.
    #[test]
    fn a_declared_length_is_not_allocated_before_its_bytes_arrive() {
        struct Declaring {
            sent: Vec<u8>,
            at: usize,
            largest: Arc<AtomicUsize>,
        }
        impl AsyncRead for Declaring {
            fn poll_read(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
                self.largest.fetch_max(buf.len(), AtomicOrdering::SeqCst);
                if self.at == self.sent.len() {
                    return Poll::Pending;
                }
                let n = buf.len().min(self.sent.len() - self.at);
                let at = self.at;
                buf[..n].copy_from_slice(&self.sent[at..at + n]);
                self.at += n;
                Poll::Ready(Ok(n))
            }
        }
        let largest = Arc::new(AtomicUsize::new(0));
        let mut sent = (1u32 << 30).to_be_bytes().to_vec();
        sent.extend_from_slice(b"0123456789");
        let mut reading = Reading::new(Box::pin(Declaring { sent, at: 0, largest: Arc::clone(&largest) }));
        let mut frame = Box::pin(reading.frame(u64::from(u32::MAX)));
        for _ in 0..4 {
            assert!(frame.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        }
        let largest = largest.load(AtomicOrdering::SeqCst);
        assert!(largest <= 64 * 1024, "a buffer of {largest} bytes was handed to the reader for ten bytes received");
    }
}

/// A handshake that never finishes holds its connection no longer than `idle_timeout`, so a
/// server at `max_connections` accepts again once it passes. Planted break: the handshake awaited
/// unbounded, which keeps the slot for ever.
#[test]
fn a_handshake_that_never_finishes_is_dropped_at_the_idle_timeout() {
    let manual = Manual::new();
    let running = Running::start(&manual, Answer::Refused, |core| core.set_max_connections(Count::Max(1)));
    running.connect.unbounded_send(Incoming::handshaking(peer(), pending())).expect("the listener takes the connection");
    running.connect(FedHalf(Fed::default()), RecordedHalf(Recorded::default()));
    manual.run();
    assert_eq!(running.accepted.load(AtomicOrdering::SeqCst), 1, "the server accepted past `max_connections(1)`");
    manual.advance(DEFAULT_IDLE_TIMEOUT - Duration::from_millis(1));
    assert_eq!(running.accepted.load(AtomicOrdering::SeqCst), 1, "the stalled handshake was dropped before its timeout");
    manual.advance(Duration::from_millis(1));
    assert_eq!(running.accepted.load(AtomicOrdering::SeqCst), 2, "a handshake that never finishes held its slot past `idle_timeout`");
}
