use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
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
use ulo::{AppHandle, BoxError, BoxFuture, Cut, Permanent, Refused, Runtime, RuntimeShutDown, Timer};
use ulo_runtime_tokio::Tokio;
#[cfg(feature = "conformance-probe")]
use ulo_rpc::__private::ClientProbe;
use ulo_rpc::__private::HoldUntil;
use ulo_rpc::link::Inbound;
use ulo_transport::__private::ordered;
use ulo_transport::Count;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, ConnectTimedOut, Data, Delivery, DeliveryMode, Frame, Link, NoDestination, NoTimer,
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
    /// `close` ran while the connect was running or before its connection was taken.
    Closed,
}

impl Connecting {
    fn start(runtime: &Tokio, uri: AMQPUri, within: Duration, clock: Arc<dyn Runtime>) -> Connecting {
        let (answered, done) = watch::channel(false);
        let outcome = Arc::new(Mutex::new(None));
        let written = Arc::clone(&outcome);
        let task = runtime.handle().spawn(async move {
            let answer = match ulo::timeout(&*clock, within, open_client(uri)).await {
                Ok(Ok(opened)) => Outcome::Opened(Box::new(opened)),
                Ok(Err(error)) => Outcome::Failed(Failure::Lapin(error)),
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
    /// `reason: "runtime_shut_down"`. A zero bound is refused in `usable`, so in a server's
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
        runtime.handle().spawn(server_writer(channel.clone(), queued));
        let side = Arc::new(ServerSide {
            connection,
            channel,
            writes,
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
                        let connecting = Connecting::start(&runtime, uri.clone(), self.connect_within, Arc::clone(client));
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
                // The task ended unanswered: its runtime dropped it, having shut down, or it
                // panicked.
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
        if let Some((side, _)) = server {
            side.phase.send_replace(Phase::Closed);
            side.calls.clear();
            lock(&side.deliveries).take();
            let closing = Arc::clone(&side);
            if let Err(error) = close_on(&side.runtime, async move { closing.connection.close(200, "close".into()).await }).await {
                failure = Some(error);
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
    writes: ordered::Sender<Reply>,
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
            Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
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
        // Queued at the call, so replies go out in the order their sends were called.
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            if is_terminal(&frame) {
                calls.release(&released);
            }
            let written = codec.encode_frame(&frame).map(|bytes| write(&writes, reply.clone(), correlation.clone(), None, bytes));
            Box::pin(async move { written?.await })
        });
        let id = self.calls.hold(key, path.clone());
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

/// Publishes the server's replies one after another, in the order they were queued, until every
/// sender is gone. The server's channel is not in confirm mode, so each publish is done once lapin
/// has written it.
async fn server_writer(channel: Channel, mut queued: ordered::Receiver<Reply>) {
    while let Some(Reply { reply, correlation, kind, body, answer }) = queued.recv().await {
        let _ = answer.send(publish_reply(&channel, reply, correlation, kind, &body).await);
    }
}

/// Queues one message on the server's writer at the call and answers a wait for it to be
/// published.
fn write(
    writes: &ordered::Sender<Reply>,
    reply: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
) -> impl Future<Output = Result<(), BoxError>> + Send + 'static {
    let (answer, answered) = oneshot::channel();
    let room = writes.send(Reply { reply, correlation, kind, body, answer });
    async move {
        let gone = || BoxError::from("the RabbitMQ link's server side is closed");
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

/// The calls a server holds, by the key their control messages carry, each with its local id and
/// its reply path. The local id is this link's own, unique across every caller.
struct Calls {
    next: AtomicU64,
    held: Mutex<HashMap<String, (u64, ReplyPath)>>,
    count: watch::Sender<usize>,
}

impl Calls {
    fn new() -> Self {
        Calls { next: AtomicU64::new(1), held: Mutex::new(HashMap::new()), count: watch::channel(0).0 }
    }

    fn hold(&self, key: String, path: ReplyPath) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut held = lock(&self.held);
        held.insert(key, (id, path));
        self.count.send_replace(held.len());
        id
    }

    fn get(&self, key: &str) -> Option<(u64, ReplyPath)> {
        lock(&self.held).get(key).cloned()
    }

    fn release(&self, key: &str) -> Option<(u64, ReplyPath)> {
        let mut held = lock(&self.held);
        let call = held.remove(key);
        self.count.send_replace(held.len());
        call
    }

    /// Drops every held path, each of which holds this table through its closure.
    fn clear(&self) {
        lock(&self.held).clear();
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
        ulo_rpc::__private::CANCEL_HOLD
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
