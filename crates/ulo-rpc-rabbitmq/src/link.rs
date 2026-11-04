use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use futures_util::stream::FuturesUnordered;
use lapin::message::Delivery as AmqpDelivery;
use lapin::options::{
    BasicAckOptions, BasicCancelOptions, BasicConsumeOptions, BasicPublishOptions, BasicQosOptions, BasicRejectOptions,
    ConfirmSelectOptions, ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, ShortString};
use lapin::uri::AMQPUri;
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties, Consumer, ExchangeKind, PublisherConfirm};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use ulo::{AppHandle, BoxError, BoxFuture, Cut, Permanent, Redacted, Refused, Runtime, RuntimeShutDown, TaskEnd, Timer, ValueHandle};
use ulo_runtime_tokio::Tokio;
#[cfg(feature = "conformance-probe")]
use ulo_rpc::link::ClientProbe;
use ulo_rpc::link::{HoldUntil, Inbound};
use ulo_transport::sync::ordered;
use ulo_transport::{Count, Details, ErrorKind, SHUTDOWN_MESSAGE};
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, ConnectTimedOut, Data, Delivery, DeliveryMode, ErrorBody, Frame, Link, NoDestination, NoTimer,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};

/// The header naming a message's frame kind where the message alone does not tell it: `open` on
/// a pattern's queue, `in`, `in_end` and `cancel` on the control exchange, `opened` on the reply
/// queue. A message on a pattern's queue without it is a `req` when it carries `reply_to` and an
/// `evt` when it does not.
const KIND: &str = "ulo-t";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The fanout exchange every server instance binds its own queue to, so each sees every control
/// message and acts on the ones whose `correlation_id` names a call it holds. A streamed request's
/// items and every `cancel` travel here: a pattern's queue hands each message to one consumer,
/// and these have to reach the instance that took the call.
const CONTROL: &str = "ulo.rpc.control";

/// RabbitMQ's direct reply-to pseudo-queue: the client consumes it, without acknowledgments, on
/// the channel it publishes requests on.
const REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// The channel's prefetch when the server's `max_inflight` is `Count::Unlimited`.
const UNLIMITED_PREFETCH: u16 = 64;

/// How long a server's `listen` and a client's connect wait for the broker, unless
/// [`RabbitMq::connect_within`] says otherwise.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// Frames queued for one side's writer ahead of the next send's wait: each send is queued at its call and waits until it is
/// among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// How long a server's `close` waits, from its first poll, for its writer to publish the answers
/// queued before it and for the broker to close the connection (transports DESIGN §5.2).
const CUT_GRACE: Duration = Duration::from_secs(2);

/// The RabbitMQ link.
///
/// Each side publishes through one writer task, fed in order by every send on it, so the frames a
/// side sends go out on its channel in the order they were sent: a request's `in` items before its
/// `in_end`, and a `cancel` after the request it names. A request's lane and the control exchange
/// are separate queues, so the broker can still deliver a `cancel` ahead of its request.
///
/// The link's connections and tasks live on the tokio runtime it holds, the one current where it
/// was built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may
/// be polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
///
/// A server's `listen` needs the app's `Timer`, which `prepare` reads, and refuses with
/// [`NoTimer`] on an app that has none.
///
/// A link dropped without `close`, as a server's is when its app drops before the close step,
/// aborts the tasks holding its channels, so its connections close, and a client connect still
/// running.
pub struct RabbitMq {
    pub(crate) url: String,
    pub(crate) codec: Codec,
    pub(crate) prefetch: u16,
    pub(crate) runtime: Option<Tokio>,
    pub(crate) connect_within: Duration,
    /// The app's clock, kept by `prepare` for `listen`.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    pub(crate) state: Mutex<State>,
    #[cfg(feature = "conformance-probe")]
    pub(crate) probe: Arc<ClientProbe>,
}

#[derive(Default)]
pub(crate) struct State {
    /// The server side and its consumers' tasks.
    server: Option<(Arc<ServerSide>, Vec<AbortHandle>)>,
    /// The client side's connection, closed on the runtime it was opened on, and the tasks of
    /// its writer and its reply router.
    client: Option<(Connection, Tokio, [AbortHandle; 2])>,
    /// The client's connect, running or answered and not yet taken.
    connecting: Option<Connecting>,
}

/// A link dropped without `close` aborts the tasks holding its channels: an `AbortHandle` dropped
/// leaves its task running, and a channel keeps its connection open until the process ends.
/// `close` closes the connections instead, and the tasks end with them. A client connect still
/// running is aborted too, which leaves lapin's thread for it (transports DESIGN §5.2).
impl Drop for State {
    fn drop(&mut self) {
        if let Some((_, tasks)) = &self.server {
            tasks.iter().for_each(AbortHandle::abort);
        }
        if let Some((_, _, tasks)) = &self.client {
            tasks.iter().for_each(AbortHandle::abort);
        }
        if let Some(connecting) = &self.connecting {
            connecting.task.abort();
        }
    }
}

/// What a client's connect opens: the connection, its channel and the direct reply-to consumer.
type Opened = (Connection, Channel, Consumer);

/// A client connect the link owns rather than the call that asked for it, run as a task on the
/// link's runtime under `connect_within`. Every `connect` arriving while it runs waits on it, so a
/// call whose own timeout ends its wait leaves the connect running for the next call, and against
/// a silent broker the link opens one connection per bound rather than one per call, each leaving
/// a lapin thread behind (transports DESIGN §5.2).
struct Connecting {
    /// `true` once `outcome` is written. The task holds the sender, so a task that ends unwritten,
    /// dropped by its runtime's shutdown, closes it.
    done: watch::Receiver<bool>,
    /// Taken by the `connect` that builds the client side on it, so each connection is handed out
    /// once; a failure stays for every `connect` that waited on it.
    outcome: Arc<Mutex<Option<Outcome>>>,
    task: AbortHandle,
}

enum Outcome {
    Opened(Box<Opened>),
    Failed(Failure),
}

/// Why a client connect ended without a connection, answered to every `connect` waiting on it.
#[derive(Clone)]
enum Failure {
    TimedOut,
    Refused(Refused),
    Lapin(lapin::Error),
    /// The connect panicked: its message, redacted as a task's panic is.
    Panicked(Arc<Redacted>),
    /// `close` ran while the connect was running or before its connection was taken.
    Closed,
}

/// The error every `connect` waiting on a connect that panicked answers. The panic is its
/// `source()`, so `ulo::is_panic` finds it, on the call's error too.
#[derive(Debug)]
pub(crate) struct ConnectPanicked(Arc<Redacted>);

impl std::fmt::Display for ConnectPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the RabbitMQ link's connect panicked: {}", self.0)
    }
}

impl std::error::Error for ConnectPanicked {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

/// A task's value, the task aborted when this drops.
struct Aborting<T>(ValueHandle<T>);

impl<T> Future for Aborting<T> {
    type Output = Result<T, TaskEnd>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> Drop for Aborting<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Connecting {
    /// `opening` runs as a task of its own on `runtime`, so a panic in it ends that task alone and
    /// is answered as `TaskEnd::Panicked`, its message redacted; the task holding the outcome
    /// awaits it under `within` and aborts it when it ends first.
    fn start<F>(runtime: &Tokio, opening: F, within: Duration, clock: Arc<dyn Runtime>) -> Connecting
    where
        F: Future<Output = lapin::Result<Opened>> + Send + 'static,
    {
        let (answered, done) = watch::channel(false);
        let outcome = Arc::new(Mutex::new(None));
        let written = Arc::clone(&outcome);
        let opening = Aborting(ulo::spawn_with(runtime, opening));
        let task = runtime.handle().spawn(async move {
            let answer = match ulo::timeout(&*clock, within, opening).await {
                Ok(Ok(Ok(opened))) => Outcome::Opened(Box::new(opened)),
                Ok(Ok(Err(error))) => Outcome::Failed(Failure::Lapin(error)),
                Ok(Err(TaskEnd::Panicked(message))) => Outcome::Failed(Failure::Panicked(message)),
                // The runtime dropped the connect unfinished, having shut down.
                Ok(Err(_)) => Outcome::Failed(Failure::Refused(Refused)),
                Err(Cut::Expired) => Outcome::Failed(Failure::TimedOut),
                Err(Cut::Refused(refused)) => Outcome::Failed(Failure::Refused(refused)),
            };
            // A `close` that ran first wrote its own answer, which stands, and a connection opened
            // after it is dropped here, which closes it.
            lock(&written).get_or_insert(answer);
            answered.send_replace(true);
        });
        Connecting { done, outcome, task: task.abort_handle() }
    }

    /// Whether a `connect` arriving now waits on this one: it is still running, or its connection
    /// has not been taken. A failed connect, or one whose task ended unanswered, is replaced.
    fn joinable(&self) -> bool {
        match &*lock(&self.outcome) {
            Some(Outcome::Opened(_)) => true,
            Some(Outcome::Failed(_)) => false,
            None => !*self.done.borrow() && self.done.has_changed().is_ok(),
        }
    }
}

/// The client's connection, without lapin's auto-recovery: a direct reply-to address lives as long
/// as its channel, so the replies a recovered connection's new channel would wait for are gone. A
/// lost connection ends the reply lane instead, and `RpcClient` connects again for the next call.
async fn open_client(uri: AMQPUri) -> lapin::Result<Opened> {
    let connection = Connection::connect_uri(uri, ConnectionProperties::default()).await?;
    let channel = connection.create_channel().await?;
    // Confirm mode is what makes a `mandatory` publish's `basic.return` reach the publisher: lapin
    // resolves the publish's confirmation with the returned message.
    channel.confirm_select(ConfirmSelectOptions::default()).await?;
    channel.exchange_declare(CONTROL.into(), ExchangeKind::Fanout, ExchangeDeclareOptions::default(), FieldTable::default()).await?;
    let replies = channel
        .basic_consume(REPLY_TO.into(), "".into(), BasicConsumeOptions { no_ack: true, ..Default::default() }, FieldTable::default())
        .await?;
    Ok((connection, channel, replies))
}

impl RabbitMq {
    /// The link on `url`, `amqp://..` or `amqps://..`, parsed in `prepare` and connected lazily.
    pub fn url(url: impl Into<String>) -> Self {
        RabbitMq {
            url: url.into(),
            codec: Codec::Json,
            prefetch: UNLIMITED_PREFETCH,
            runtime: Tokio::try_current(),
            connect_within: CONNECT_WITHIN,
            timer: None,
            state: Mutex::new(State::default()),
            #[cfg(feature = "conformance-probe")]
            probe: Arc::default(),
        }
    }

    /// The client side as the RPC conformance suite reads it. Built by the `conformance-probe`
    /// feature, which is for the conformance suite, not an application feature.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn probe(&self) -> &ClientProbe {
        &self.probe
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// How long the link waits for the broker before it gives up, 30 seconds by default. On a
    /// server it bounds the whole of `listen`: the connection that checks the broker, its close,
    /// the serving connection opened after it, and the queues and consumers set up on that one, so
    /// a broker lost between the two connections fails `listen` here rather than after lapin's
    /// sixteen reconnect attempts. On a client it bounds the connect, which the link runs once for
    /// every call waiting on it: the first call that needs a connection starts it, each call waits
    /// on it under its own timeout, five seconds unset, and a call whose timeout ends first leaves
    /// the connect running for the calls after it. A wait that runs out fails with
    /// [`ConnectTimedOut`], which a server's `listen` returns as `Bind` and `RpcClient` answers
    /// `Unavailable` to every call still waiting, the next call starting a new connect; a connect
    /// that fails is replaced by the next call's the same way. A server's wait is timed on the
    /// app's `Timer`, which `prepare` reads, and a client's on the runtime the connect's first
    /// call was handed, its client's, the app's inside an app; a clock whose runtime has shut down
    /// fails `listen` as `Permanent` `RuntimeShutDown` and a call `Unavailable` with
    /// `reason: "runtime_shut_down"`. A client connect that panics fails every call waiting on
    /// it `Unavailable`, the panic's message redacted and kept as the error's source, so
    /// `ulo::is_panic` is true of the call's error. A zero bound is refused in `usable`, so in a server's
    /// `prepare` and where a client takes the link.
    ///
    /// A connect that runs out leaves lapin's `lapin-io-loop` thread and its socket behind, since
    /// lapin 4.12 has no way to cancel one (transports DESIGN §5.2): against a silent broker, one
    /// per `listen` and one per bound for a client, however many calls time out meanwhile.
    pub fn connect_within(mut self, bound: Duration) -> Self {
        self.connect_within = bound;
        self
    }

    /// The tokio runtime the link's connections and tasks run on, in place of the one current
    /// where it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }

    /// The client side on a connection its connect opened: its writer and reply router spawned on
    /// the link's runtime and the connection written to the link's state, with no await between.
    fn client_side(&self, runtime: Tokio, (connection, channel, replies): Opened) -> Outbound {
        let side = Arc::new(ClientSide {
            channel,
            codec: self.codec,
            id: uuid::Uuid::new_v4().simple().to_string(),
            calls: Mutex::new(HashMap::new()),
            runtime: runtime.clone(),
            #[cfg(feature = "conformance-probe")]
            probe: Arc::clone(&self.probe),
        });
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        #[cfg(feature = "conformance-probe")]
        {
            let late = writes.clone();
            let counted = Arc::downgrade(&side);
            self.probe.attach(
                move |call| drop(late.send(Job::Opened(call))),
                move || counted.upgrade().map_or(0, |side| lock(&side.calls).len()),
            );
        }
        let writing = runtime.handle().spawn(client_writer(Arc::clone(&side), queued));
        let (frames, replies_out) = mpsc::unbounded_channel();
        let routing = runtime.handle().spawn(route_replies(Arc::clone(&side), replies, frames, writes.clone()));
        lock(&self.state).client = Some((connection, runtime.clone(), [writing.abort_handle(), routing.abort_handle()]));

        Outbound {
            // Queued at the call, so frames go out in the order their sends were called.
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let (answer, answered) = oneshot::channel();
                let room = writes.send(Job::Send { pattern, frame, answer });
                Box::pin(async move {
                    room.await.map_err(|_| BoxError::from(CLOSED))?;
                    answered.await.map_err(|_| BoxError::from(CLOSED))?
                })
            }),
            replies: receiver_stream(replies_out),
        }
    }

    /// The error a `connect` that waited on a failed connect answers.
    fn failed(&self, failure: Failure, uri: &AMQPUri) -> BoxError {
        match failure {
            Failure::TimedOut => ConnectTimedOut::new(self.connect_within, address(uri), "RabbitMQ").into(),
            Failure::Refused(refused) => RuntimeShutDown::new(refused).into(),
            Failure::Lapin(error) => error.into(),
            Failure::Panicked(message) => Box::new(ConnectPanicked(message)),
            Failure::Closed => CLOSED.into(),
        }
    }
}

const NO_RUNTIME: &str = "the RabbitMQ link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for RabbitMq {
    const NAME: &'static str = "rabbitmq";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerQueue)
            .native_backpressure(true)
            .miss_signal(true)
            .holds_unserved(true)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime()?.require_io()?;
        if self.connect_within.is_zero() {
            return Err("the RabbitMQ link's `connect_within` is zero, so no connection to the broker could be made within it".into());
        }
        Ok(())
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.timer = app.timer().cloned();
        // `listen` binds on the link's runtime: one that has shut down, or has no IO driver, is
        // refused here, before the configuration and before any server binds. The shutdown is
        // asked first, since a runtime that has shut down answers nothing about its driver.
        let runtime = self.runtime()?;
        runtime.run(async {}).await.map_err(Permanent::new)?;
        runtime.require_io().map_err(Permanent::new)?;
        self.usable()?;
        uri(&self.url)?;
        Ok(())
    }

    /// The unacknowledged deliveries the pattern consumers take at once, together (`basic.qos`
    /// with `global = true`, which RabbitMQ reads as one window shared by every consumer on the
    /// channel): the server's bound, 1,024 at `Count::Default`, capped at AMQP's 65 535, so the
    /// broker holds a request the server has no place for rather than delivering it to be
    /// refused. `Unlimited` gives 64, since AMQP's own unlimited, 0, would hand an instance its
    /// whole queue.
    fn max_inflight(&mut self, calls: Count) {
        self.prefetch = match calls.max_inflight() {
            Some(calls) => u16::try_from(calls).unwrap_or(u16::MAX),
            None => UNLIMITED_PREFETCH,
        };
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let runtime = self.runtime()?;
        let clock = self.timer.clone().ok_or(NoTimer { link: "RabbitMQ" })?;
        let (uri, patterns, prefetch) = (uri(&self.url)?, patterns.to_vec(), self.prefetch);
        let timed_out = ConnectTimedOut::new(self.connect_within, address(&uri), "RabbitMQ");
        let connecting = runtime.run(async move {
            // lapin's auto-recovery retries a first connect too, sixteen times on an
            // exponential backoff, so a broker refusing it would hold `listen` past 30 s, and
            // recovery cannot be turned on once a connection is up. A first connection without
            // it reports the broker's refusal at once and is closed; the serving connection,
            // opened after it, recovers.
            let checked = Connection::connect_uri(uri.clone(), ConnectionProperties::default()).await?;
            let _ = checked.close(200, "connected".into()).await;
            let connection = Connection::connect_uri(uri, ConnectionProperties::default().enable_auto_recover()).await?;
            match subscribe(&connection, &patterns, prefetch).await {
                Ok((channel, consumers, control)) => Ok((connection, channel, consumers, control)),
                Err(error) => {
                    let _ = connection.close(200, "bind failed".into()).await;
                    Err(error)
                }
            }
        });
        let (connection, channel, consumers, control) = match ulo::timeout(&*clock, timed_out.within(), connecting).await {
            Ok(connected) => connected.map_err(Permanent::new)??,
            Err(Cut::Expired) => return Err(timed_out.into()),
            Err(Cut::Refused(refused)) => return Err(Permanent::new(RuntimeShutDown::new(refused)).into()),
        };
        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        let writer = runtime.handle().spawn(server_writer(channel.clone(), queued)).abort_handle();
        let side = Arc::new(ServerSide {
            connection,
            channel,
            writes,
            writer,
            timer: clock,
            codec: self.codec,
            runtime: runtime.clone(),
            calls: Arc::new(Calls::new()),
            phase,
            tags: consumers.iter().map(Consumer::tag).collect(),
            deliveries: Mutex::new(Some(deliveries)),
        });
        let handle = runtime.handle();
        let mut tasks: Vec<AbortHandle> =
            consumers.into_iter().map(|consumer| handle.spawn(request_lane(Arc::clone(&side), consumer)).abort_handle()).collect();
        tasks.push(handle.spawn(control_lane(Arc::clone(&side), control)).abort_handle());
        lock(&self.state).server = Some((side, tasks));
        Ok(receiver_stream(inbound))
    }

    // Waits on the link's one client connect, starting it when none is running, its bound timed
    // on the clock of the `client` that started it (`Connecting`). The client side is built on the
    // connection in this poll, with no await between taking it and writing the link's state.
    async fn connect(&self, client: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let runtime = self.runtime()?;
        let uri = uri(&self.url)?;
        loop {
            let (mut done, outcome) = {
                let mut state = lock(&self.state);
                match state.connecting.as_ref().filter(|connecting| connecting.joinable()) {
                    Some(connecting) => (connecting.done.clone(), Arc::clone(&connecting.outcome)),
                    None => {
                        let connecting = Connecting::start(&runtime, open_client(uri.clone()), self.connect_within, Arc::clone(client));
                        let joined = (connecting.done.clone(), Arc::clone(&connecting.outcome));
                        state.connecting = Some(connecting);
                        joined
                    }
                }
            };
            let answered = done.wait_for(|done| *done).await.is_ok();
            let taken = {
                let mut state = lock(&self.state);
                let mut written = lock(&outcome);
                match written.take() {
                    Some(Outcome::Opened(opened)) => {
                        if state.connecting.as_ref().is_some_and(|connecting| Arc::ptr_eq(&connecting.outcome, &outcome)) {
                            state.connecting = None;
                        }
                        Ok(*opened)
                    }
                    Some(Outcome::Failed(failure)) => {
                        *written = Some(Outcome::Failed(failure.clone()));
                        Err(Some(failure))
                    }
                    None => Err(None),
                }
            };
            return match taken {
                Ok(opened) => Ok(self.client_side(runtime, opened)),
                Err(Some(failure)) => Err(self.failed(failure, &uri)),
                // Another `connect` took the connection: this one starts the next.
                Err(None) if answered => continue,
                // The task ended unanswered: its runtime dropped it, having shut down.
                Err(None) => Err(match runtime.run(async {}).await {
                    Err(shut_down) => shut_down.into(),
                    Ok(()) => "the RabbitMQ link's connect ended without answering".into(),
                }),
            };
        }
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.as_ref().map(|(side, _)| Arc::clone(side));
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        let cancelling = Arc::clone(&side);
        let _ = side
            .runtime
            .run(async move {
                for tag in &cancelling.tags {
                    if let Err(error) = cancelling.channel.basic_cancel(tag.clone(), BasicCancelOptions::default()).await {
                        tracing::warn!(%error, consumer = tag.as_str(), "the RabbitMQ link could not cancel a consumer");
                    }
                }
            })
            .await;
        // The side is held weakly: its drop clears the table this waits on, and a watcher holding
        // it would keep it and the table after the link's drop.
        let (mut count, watched) = (side.calls.count.subscribe(), Arc::downgrade(&side));
        side.runtime.handle().spawn(async move {
            let _ = count.wait_for(|held| *held == 0).await;
            if let Some(side) = watched.upgrade() {
                lock(&side.deliveries).take();
            }
        });
    }

    async fn close(&self) -> Result<(), BoxError> {
        let (server, client, connecting) = {
            let mut state = lock(&self.state);
            (state.server.take(), state.client.take(), state.connecting.take())
        };
        // Started before the first await, so a `close` cut at its first poll leaves the server's
        // connection to the task, which publishes what the server queued before it.
        let closing = server.map(|(side, tasks)| {
            side.phase.send_replace(Phase::Closed);
            lock(&side.deliveries).take();
            side.close_after_writes(tasks)
        });
        let mut failure = None;
        // A connect still running is aborted, and a connection no `connect` took is closed; either
        // way the `connect`s waiting on it answer that the client is closed.
        let untaken = connecting.and_then(|connecting| {
            let untaken = {
                let mut outcome = lock(&connecting.outcome);
                let untaken = match outcome.take() {
                    Some(Outcome::Opened(opened)) => Some(opened.0),
                    kept => {
                        *outcome = kept;
                        None
                    }
                };
                outcome.get_or_insert(Outcome::Failed(Failure::Closed));
                untaken
            };
            connecting.task.abort();
            untaken
        });
        if let (Some(connection), Some(runtime)) = (untaken, &self.runtime)
            && let Err(error) = close_on(runtime, async move { connection.close(200, "close".into()).await }).await
        {
            failure = Some(error);
        }
        if let Some(closing) = closing {
            match closing.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failure = Some(error),
                // The task was dropped unfinished, its runtime having shut down.
                Err(_) => failure = Some(RuntimeShutDown::new(Refused).into()),
            }
        }
        if let Some((connection, runtime, _)) = client
            && let Err(error) = close_on(&runtime, async move { connection.close(200, "close".into()).await }).await
        {
            failure = Some(error);
        }
        match failure {
            Some(error) => Err(format!("the RabbitMQ link did not close cleanly: {error}").into()),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// A connection's `close` run on the link's runtime.
async fn close_on(runtime: &Tokio, close: impl Future<Output = lapin::Result<()>> + Send + 'static) -> Result<(), BoxError> {
    Ok(runtime.run(close).await??)
}

fn uri(url: &str) -> Result<AMQPUri, BoxError> {
    url.parse::<AMQPUri>().map_err(|error| format!("the RabbitMQ link's url does not parse: {error}").into())
}

/// The broker's `host:port`, without the url's credentials.
fn address(uri: &AMQPUri) -> String {
    format!("{}:{}", uri.authority.host, uri.authority.port)
}

/// Declares a queue per pattern and consumes each under the prefetch, one window for every
/// pattern under RabbitMQ's reading of `global = true`, then this instance's own queue on the
/// control exchange, on a channel of its own: lapin's topology replay after a reconnect cannot
/// redeclare a server-named queue, and a failed replay closes its channel. The control consumer
/// acknowledges nothing and is outside the window, so a request's items and `cancel` reach the
/// server while the window is full.
async fn subscribe(connection: &Connection, patterns: &[Pattern], prefetch: u16) -> Result<(Channel, Vec<Consumer>, Consumer), BoxError> {
    let channel = connection.create_channel().await?;
    channel.basic_qos(prefetch, BasicQosOptions { global: true }).await?;
    let mut consumers = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        channel.queue_declare(pattern.as_str().into(), QueueDeclareOptions::default(), FieldTable::default()).await?;
        // An empty tag has the broker assign one; a fixed tag would repeat across the patterns
        // sharing the channel, which the broker refuses.
        let consumer = channel
            .basic_consume(pattern.as_str().into(), "".into(), BasicConsumeOptions::default(), FieldTable::default())
            .await?;
        consumers.push(consumer);
    }

    let control = connection.create_channel().await?;
    control
        .exchange_declare(CONTROL.into(), ExchangeKind::Fanout, ExchangeDeclareOptions::default(), FieldTable::default())
        .await?;
    let queue: ShortString = format!("{CONTROL}.{}", uuid::Uuid::new_v4().simple()).into();
    control
        .queue_declare(queue.clone(), QueueDeclareOptions { auto_delete: true, ..Default::default() }, FieldTable::default())
        .await?;
    control.queue_bind(queue.clone(), CONTROL.into(), "".into(), QueueBindOptions::default(), FieldTable::default()).await?;
    let control_consumer = control
        .basic_consume(queue, "".into(), BasicConsumeOptions { no_ack: true, ..Default::default() }, FieldTable::default())
        .await?;
    Ok((channel, consumers, control_consumer))
}

/// The server side of a bound link: the channel its pattern consumers and replies share, the
/// calls it holds, and where deliveries go.
struct ServerSide {
    connection: Connection,
    channel: Channel,
    /// The server's writer, which every reply and `opened` is published through.
    writes: ordered::Sender<Write>,
    /// The writer's task, aborted when the connection is let go at [`CUT_GRACE`].
    writer: AbortHandle,
    /// The app's clock, which [`CUT_GRACE`] is timed on.
    timer: Arc<dyn Timer>,
    codec: Codec,
    /// The link's runtime, where an `Ack`'s settlement runs: `Ack::ack` and `reject` are
    /// synchronous, and lapin's acknowledgment is a future.
    runtime: Tokio,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// The pattern consumers' tags, cancelled by the drain.
    tags: Vec<ShortString>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
}

/// A held call's reply path holds the table it is held in and the writer's queue, so a side
/// dropped with calls held, its link dropped without `close`, would keep them, the writer and the
/// channel it publishes on, which keeps the connection open (F429).
impl Drop for ServerSide {
    fn drop(&mut self) {
        self.calls.clear();
    }
}

impl ServerSide {
    /// Queues the `err` of kind `unavailable` with the shutdown phrase for every call the side
    /// holds, and a flush behind them, in one hold of the table's lock, which every reply path
    /// takes to queue a frame, so a call's own answer is queued before them or dropped; then spawns
    /// the task that closes the connection once the writer reaches the flush, answering the
    /// close's outcome on the receiver. The task does not wait on the receiver, so a `close` cut short leaves it running.
    /// It ends at [`CUT_GRACE`] on the app's clock if the connection has not closed by then, and
    /// aborts `tasks` and the writer as it ends, so the side's last handles on the connection go
    /// and lapin closes it. A clock whose runtime has shut down cannot time the grace, and the
    /// connection is let go at once.
    fn close_after_writes(self: Arc<Self>, tasks: Vec<AbortHandle>) -> oneshot::Receiver<Result<(), BoxError>> {
        let (flush, flushed) = oneshot::channel();
        {
            let mut table = self.calls.close();
            for (_, call) in table.held.drain() {
                let error = ErrorBody::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE, Details::new());
                match self.codec.encode_frame(&Frame::Err { id: call.id, error }) {
                    Ok(bytes) => drop(write(&self.writes, call.reply, call.correlation, None, bytes)),
                    Err(error) => tracing::debug!(%error, "the RabbitMQ link could not encode the shutdown answer of a call it held at `close`"),
                }
            }
            self.calls.count.send_replace(0);
            drop(self.writes.send(Write::Flush(flush)));
        }
        let (answer, answered) = oneshot::channel();
        let grace = self.timer.try_sleep(CUT_GRACE);
        let runtime = self.runtime.handle().clone();
        drop(runtime.spawn(async move {
            let outcome = {
                let closing = async {
                    let _ = flushed.await;
                    self.connection.close(200, "close".into()).await.map_err(BoxError::from)
                };
                match select(pin!(closing), grace).await {
                    Either::Left((closed, _)) => closed,
                    Either::Right((Ok(()), _)) => Err("its connection had not closed `CUT_GRACE` after `close` began".into()),
                    Either::Right((Err(refused), _)) => Err(RuntimeShutDown::new(refused).into()),
                }
            };
            tasks.iter().for_each(AbortHandle::abort);
            if outcome.is_err() {
                self.writer.abort();
            }
            drop(self);
            let _ = answer.send(outcome);
        }));
        answered
    }

    fn deliver(&self, delivery: Delivery) {
        if let Some(deliveries) = lock(&self.deliveries).as_ref() {
            let _ = deliveries.send(delivery);
        }
    }

    async fn on_request(&self, delivery: AmqpDelivery) {
        let pattern = delivery.routing_key.as_str().to_owned();
        let headers = delivery.properties.headers().as_ref();
        let kind = kind_of(headers);
        let call_headers = call_headers(headers);
        let reply_to = delivery.properties.reply_to().as_ref().map(|reply| reply.as_str().to_owned());
        let correlation = delivery.properties.correlation_id().as_ref().map(|id| id.as_str().to_owned());
        let acker = delivery.acker;
        let runtime = self.runtime.handle().clone();
        // Settled once the handler completes, so the queue redelivers a call whose server died
        // mid-handler: at-least-once.
        let ack = Ack::new(move |accepted| {
            runtime.spawn(async move {
                let settled = if accepted {
                    acker.ack(BasicAckOptions::default()).await
                } else {
                    acker.reject(BasicRejectOptions { requeue: false }).await
                };
                if let Err(error) = settled {
                    tracing::warn!(%error, "the RabbitMQ link could not settle a delivery");
                }
            });
        });
        let data = Data::new(delivery.data);
        match (reply_to, kind.as_deref()) {
            (None, None) => self.deliver(Delivery { frame: Frame::Evt { pattern, headers: call_headers, data }, reply: None, ack }),
            (Some(reply), None) => {
                let key = correlation.clone().unwrap_or_else(|| reply.clone());
                let (id, path) = self.hold(key, reply, correlation);
                self.deliver(Delivery { frame: Frame::Req { id, pattern, headers: call_headers, data }, reply: Some(path), ack });
            }
            (Some(reply), Some(OPEN)) => {
                let key = correlation.clone().unwrap_or_else(|| reply.clone());
                let (id, path) = self.hold(key, reply.clone(), correlation.clone());
                // The caller holds the request's items until this arrives, so they reach the
                // control exchange after the call is held here.
                if let Err(error) = write(&self.writes, reply, correlation, Some(OPENED), Bytes::new()).await {
                    tracing::warn!(%error, pattern, "the RabbitMQ link could not acknowledge a streamed request");
                }
                self.deliver(Delivery { frame: Frame::Open { id, pattern, headers: call_headers }, reply: Some(path), ack });
            }
            (_, Some(kind)) => {
                tracing::warn!(pattern, kind, "the RabbitMQ link rejected a message of an unknown kind");
                ack.reject();
            }
        }
    }

    fn on_control(&self, delivery: AmqpDelivery) {
        let Some(key) = delivery.properties.correlation_id().as_ref().map(|id| id.as_str().to_owned()) else { return };
        let frame = match kind_of(delivery.properties.headers().as_ref()).as_deref() {
            Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(delivery.data) }, path)),
            Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
            Some(CANCEL) => self.calls.release(&key).map(|call| (Frame::Cancel { id: call.id }, call.path)),
            _ => None,
        };
        if let Some((frame, path)) = frame {
            self.deliver(Delivery { frame, reply: Some(path), ack: Ack::none() });
        }
    }

    /// Holds a call under `key`, its `correlation_id`, the key its control messages carry.
    fn hold(&self, key: String, reply: String, correlation: Option<String>) -> (u64, ReplyPath) {
        let writes = self.writes.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let released = key.clone();
        let (to, correlated) = (reply.clone(), correlation.clone());
        // Queued at the call, so replies go out in the order their sends were called, and under
        // the table's lock, so a frame is queued before `close`'s answers and flush or not at all.
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let mut table = lock(&calls.held);
            if table.closed {
                return Box::pin(std::future::ready(Err(BoxError::from(SERVER_CLOSED))));
            }
            if is_terminal(&frame) {
                calls.remove(&mut table, &released);
            }
            let written = codec.encode_frame(&frame).map(|bytes| write(&writes, to.clone(), correlated.clone(), None, bytes));
            drop(table);
            Box::pin(async move { written?.await })
        });
        let id = self.calls.hold(key, Held { id: 0, path: path.clone(), reply, correlation });
        (id, path)
    }
}

/// One reply-queue message for the server's writer, its outcome answered on `answer`.
struct Reply {
    reply: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
    answer: oneshot::Sender<Result<(), BoxError>>,
}

/// What the server's writer takes, in order.
enum Write {
    Reply(Reply),
    /// `close`'s, answered once every reply queued before it has been published.
    Flush(oneshot::Sender<()>),
}

/// Publishes the server's replies one after another, in the order they were queued, until every
/// sender is gone. The server's channel is not in confirm mode, so each publish is done once lapin
/// has written it.
async fn server_writer(channel: Channel, mut queued: ordered::Receiver<Write>) {
    while let Some(write) = queued.recv().await {
        match write {
            Write::Reply(Reply { reply, correlation, kind, body, answer }) => {
                #[cfg(test)]
                tests::gate::at_publish(&reply).await;
                let _ = answer.send(publish_reply(&channel, reply, correlation, kind, &body).await);
            }
            Write::Flush(flushed) => {
                let _ = flushed.send(());
            }
        }
    }
}

/// Queues one message on the server's writer at the call and answers a wait for it to be
/// published.
fn write(
    writes: &ordered::Sender<Write>,
    reply: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
) -> impl Future<Output = Result<(), BoxError>> + Send + 'static {
    let (answer, answered) = oneshot::channel();
    let room = writes.send(Write::Reply(Reply { reply, correlation, kind, body, answer }));
    async move {
        let gone = || BoxError::from(SERVER_CLOSED);
        room.await.map_err(|_| gone())?;
        answered.await.map_err(|_| gone())?
    }
}

async fn publish_reply(channel: &Channel, reply: String, correlation: Option<String>, kind: Option<&str>, body: &[u8]) -> Result<(), BoxError> {
    let mut properties = BasicProperties::default();
    if let Some(correlation) = correlation {
        properties = properties.with_correlation_id(correlation.into());
    }
    if let Some(headers) = amqp_headers(&CallHeaders::new(), kind) {
        properties = properties.with_headers(headers);
    }
    channel.basic_publish("".into(), reply.into(), BasicPublishOptions::default(), body, properties).await?;
    Ok(())
}

/// A pattern consumer's deliveries until it is cancelled or its channel closes. lapin yields an
/// `Err` while it recovers a dropped connection and carries on after it.
async fn request_lane(side: Arc<ServerSide>, mut consumer: Consumer) {
    while let Some(delivery) = consumer.next().await {
        match delivery {
            Ok(delivery) => side.on_request(delivery).await,
            Err(error) => tracing::warn!(%error, "the RabbitMQ link's consumer is recovering"),
        }
    }
}

async fn control_lane(side: Arc<ServerSide>, mut consumer: Consumer) {
    while let Some(delivery) = consumer.next().await {
        match delivery {
            Ok(delivery) => side.on_control(delivery),
            Err(error) => tracing::warn!(%error, "the RabbitMQ link's control consumer is recovering"),
        }
    }
}

const SERVER_CLOSED: &str = "the RabbitMQ link's server side is closed";

/// The calls a server holds, by the key their control messages carry, each with its local id, its
/// reply path and where its replies go. The local id is this link's own, unique across every
/// caller.
struct Calls {
    next: AtomicU64,
    held: Mutex<Table>,
    count: watch::Sender<usize>,
}

#[derive(Default)]
struct Table {
    held: HashMap<String, Held>,
    /// Set by `close` once it has answered the calls held: a reply sent after it is dropped.
    closed: bool,
}

struct Held {
    id: u64,
    path: ReplyPath,
    reply: String,
    correlation: Option<String>,
}

impl Calls {
    fn new() -> Self {
        Calls { next: AtomicU64::new(1), held: Mutex::new(Table::default()), count: watch::channel(0).0 }
    }

    fn hold(&self, key: String, mut call: Held) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        call.id = id;
        let mut table = lock(&self.held);
        table.held.insert(key, call);
        self.count.send_replace(table.held.len());
        id
    }

    fn get(&self, key: &str) -> Option<(u64, ReplyPath)> {
        lock(&self.held).held.get(key).map(|call| (call.id, call.path.clone()))
    }

    fn release(&self, key: &str) -> Option<Held> {
        self.remove(&mut lock(&self.held), key)
    }

    fn remove(&self, table: &mut Table, key: &str) -> Option<Held> {
        let call = table.held.remove(key);
        self.count.send_replace(table.held.len());
        call
    }

    /// The table, closed: every reply sent from here on is dropped. The caller drains what it
    /// holds.
    fn close(&self) -> MutexGuard<'_, Table> {
        let mut table = lock(&self.held);
        table.closed = true;
        table
    }

    /// Drops every held path, each of which holds this table through its closure.
    fn clear(&self) {
        lock(&self.held).held.clear();
        self.count.send_replace(0);
    }
}

/// The client side: one confirm-mode channel for every publish and for the direct reply-to
/// consumer, each call's `correlation_id` `<id>.<call>`.
struct ClientSide {
    channel: Channel,
    codec: Codec,
    id: String,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// The link's runtime, on which a held `cancel`'s hold runs out.
    runtime: Tokio,
    #[cfg(feature = "conformance-probe")]
    probe: Arc<ClientProbe>,
}

enum ClientCall {
    Unary,
    /// A streamed request: until the server acknowledges the `open`, its `in` and `in_end` frames
    /// wait in `held`, and the writer publishes them in order once it does.
    /// `until` is the deadline its `open` carried.
    Streaming { opened: bool, held: VecDeque<Frame>, until: HoldUntil },
    /// A streamed request cancelled before the server acknowledged its `open`: the `cancel` waits
    /// for the acknowledgment and is published alone, the frames held before it dropped. The entry
    /// stays until then, or until the hold runs out, the call's deadline or `CANCEL_HOLD` after
    /// the `cancel` for an `open` carrying none, when it is dropped.
    Cancelled,
}

const CLOSED: &str = "the RabbitMQ link's client is closed";

/// What the client's writer takes, in order: a frame to send, or the server's `opened` for a
/// streamed request, which releases the frames held for it.
enum Job {
    Send { pattern: Pattern, frame: Frame, answer: oneshot::Sender<Result<(), BoxError>> },
    Opened(u64),
}

/// The broker's confirmations the writer waits on while it publishes the next frames.
type Confirming = FuturesUnordered<BoxFuture<'static, ()>>;

/// Publishes the client's frames in the order they were queued, waiting for each confirmation
/// beside the publishes that follow, until every sender is gone.
async fn client_writer(side: Arc<ClientSide>, mut queued: ordered::Receiver<Job>) {
    let mut confirming = Confirming::new();
    loop {
        let job = tokio::select! {
            job = queued.recv() => match job {
                Some(job) => job,
                None => break,
            },
            Some(()) = confirming.next(), if !confirming.is_empty() => continue,
        };
        match job {
            Job::Send { pattern, frame, answer } => side.send(pattern, frame, answer, &mut confirming).await,
            Job::Opened(call) => {
                for frame in side.opened(call) {
                    side.publish_held(call, frame, &mut confirming).await;
                }
            }
        }
    }
    while confirming.next().await.is_some() {}
}

impl ClientSide {
    async fn send(self: &Arc<Self>, pattern: Pattern, frame: Frame, answer: oneshot::Sender<Result<(), BoxError>>, confirming: &mut Confirming) {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                lock(&self.calls).insert(id, ClientCall::Unary);
                let published = self.publish_request(&pattern, id, &headers, None, data.as_bytes()).await;
                self.settle(published, Some(id), pattern, answer, confirming);
            }
            Frame::Evt { headers, data, .. } => {
                let mut properties = BasicProperties::default();
                if let Some(table) = amqp_headers(&headers, None) {
                    properties = properties.with_headers(table);
                }
                let published = self
                    .channel
                    .basic_publish("".into(), pattern.as_str().into(), BasicPublishOptions::default(), data.as_bytes(), properties)
                    .await;
                self.settle(published, None, pattern, answer, confirming);
            }
            Frame::Open { id, headers, .. } => {
                lock(&self.calls).insert(id, ClientCall::Streaming { opened: false, held: VecDeque::new(), until: HoldUntil::at_open(&headers) });
                let published = self.publish_request(&pattern, id, &headers, Some(OPEN), &[]).await;
                self.settle(published, Some(id), pattern, answer, confirming);
            }
            Frame::In { id, data } => {
                self.control(id, Frame::In { id, data }, confirming).await;
                let _ = answer.send(Ok(()));
            }
            Frame::InEnd { id } => {
                self.control(id, Frame::InEnd { id }, confirming).await;
                let _ = answer.send(Ok(()));
            }
            Frame::Cancel { id } => {
                let call = lock(&self.calls).remove(&id);
                match call {
                    Some(ClientCall::Unary) => {
                        let published = self.publish_control(id, CANCEL, &[]).await;
                        self.settle(published, None, pattern, answer, confirming);
                    }
                    Some(ClientCall::Streaming { opened: true, .. }) => {
                        self.publish_held(id, Frame::Cancel { id }, confirming).await;
                        let _ = answer.send(Ok(()));
                    }
                        // The server takes the `open` and holds the call, so the `cancel` follows once it
                    // acknowledges it.
                    Some(ClientCall::Streaming { opened: false, until, .. }) => {
                        lock(&self.calls).insert(id, ClientCall::Cancelled);
                        self.expire_cancelled(id, until);
                        let _ = answer.send(Ok(()));
                    }
                    Some(ClientCall::Cancelled) | None => {
                        let _ = answer.send(Ok(()));
                    }
                }
            }
            other => {
                let _ = answer.send(Err(format!("a client does not send a `{}` frame", other.kind()).into()));
            }
        }
    }

    /// Answers a publish once the broker confirms it: a refusal or a request returned for want of
    /// a queue fails it, and a request that failed is forgotten.
    fn settle(
        self: &Arc<Self>,
        published: lapin::Result<PublisherConfirm>,
        call: Option<u64>,
        pattern: Pattern,
        answer: oneshot::Sender<Result<(), BoxError>>,
        confirming: &mut Confirming,
    ) {
        let side = Arc::clone(self);
        let confirm = match published {
            Ok(confirm) => confirm,
            Err(error) => {
                if let Some(call) = call {
                    lock(&side.calls).remove(&call);
                }
                let _ = answer.send(Err(error.into()));
                return;
            }
        };
        confirming.push(Box::pin(async move {
            let outcome = match confirm.await {
                Ok(confirmation) if confirmation.is_nack() => Err(format!("the RabbitMQ broker refused a frame on `{pattern}`").into()),
                // Only a request is published `mandatory`, so only a request comes back returned.
                Ok(confirmation) => match (call, confirmation.take_message()) {
                    (Some(_), Some(_)) => Err(BoxError::from(NoDestination { pattern: pattern.to_string() })),
                    _ => Ok(()),
                },
                Err(error) => Err(error.into()),
            };
            if outcome.is_err()
                && let Some(call) = call
            {
                lock(&side.calls).remove(&call);
            }
            let _ = answer.send(outcome);
        }));
    }

    /// Drops call `id`'s held `cancel` once the hold runs out, if no `opened` released it first.
    fn expire_cancelled(self: &Arc<Self>, id: u64, until: HoldUntil) {
        let side = Arc::downgrade(self);
        let hold = until.remaining(self.fallback_hold());
        self.runtime.handle().spawn(async move {
            tokio::time::sleep(hold).await;
            if let Some(side) = side.upgrade() {
                let mut calls = lock(&side.calls);
                if matches!(calls.get(&id), Some(ClientCall::Cancelled)) {
                    calls.remove(&id);
                }
            }
        });
    }

    /// How long a held `cancel` whose `open` carried no deadline is kept.
    fn fallback_hold(&self) -> Duration {
        #[cfg(feature = "conformance-probe")]
        return self.probe.hold();
        #[cfg(not(feature = "conformance-probe"))]
        ulo_rpc::link::CANCEL_HOLD
    }

    /// A streamed request's `in` or `in_end`: published once the server has acknowledged the
    /// `open`, held until then, and dropped for a call that has ended.
    async fn control(&self, id: u64, frame: Frame, confirming: &mut Confirming) {
        let held = {
            let mut calls = lock(&self.calls);
            match calls.get_mut(&id) {
                Some(ClientCall::Streaming { opened: false, held, .. }) => {
                    held.push_back(frame);
                    return;
                }
                Some(ClientCall::Streaming { opened: true, .. }) => frame,
                _ => return,
            }
        };
        self.publish_held(id, held, confirming).await;
    }

    /// The frames held for a streamed request the server has now acknowledged, which the writer
    /// publishes before anything queued after the acknowledgment.
    fn opened(&self, id: u64) -> VecDeque<Frame> {
        let mut calls = lock(&self.calls);
        match calls.get_mut(&id) {
            Some(ClientCall::Streaming { opened, held, .. }) if !*opened => {
                *opened = true;
                std::mem::take(held)
            }
            Some(ClientCall::Cancelled) => {
                calls.remove(&id);
                VecDeque::from([Frame::Cancel { id }])
            }
            _ => VecDeque::new(),
        }
    }

    /// Publishes one control frame of a streamed request; a failure is logged, as the call's
    /// caller has gone on.
    async fn publish_held(&self, call: u64, frame: Frame, confirming: &mut Confirming) {
        let (kind, body) = match frame {
            Frame::In { data, .. } => (IN, data.into_bytes()),
            Frame::InEnd { .. } => (IN_END, Bytes::new()),
            Frame::Cancel { .. } => (CANCEL, Bytes::new()),
            _ => return,
        };
        match self.publish_control(call, kind, &body).await {
            Ok(confirm) => confirming.push(Box::pin(async move {
                if let Err(error) = confirm.await {
                    tracing::debug!(%error, kind, "the RabbitMQ link could not publish a control frame");
                }
            })),
            Err(error) => tracing::debug!(%error, kind, "the RabbitMQ link could not publish a control frame"),
        }
    }

    fn correlation(&self, call: u64) -> String {
        format!("{}.{call}", self.id)
    }

    fn call_of(&self, correlation: &str) -> Option<u64> {
        correlation.strip_prefix(self.id.as_str())?.strip_prefix('.')?.parse().ok()
    }

    /// A `mandatory` publish through the default exchange: a queue named after the pattern takes
    /// it, and with none the broker returns it, which is the miss signal.
    async fn publish_request(&self, pattern: &Pattern, call: u64, headers: &CallHeaders, kind: Option<&str>, body: &[u8]) -> lapin::Result<PublisherConfirm> {
        let mut properties = BasicProperties::default().with_reply_to(REPLY_TO.into()).with_correlation_id(self.correlation(call).into());
        if let Some(table) = amqp_headers(headers, kind) {
            properties = properties.with_headers(table);
        }
        let options = BasicPublishOptions { mandatory: true, ..Default::default() };
        self.channel.basic_publish("".into(), pattern.as_str().into(), options, body, properties).await
    }

    async fn publish_control(&self, call: u64, kind: &str, body: &[u8]) -> lapin::Result<PublisherConfirm> {
        let mut properties = BasicProperties::default().with_correlation_id(self.correlation(call).into());
        if let Some(table) = amqp_headers(&CallHeaders::new(), Some(kind)) {
            properties = properties.with_headers(table);
        }
        self.channel.basic_publish(CONTROL.into(), "".into(), BasicPublishOptions::default(), body, properties).await
    }
}

/// Routes the direct reply-to deliveries until the consumer ends or fails; either ends the reply
/// lane, so `RpcClient` fails the calls waiting on it `Unavailable`. A streamed request's
/// `opened` goes to the writer, behind the frames already queued.
async fn route_replies(side: Arc<ClientSide>, mut replies: Consumer, frames: mpsc::UnboundedSender<Frame>, writes: ordered::Sender<Job>) {
    while let Some(delivery) = replies.next().await {
        let delivery = match delivery {
            Ok(delivery) => delivery,
            Err(error) => {
                tracing::warn!(%error, "the RabbitMQ link lost its reply consumer; the calls waiting on it fail");
                break;
            }
        };
        let Some(call) = delivery.properties.correlation_id().as_ref().and_then(|id| side.call_of(id.as_str())) else { continue };
        if kind_of(delivery.properties.headers().as_ref()).as_deref() == Some(OPENED) {
            #[cfg(feature = "conformance-probe")]
            if side.probe.withholds(call) {
                continue;
            }
            if writes.send(Job::Opened(call)).await.is_err() {
                break;
            }
            continue;
        }
        let frame = match side.codec.decode_frame(&delivery.data) {
            Ok(frame) => with_id(frame, call),
            Err(error) => {
                tracing::warn!(%error, "the RabbitMQ link dropped a reply that does not decode");
                continue;
            }
        };
        if is_terminal(&frame) {
            lock(&side.calls).remove(&call);
        }
        if frames.send(frame).is_err() {
            break;
        }
    }
}

/// AMQP headers from a call's headers. A field table is a map, so a name repeated in the call
/// keeps its last value.
fn amqp_headers(headers: &CallHeaders, kind: Option<&str>) -> Option<FieldTable> {
    if headers.is_empty() && kind.is_none() {
        return None;
    }
    let mut table = FieldTable::default();
    for (name, value) in headers.iter() {
        table.insert(name.into(), AMQPValue::LongString(value.into()));
    }
    if let Some(kind) = kind {
        table.insert(KIND.into(), AMQPValue::LongString(kind.into()));
    }
    Some(table)
}

fn header_text(value: &AMQPValue) -> Option<String> {
    match value {
        AMQPValue::LongString(text) => Some(String::from_utf8_lossy(text.as_bytes()).into_owned()),
        AMQPValue::ShortString(text) => Some(text.as_str().to_owned()),
        AMQPValue::Boolean(value) => Some(value.to_string()),
        AMQPValue::ShortShortInt(value) => Some(value.to_string()),
        AMQPValue::ShortShortUInt(value) => Some(value.to_string()),
        AMQPValue::ShortInt(value) => Some(value.to_string()),
        AMQPValue::ShortUInt(value) => Some(value.to_string()),
        AMQPValue::LongInt(value) => Some(value.to_string()),
        AMQPValue::LongUInt(value) => Some(value.to_string()),
        AMQPValue::LongLongInt(value) => Some(value.to_string()),
        _ => None,
    }
}

fn kind_of(headers: Option<&FieldTable>) -> Option<String> {
    headers?.inner().iter().find(|(name, _)| name.as_str() == KIND).and_then(|(_, value)| header_text(value))
}

/// The call's headers from the AMQP headers: text and scalar values as text; tables, arrays and
/// byte arrays, which a header string cannot carry, are left out.
fn call_headers(headers: Option<&FieldTable>) -> CallHeaders {
    let mut out = CallHeaders::new();
    if let Some(headers) = headers {
        for (name, value) in headers.inner() {
            if name.as_str() == KIND {
                continue;
            }
            if let Some(text) = header_text(value) {
                out.insert(name.as_str(), text);
            }
        }
    }
    out
}

/// The reply frame with the caller's own `id`, read from the `correlation_id`: the server wrote
/// its local id.
fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        other => other,
    }
}

fn is_terminal(frame: &Frame) -> bool {
    matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. })
}

fn receiver_stream<T: Send + 'static>(receiver: mpsc::UnboundedReceiver<T>) -> futures_core::stream::BoxStream<'static, T> {
    futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) }).boxed()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use ulo_rpc::{ErrorKind, RpcClient};

    use super::*;

    /// The server's writer stopped before it publishes a reply to a named queue, for a test to
    /// run `close` while the reply waits.
    pub(super) mod gate {
        use super::*;

        type Stop = (String, oneshot::Sender<()>, oneshot::Receiver<()>);

        static AT_PUBLISH: Mutex<Vec<Stop>> = Mutex::new(Vec::new());

        /// Stops the writer at its next reply to `reply`: the first receiver answers once it has
        /// stopped there, and the sender releases it.
        #[cfg_attr(not(feature = "integration"), allow(dead_code))]
        pub(crate) fn stop(reply: &str) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
            let (arrived, stopped) = oneshot::channel();
            let (release, released) = oneshot::channel();
            lock(&AT_PUBLISH).push((reply.to_owned(), arrived, released));
            (stopped, release)
        }

        pub(crate) async fn at_publish(reply: &str) {
            let stop = {
                let mut stops = lock(&AT_PUBLISH);
                stops.iter().position(|(stopped, _, _)| stopped == reply).map(|at| stops.remove(at))
            };
            if let Some((_, arrived, released)) = stop {
                let _ = arrived.send(());
                let _ = released.await;
            }
        }
    }

    /// Q21-P6: a client connect that panics fails the call waiting on it `Unavailable` holding the
    /// panic, so `ulo::is_panic` is true of the call's error, its message redacted. A panicking
    /// future stands in for `open_client` through `Connecting::start`, held at a gate until the
    /// call has joined the connect. Planted break: the panic answered as a plain error.
    #[tokio::test]
    async fn a_connect_that_panics_fails_the_call_holding_the_redacted_panic() {
        let runtime = Tokio::current();
        let clock: Arc<dyn Runtime> = Arc::new(runtime.clone());
        let link = RabbitMq::url("amqp://ulo:hunter2@127.0.0.1:1");
        let (release, released) = oneshot::channel::<()>();
        let opening = async move {
            let _ = released.await;
            panic!("connecting to amqp://ulo:hunter2@127.0.0.1:1");
        };
        let connecting = Connecting::start(&runtime, opening, CONNECT_WITHIN, Arc::clone(&clock));
        // The connect's own handle, its task's and this one: a call that joins adds a fourth.
        let outcome = Arc::clone(&connecting.outcome);
        lock(&link.state).connecting = Some(connecting);
        let client = RpcClient::new(link, clock).expect("the link is usable");
        let caller = client.clone();
        let calling = tokio::spawn(async move { caller.request::<_, u32>("panics.call", &()).await });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&outcome) < 4 {
            assert!(std::time::Instant::now() < deadline, "the call did not join the running connect within 5 s");
            tokio::task::yield_now().await;
        }
        release.send(()).expect("the connect waits at its gate");
        let failed = calling.await.expect("the call's task completes").expect_err("a call over a connect that panicked succeeded");
        assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, None), "{failed:?}");
        let message = failed.message().to_owned();
        assert!(message.contains("connect panicked: connecting to amqp://[redacted]@127.0.0.1:1"), "the call does not carry the panic: {message}");
        assert!(!message.contains("hunter2"), "the call repeats the url's password: {message}");
        assert!(ulo::is_panic(&BoxError::from(failed)), "the call's error does not hold the panic: {message}");

        // The panicked connect is not joined: the next call starts its own, which the closed port
        // refuses as an ordinary error.
        let next = client.request::<_, u32>("panics.call", &()).await.expect_err("nothing listens on port 1");
        assert!(!ulo::is_panic(&BoxError::from(next)), "the next call joined the connect that panicked");
    }

    /// A connect `close` cuts short ends with its task: the opening future is dropped, not left
    /// running beside a closed link. Planted break: the opening task not aborted with the task
    /// holding its outcome.
    #[tokio::test]
    async fn close_drops_the_opening_it_cuts_short() {
        struct Flag(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let runtime = Tokio::current();
        let clock: Arc<dyn Runtime> = Arc::new(runtime.clone());
        let link = RabbitMq::url("amqp://127.0.0.1:1");
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Flag(Arc::clone(&dropped));
        let opening = async move {
            let _held = flag;
            std::future::pending::<lapin::Result<Opened>>().await
        };
        lock(&link.state).connecting = Some(Connecting::start(&runtime, opening, CONNECT_WITHIN, clock));
        tokio::task::yield_now().await;
        link.close().await.expect("the link closes");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dropped.load(Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline, "the opening `close` cut short was still held 5 s later");
            tokio::task::yield_now().await;
        }
    }

    /// F522 (Q22-P8): a server's `close` publishes the answers queued before it, then closes the
    /// connection, from a task its own future does not hold, ending at `CUT_GRACE` on the app's
    /// clock. Each test builds a server side on a connection to a broker in a container, holds a
    /// call whose reply queue a caller consumes on a connection of its own, and stops the writer
    /// before it publishes the call's answer.
    #[cfg(feature = "integration")]
    mod cut_close {
        use std::net::{Ipv4Addr, SocketAddr};
        use std::time::Instant;

        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{ContainerAsync, GenericImage};
        use ulo_rpc::{Details, ErrorBody};
        use ulo_rpc_conformance::relay::{reachable, unshadowed};
        use ulo_rpc_conformance::{report, startup_failed};

        use super::*;

        async fn rabbitmq() -> (AMQPUri, ContainerAsync<GenericImage>) {
            let (container, addrs) = unshadowed("the RabbitMQ container", || async {
                let container = GenericImage::new("rabbitmq", "4.1-alpine")
                    .with_exposed_port(5672.tcp())
                    .with_wait_for(WaitFor::message_on_stdout("Server startup complete"))
                    .start()
                    .await
                    .unwrap_or_else(|error| startup_failed!("the RabbitMQ container did not start: {}", report(&error)));
                let port = container
                    .get_host_port_ipv4(5672)
                    .await
                    .unwrap_or_else(|error| startup_failed!("the AMQP port is not mapped: {}", report(&error)));
                (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
            })
            .await;
            reachable(addrs[0], Duration::from_secs(10)).await;
            (uri(&format!("amqp://guest:guest@{}/%2f", addrs[0])).expect("the url parses"), container)
        }

        /// A clock that moves only when the test advances it, and records each sleep asked of it.
        #[derive(Clone, Default)]
        struct Scripted(Arc<Mutex<Script>>);

        #[derive(Default)]
        struct Script {
            elapsed: Duration,
            asked: Vec<Duration>,
            waiting: Vec<std::task::Waker>,
        }

        impl Scripted {
            fn advance(&self, by: Duration) {
                let waiting = {
                    let mut script = lock(&self.0);
                    script.elapsed += by;
                    std::mem::take(&mut script.waiting)
                };
                waiting.into_iter().for_each(std::task::Waker::wake);
            }

            fn asked(&self) -> Vec<Duration> {
                lock(&self.0).asked.clone()
            }
        }

        impl Timer for Scripted {
            fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
                let sleeping = self.try_sleep(d);
                Box::pin(async move {
                    let _ = sleeping.await;
                })
            }

            fn now(&self) -> Instant {
                Instant::now()
            }

            fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
                let script = Arc::clone(&self.0);
                let until = {
                    let mut script = lock(&script);
                    script.asked.push(d);
                    script.elapsed + d
                };
                Box::pin(std::future::poll_fn(move |cx| {
                    let mut script = lock(&script);
                    if script.elapsed >= until {
                        return Poll::Ready(Ok(()));
                    }
                    script.waiting.push(cx.waker().clone());
                    Poll::Pending
                }))
            }
        }

        /// A server's link holding a call whose answer goes to `reply`, which a caller consumes,
        /// and the call's reply path, with the writer stopped before the answer's publish.
        struct Held {
            link: RabbitMq,
            path: ReplyPath,
            status: lapin::ConnectionStatus,
            replies: Consumer,
            _caller: Connection,
            stopped: oneshot::Receiver<()>,
            release: oneshot::Sender<()>,
            /// Answers once the task standing in for a pattern consumer has been aborted.
            consumer_gone: oneshot::Receiver<()>,
        }

        /// Sends on its sender when dropped, as an aborted task drops what its future holds.
        struct OnDrop(Option<oneshot::Sender<()>>);

        impl Drop for OnDrop {
            fn drop(&mut self) {
                if let Some(gone) = self.0.take() {
                    let _ = gone.send(());
                }
            }
        }

        async fn held(uri: &AMQPUri, reply: &str, clock: &Scripted) -> Held {
            let caller = Connection::connect_uri(uri.clone(), ConnectionProperties::default()).await.expect("the caller connects");
            let consuming = caller.create_channel().await.expect("the caller's channel opens");
            consuming
                .queue_declare(reply.into(), QueueDeclareOptions { exclusive: true, ..Default::default() }, FieldTable::default())
                .await
                .expect("the reply queue is declared");
            let replies = consuming
                .basic_consume(reply.into(), "".into(), BasicConsumeOptions { no_ack: true, ..Default::default() }, FieldTable::default())
                .await
                .expect("the caller consumes its reply queue");

            let connection = Connection::connect_uri(uri.clone(), ConnectionProperties::default()).await.expect("the server connects");
            let channel = connection.create_channel().await.expect("the server's channel opens");
            let status = connection.status().clone();
            let runtime = Tokio::current();
            let (writes, queued) = ordered::channel(WRITE_QUEUE);
            let writer = runtime.handle().spawn(server_writer(channel.clone(), queued)).abort_handle();
            let (gone, consumer_gone) = oneshot::channel();
            let held = OnDrop(Some(gone));
            let consumer = runtime
                .handle()
                .spawn(async move {
                    let _held = held;
                    std::future::pending::<()>().await;
                })
                .abort_handle();
            let side = Arc::new(ServerSide {
                connection,
                channel,
                writes,
                writer,
                timer: Arc::new(clock.clone()),
                codec: Codec::Json,
                runtime,
                calls: Arc::new(Calls::new()),
                phase: watch::channel(Phase::Serving).0,
                tags: Vec::new(),
                deliveries: Mutex::new(None),
            });
            let (_id, path) = side.hold("call".to_owned(), reply.to_owned(), Some("call".to_owned()));
            let link = RabbitMq::url("amqp://127.0.0.1:1");
            lock(&link.state).server = Some((side, vec![consumer]));
            let (stopped, release) = gate::stop(reply);
            Held { link, path, status, replies, _caller: caller, stopped, release, consumer_gone }
        }

        fn unavailable() -> Frame {
            Frame::Err { id: 1, error: ErrorBody::new(ErrorKind::Unavailable, "shutting down", Details::new()) }
        }

        /// The writer stopping at its gate, within 10 s: a publish that never comes fails the
        /// test rather than holding it.
        async fn stops(stopped: oneshot::Receiver<()>, what: &str) {
            match tokio::time::timeout(Duration::from_secs(10), stopped).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => panic!("{what}: the gate was dropped"),
                Err(_) => panic!("{what}: the writer published nothing for 10 s"),
            }
        }

        /// The caller reads the call's answer, within 10 s.
        async fn answered(replies: &mut Consumer) -> Frame {
            let delivery = tokio::time::timeout(Duration::from_secs(10), replies.next())
                .await
                .expect("the caller read no answer within 10 s")
                .expect("the caller's consumer ended")
                .expect("the caller's consumer failed");
            Codec::Json.decode_frame(&delivery.data).expect("the answer decodes")
        }

        /// The consumer's task aborted, within 10 s.
        async fn consumer_aborted(consumer_gone: oneshot::Receiver<()>) {
            tokio::time::timeout(Duration::from_secs(10), consumer_gone)
                .await
                .expect("the consumer's task was still running 10 s after the close ended")
                .expect("the consumer's task ended unaborted");
        }

        async fn until_closed(status: &lapin::ConnectionStatus) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !status.closed() {
                assert!(Instant::now() < deadline, "the server's connection was still open 10 s after it was let go");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        /// A `close` the writer holds up, its connection's close waiting for the writer to publish
        /// the answer queued before it: the close is still running 2 s after it began, the caller
        /// reads the answer once the writer goes on, and the connection closes after it. Planted
        /// break: the connection closed in `close` itself, ahead of the writer, which loses the
        /// answer.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_close_publishes_the_answers_queued_before_it_and_then_closes() {
            let (uri, _container) = rabbitmq().await;
            let clock = Scripted::default();
            let Held { link, path, status, mut replies, _caller, stopped, release, consumer_gone } = tokio::time::timeout(Duration::from_secs(30), held(&uri, "cut.answered", &clock)).await.expect("the server side and its caller were not set up within 30 s");
            drop(path.send(unavailable()));
            stops(stopped, "the writer stops before the answer's publish").await;

            let link = Arc::new(link);
            let closing = tokio::spawn({
                let link = Arc::clone(&link);
                async move { link.close().await }
            });
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(!closing.is_finished(), "the close ended while the answer queued before it was unpublished");
            assert!(!status.closed(), "the connection closed while the answer queued before it was unpublished");
            release.send(()).expect("the writer waits at its gate");
            let Frame::Err { error, .. } = answered(&mut replies).await else { panic!("the caller read an answer other than an `err`") };
            assert_eq!(error.kind, ErrorKind::Unavailable, "the answer the caller read");
            tokio::time::timeout(Duration::from_secs(10), closing)
                .await
                .expect("the close outlived the answer's publish by 10 s")
                .expect("the close's task")
                .expect("the close ends clean");
            assert!(status.closed(), "the close ended with the connection open");
            consumer_aborted(consumer_gone).await;
            assert_eq!(clock.asked(), [CUT_GRACE], "the sleeps the close asked of the app's clock");
        }

        /// A `close` polled once and dropped, as a spent shutdown cap leaves it, with the answer
        /// queued before it unpublished: the caller reads the answer once the writer goes on, and
        /// the connection closes after it, the task outliving the close.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_close_cut_at_its_first_poll_still_publishes_its_answers_and_closes() {
            let (uri, _container) = rabbitmq().await;
            let clock = Scripted::default();
            let Held { link, path, status, mut replies, _caller, stopped, release, consumer_gone: _ } = tokio::time::timeout(Duration::from_secs(30), held(&uri, "cut.short", &clock)).await.expect("the server side and its caller were not set up within 30 s");
            drop(path.send(unavailable()));
            drop(path);
            stops(stopped, "the writer stops before the answer's publish").await;

            {
                let mut close = pin!(link.close());
                assert!(futures_util::poll!(close.as_mut()).is_pending(), "the close ended while the writer held the answer");
            }
            drop(link);
            release.send(()).expect("the writer waits at its gate");
            let Frame::Err { error, .. } = answered(&mut replies).await else { panic!("the caller read an answer other than an `err`") };
            assert_eq!(error.kind, ErrorKind::Unavailable, "the answer the caller read");
            until_closed(&status).await;
        }

        /// A `close` cut at its first poll answers the call it holds, which the server has not
        /// answered, `unavailable` with the shutdown phrase, the answer published once the writer
        /// goes on; a reply the call sends after it is refused, and the connection closes. Planted
        /// break: the held calls dropped without an answer, the caller left to its own `Timeout`.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_close_cut_at_its_first_poll_answers_the_calls_it_holds() {
            let (uri, _container) = rabbitmq().await;
            let clock = Scripted::default();
            let Held { link, path, status, mut replies, _caller, stopped, release, consumer_gone: _ } = tokio::time::timeout(Duration::from_secs(30), held(&uri, "cut.unanswered", &clock)).await.expect("the server side and its caller were not set up within 30 s");

            {
                let mut close = pin!(link.close());
                assert!(futures_util::poll!(close.as_mut()).is_pending(), "the close ended while the writer held its answer");
            }
            drop(link);
            stops(stopped, "the writer stops before the close's answer is published").await;
            let late = tokio::time::timeout(Duration::from_secs(10), path.send(unavailable())).await.expect("a reply sent after the close waited 10 s");
            assert!(late.is_err(), "a reply sent after the close was taken");
            drop(path);
            release.send(()).expect("the writer waits at its gate");
            let Frame::Err { id, error } = answered(&mut replies).await else { panic!("the caller read an answer other than an `err`") };
            assert_eq!((id, error.kind, error.message.as_str()), (1, ErrorKind::Unavailable, SHUTDOWN_MESSAGE), "the answer the caller read");
            until_closed(&status).await;
        }

        /// A writer that never publishes holds the connection until `CUT_GRACE` on the app's clock
        /// and no longer: a millisecond short of it the close is still running and the connection
        /// open; at it the close fails naming the grace, and the connection closes, the writer
        /// aborted with it. Planted break: no grace, the close waiting on the writer.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_close_whose_writer_never_publishes_ends_at_the_grace() {
            let (uri, _container) = rabbitmq().await;
            let clock = Scripted::default();
            let Held { link, path, status, replies: _replies, _caller, stopped, release: _release, consumer_gone } = tokio::time::timeout(Duration::from_secs(30), held(&uri, "cut.grace", &clock)).await.expect("the server side and its caller were not set up within 30 s");
            drop(path.send(unavailable()));
            stops(stopped, "the writer stops before the answer's publish").await;

            let link = Arc::new(link);
            let closing = tokio::spawn({
                let link = Arc::clone(&link);
                async move { link.close().await }
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while clock.asked().is_empty() {
                assert!(Instant::now() < deadline, "the close asked the app's clock for no grace within 10 s");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(clock.asked(), [CUT_GRACE], "the sleeps the close asked of the app's clock");
            clock.advance(CUT_GRACE - Duration::from_millis(1));
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(!closing.is_finished() && !status.closed(), "the close let go of the connection short of the grace");
            clock.advance(Duration::from_millis(1));
            let failed = tokio::time::timeout(Duration::from_secs(10), closing)
                .await
                .expect("the close outlived the grace by 10 s")
                .expect("the close's task")
                .expect_err("a close whose writer never published ended clean");
            assert!(failed.to_string().contains("`CUT_GRACE`"), "the failure does not name the grace: {failed}");
            until_closed(&status).await;
            consumer_aborted(consumer_gone).await;
        }
    }

}
