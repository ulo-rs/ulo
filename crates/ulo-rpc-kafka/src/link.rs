use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer, StreamConsumer};
use rdkafka::error::KafkaError;
use rdkafka::message::{Header, Headers, Message, OwnedHeaders, OwnedMessage};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::types::RDKafkaErrorCode;
use rdkafka::util::Timeout;
use rdkafka::{Offset, TopicPartitionList};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use ulo::{AppHandle, BoxError, BoxFuture, Cut, Permanent, Refused, Runtime, RuntimeShutDown, Timer};
use ulo_runtime_tokio::Tokio;
#[cfg(feature = "conformance-probe")]
use ulo_rpc::link::ClientProbe;
use ulo_rpc::link::{HoldUntil, Inbound, TaskPanics};
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, ConnectTimedOut, Data, Delivery, DeliveryMode, Frame, FrameTooLarge, Instances, Link, NoDestination, NoTimer,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo, Subscription,
};
use ulo_transport::sync::ordered;
use ulo_transport::Count;

/// The header naming a record's frame kind where the record alone does not tell it: `open` on a
/// pattern's topic, `in`, `in_end` and `cancel` on the control topic, `opened` on the reply topic.
/// A record on a pattern's topic without it is a `req` when it carries `ulo-reply-to` and an `evt`
/// when it does not.
const KIND: &str = "ulo-t";
const REPLY_TO: &str = "ulo-reply-to";
const CORRELATION: &str = "ulo-correlation-id";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The topic every server instance reads from its end, outside the consumer group, so each sees
/// every control record and acts on the ones whose correlation id names a call it holds. A
/// streamed request's items and every `cancel` travel here: the group hands a pattern's partition
/// to one instance, and these have to reach the instance that took the call.
const CONTROL: &str = "ulo.rpc.control";

/// The producer's `message.max.bytes`, librdkafka's default, written out so the link can declare
/// it as its `max_frame`. A broker configured lower refuses with `MSG_SIZE_TOO_LARGE`, which the
/// link reports the same way.
const MAX_MESSAGE: u64 = 1_000_000;

/// How long librdkafka keeps trying to deliver one record, in place of its five-minute default.
const DELIVERY_TIMEOUT: &str = "30000";

/// How long a record waits for room in librdkafka's queue before its produce fails.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

/// Frames queued for one side's writer ahead of the next send's wait: each send is queued at its call and waits until it is
/// among this many the writer has not taken.
const WRITE_QUEUE: usize = 64;

/// How long a server's `listen` may take, unless [`Kafka::connect_within`] says otherwise.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// How often a server reads the partition count of each topic it reads every partition of, the
/// interval at which librdkafka refreshes its own metadata, `topic.metadata.refresh.interval.ms`.
const METADATA_REFRESH: Duration = Duration::from_secs(300);

/// The bound on each metadata and watermark read an own consumer's assignment makes.
const METADATA_TIMEOUT: Duration = Duration::from_secs(10);

/// The Kafka link.
///
/// Each side produces through one writer task, fed in order by every send on it, so the records a
/// side sends are queued with librdkafka in the order they were sent: a request's `in` items
/// before its `in_end`, and a `cancel` after the request it names. A request's topic and the
/// control topic are separate, so the broker can still deliver a `cancel` ahead of its request.
///
/// The link's clients and tasks live on the tokio runtime it holds, the one current where it was
/// built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may be
/// polled on any executor or on a plain thread. A link built outside a runtime and given none
/// refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
///
/// A server's `listen` returns once every consumer of its own is assigned, so a control record or
/// an event declared `every_instance` produced after `bind` returns is read, and fails as `Bind`,
/// with [`ConnectTimedOut`](ulo_rpc::ConnectTimedOut), when it has not returned within
/// [`connect_within`](Self::connect_within).
///
/// A link dropped without `close`, as a server's is when its app drops before the close step,
/// aborts the tasks reading its consumers and its writers, so each consumer leaves its group and
/// its connections close, on a thread of its own as at `close`.
///
/// A panic in one of the link's tasks, a request lane, the control lane, a writer, the partition
/// watcher or the reply router, ends a server as a transport failure, the app's shutdown outcome
/// naming the link and the task and holding the panic redacted with the app's secrets; on a
/// client it fails the calls waiting on the connection `Unavailable` holding the panic, and the
/// next call connects again ([`TaskPanics`](ulo_rpc::link::TaskPanics)).
pub struct Kafka {
    pub(crate) brokers: String,
    pub(crate) runtime: Option<Tokio>,
    pub(crate) group: Option<String>,
    pub(crate) reply_topic: Option<String>,
    pub(crate) codec: Codec,
    pub(crate) partitions: i32,
    pub(crate) replication: i32,
    pub(crate) connect_within: Duration,
    /// Awaited immediately before the consumers of the server's own are assigned, the last step
    /// of `listen`, with the topics they are assigned, its error failing `listen`: [`no_hold`],
    /// which a test replaces with one that holds the assignment or refuses it.
    pub(crate) before_assign: fn(&[&str]) -> BoxFuture<'static, Result<(), BoxError>>,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The server's in-flight bound, from `Link::max_inflight`; `None` for no bound.
    pub(crate) max_inflight: Option<usize>,
    /// The app's clock, from `prepare`, which times the anchor's retries.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    /// The client's producer, made in `connect` after its reply consumer: [`producer`], which a
    /// test replaces.
    pub(crate) client_producer: fn(&ClientConfig) -> Result<FutureProducer, BoxError>,
    /// The app's runtime, read by `prepare`, which redacts a server task's panic with the app's
    /// secrets when it is logged.
    pub(crate) app_runtime: Option<Arc<dyn Runtime>>,
    pub(crate) state: Mutex<State>,
    #[cfg(feature = "conformance-probe")]
    pub(crate) probe: Arc<ClientProbe>,
    /// The task [`Kafka::panic_in`] names, and what it panics with.
    #[cfg(feature = "conformance-probe")]
    pub(crate) planted: Option<ulo_rpc::link::PlantedPanic>,
    /// The link's watched tasks whose futures live, read through [`Kafka::live_tasks`].
    #[cfg(feature = "conformance-probe")]
    pub(crate) live_tasks: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Default)]
pub(crate) struct State {
    /// The server side and the tasks reading its consumers.
    server: Option<(Arc<ServerSide>, Vec<AbortHandle>)>,
    client: Option<Arc<ClientSide>>,
}

/// A link dropped without `close` aborts the tasks reading its consumers and its writers, which
/// `close` lets finish: an `AbortHandle` or a `JoinHandle` dropped leaves its task running, and
/// each holds its side, so its consumers stay in
/// their groups and its producers connected until the process ends. Each consumer is `Detached`,
/// so its drop runs on a thread of its own; the client's producer is dropped on one too, as
/// `close` drops it.
impl Drop for State {
    fn drop(&mut self) {
        if let Some((_, tasks)) = &self.server {
            tasks.iter().for_each(AbortHandle::abort);
        }
        if let Some(side) = &self.client {
            if let Some(routing) = lock(&side.routing).take() {
                routing.abort();
            }
            if let Some(writing) = lock(&side.writing).take() {
                writing.abort();
            }
            if let Some(producer) = lock(&side.producer).take() {
                drop(drop_detached(producer));
            }
        }
    }
}

impl Kafka {
    /// The link on `brokers`, a comma-separated `host:port` list, connected lazily. An entry
    /// written `ssl://host:port` selects `security.protocol=SSL`, which the crate's `tls` feature
    /// compiles in.
    pub fn brokers(brokers: impl Into<String>) -> Self {
        Kafka {
            brokers: brokers.into(),
            runtime: Tokio::try_current(),
            group: None,
            reply_topic: None,
            codec: Codec::Json,
            partitions: 1,
            replication: 1,
            connect_within: CONNECT_WITHIN,
            before_assign: no_hold,
            default_group: None,
            max_inflight: Count::Default.max_inflight(),
            timer: None,
            client_producer: producer,
            app_runtime: None,
            state: Mutex::new(State::default()),
            #[cfg(feature = "conformance-probe")]
            probe: Arc::default(),
            #[cfg(feature = "conformance-probe")]
            planted: None,
            #[cfg(feature = "conformance-probe")]
            live_tasks: Arc::default(),
        }
    }

    /// The client side as the RPC conformance suite reads it. Built by the `conformance-probe`
    /// feature, which is for the conformance suite, not an application feature.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn probe(&self) -> &ClientProbe {
        &self.probe
    }

    /// Makes the link's task `planted` names, `request lane`, `control lane`, `writer`, `partition
    /// watcher` or `reply router`, panic once `planted` fires. For the link's own tests, under the
    /// `conformance-probe` feature.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn panic_in(mut self, planted: ulo_rpc::link::PlantedPanic) -> Self {
        self.planted = Some(planted);
        self
    }

    /// How many of the link's lanes, writers, reply routers, partition watchers and event loops
    /// have a future that has not yet dropped, on both sides. For the link's own tests, under the
    /// `conformance-probe` feature.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn live_tasks(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        Arc::clone(&self.live_tasks)
    }

    /// Where one side's tasks report a panic: redacted on `runtime` when it is logged.
    fn panics(&self, runtime: Arc<dyn Runtime>) -> TaskPanics {
        let panics = TaskPanics::new("Kafka", runtime);
        #[cfg(feature = "conformance-probe")]
        panics.plant(self.planted.clone());
        #[cfg(feature = "conformance-probe")]
        panics.count_into(Arc::clone(&self.live_tasks));
        panics
    }

    /// The consumer group server instances share, in place of the root module's full type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The client's reply topic, one per logical client and reused across restarts; unset, one per
    /// process.
    pub fn reply_topic(mut self, topic: impl Into<String>) -> Self {
        self.reply_topic = Some(topic.into());
        self
    }

    /// `Codec::Cbor` carries raw bytes and reports `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// Partitions for each topic the link creates, a handler's topic at `bind` and the client's
    /// reply topic; 1 unset. A topic that already exists keeps its own.
    pub fn topic_partitions(mut self, partitions: i32) -> Self {
        self.partitions = partitions;
        self
    }

    /// The replication factor for each topic the link creates; 1 unset. A topic that already
    /// exists keeps its own.
    pub fn replication_factor(mut self, replication: i32) -> Self {
        self.replication = replication;
        self
    }

    /// How long a server's `listen` may take, 30 seconds by default, timed on the app's `Timer`:
    /// the topics' creation, the starting commit, the consumers' creation and the assignment of
    /// each consumer of the server's own. A `listen` still running at the bound fails as `Bind`
    /// with [`ConnectTimedOut`](ulo_rpc::ConnectTimedOut) naming the link and its brokers; a
    /// `Timer` whose runtime has shut down fails it as `Permanent` `RuntimeShutDown`. A zero bound
    /// is refused by `prepare`. A client's connect is not bounded by it.
    pub fn connect_within(mut self, bound: Duration) -> Self {
        self.connect_within = bound;
        self
    }

    /// The tokio runtime the link's clients and tasks run on, in place of the one current where
    /// it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the Kafka link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

/// Why a `cancel` can reach the server ahead of its request.
const CONTROL_LANE: &str = "the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first";

impl Link for Kafka {
    const NAME: &'static str = "kafka";

    /// Each comma-separated broker, as the setting is read.
    fn endpoint(&self) -> Option<String> {
        Some(self.brokers.split(',').map(|broker| ulo::secrets::setting_url(broker.trim())).collect::<Vec<_>>().join(","))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing)
            .codec(self.codec)
            .ordering(Order::PerPartition)
            .max_frame(Some(MAX_MESSAGE))
            .without_miss_signal(
                "the broker's `auto.create.topics.enable`, on by default, creates the topic a request to an unhandled pattern names, and the \
                 caller sees its own `Timeout`",
            )
            .without_ordered_control(CONTROL_LANE)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.usable()?;
        Brokers::parse(&self.brokers)?;
        if self.partitions < 1 || self.replication < 1 {
            return Err(format!(
                "the Kafka link's `topic_partitions` ({}) and `replication_factor` ({}) must each be at least 1",
                self.partitions, self.replication
            )
            .into());
        }
        if self.connect_within.is_zero() {
            return Err("the Kafka link's `connect_within` is zero, so no server could bind within it".into());
        }
        match &self.group {
            Some(group) if group.is_empty() => return Err("the Kafka link's group is empty".into()),
            Some(_) => {}
            None => self.default_group = Some(format!("{:#}", app.root().name())),
        }
        self.timer = app.timer().cloned();
        self.app_runtime = app.runtime().cloned();
        // `listen` connects on the link's runtime: one that has shut down is refused here, beside
        // the configuration, before any server binds.
        self.runtime()?.run(async {}).await.map_err(Permanent::new)?;
        Ok(())
    }

    /// The bound at which the request lane pauses its partitions, so a request over it waits in
    /// its topic: `Count::max_inflight`'s reading, 1,024 at `Default`.
    fn max_inflight(&mut self, calls: Count) {
        self.max_inflight = calls.max_inflight();
    }

    /// A pattern reaching one instance is a topic of the group's consumer, which Kafka assigns
    /// each partition of to one member. Those reaching every instance, events declared
    /// `every_instance`, are read by a consumer of this instance's own, assigned every partition of
    /// them at its end outside any group's rebalancing, as the control consumer is: its
    /// `group.id`, `<group>.every.<uuid>`, is this instance's and commits nothing, so the broker
    /// keeps no group or offset for it. Each partition's end is read before the assignment and
    /// the consumer assigned that offset (`own_consumer`), and `listen` returns once both
    /// consumers are assigned, its last step, so a record produced after `bind` returns is read.
    ///
    /// A partition added to such a topic after `bind` is not read: an event produced there reaches
    /// none of the instances already listening, and one started later reads it from its end. Each
    /// instance reads the partition count of the topics it reads every partition of every five
    /// minutes, librdkafka's metadata refresh interval, and logs a `warn` naming the topic and both
    /// counts when it has changed (`watch_partitions`).
    async fn listen(&self, subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the Kafka link's group is unset: `listen` ran before `prepare`")?;
        let runtime = self.runtime()?;
        let brokers = Brokers::parse(&self.brokers)?;
        let base = brokers.config();
        let timer = self.timer.clone().ok_or(NoTimer { link: "Kafka" })?;
        let timed_out = ConnectTimedOut::new(self.connect_within, brokers.list, "Kafka");
        let (every, shared): (Vec<&Subscription>, Vec<&Subscription>) =
            subscriptions.iter().partition(|subscription| subscription.instances == Instances::Every);
        let topics: Vec<String> = shared.iter().map(|subscription| subscription.pattern.as_str().to_owned()).collect();
        let own: Vec<String> = every.iter().map(|subscription| subscription.pattern.as_str().to_owned()).collect();
        let (partitions, replication, before_assign) = (self.partitions, self.replication, self.before_assign);
        let anchoring = Arc::clone(&timer);
        let connecting = runtime.run(async move {
            let timer = anchoring;
            // Created here, not on the first produce, so a stopped server's topic still exists
            // and a miss is the caller's `Timeout` whether or not the broker auto-creates
            // topics; the consumer also gets its partitions at once rather than after a
            // metadata refresh. The admin API refuses an empty list as an invalid argument, and a
            // server whose patterns are all events declared `every_instance` has no group topic.
            for list in [&topics, &own] {
                if !list.is_empty() {
                    create_topics(&base, list, partitions, replication).await?;
                }
            }
            create_topics(&base, &[CONTROL.to_owned()], 1, replication).await?;
            anchor(&base, &group, &topics, &timer).await?;

            let mut config = base.clone();
            config
                .set("group.id", &group)
                .set("enable.auto.commit", "true")
                .set("enable.auto.offset.store", "false")
                .set("auto.offset.reset", "latest");
            let consumer: Arc<Detached<StreamConsumer>> = Arc::new(Detached::new(config.create()?));
            let names: Vec<&str> = topics.iter().map(String::as_str).collect();
            // librdkafka refuses an empty subscription as an invalid argument too.
            if !names.is_empty() {
                consumer.subscribe(&names)?;
            }

            let every = if own.is_empty() {
                None
            } else {
                Some(own_consumer(&base, format!("{group}.every.{}", uuid::Uuid::new_v4().simple()), own).await?)
            };
            let control = own_consumer(&base, format!("{group}.control.{}", uuid::Uuid::new_v4().simple()), vec![CONTROL.to_owned()]).await?;
            // Detached from its creation, so its drop, wherever the writer ends or the runtime
            // drops it, runs on a thread of its own rather than on a worker.
            let producer = Detached::new(producer(&base)?);
            // Assigned last, so `listen` returns as soon as both are reading.
            let assigned: Vec<&str> = every.iter().chain([&control]).flat_map(|own| own.topics.iter().map(|(topic, _)| topic.as_str())).collect();
            before_assign(&assigned).await?;
            let every = every.map(Unassigned::assign).transpose()?;
            let control = control.assign()?;
            Ok::<_, BoxError>((consumer, every, control, producer))
        });
        // Dropping `connecting` at the bound aborts its task; a blocking librdkafka call it was
        // waiting on ends on its own thread within that call's timeout.
        let (consumer, every, control, producer) = match ulo::timeout(&*timer, timed_out.within(), connecting).await {
            Ok(connected) => connected.map_err(Permanent::new)??,
            Err(Cut::Expired) => return Err(timed_out.into()),
            Err(Cut::Refused(refused)) => return Err(Permanent::new(RuntimeShutDown::new(refused)).into()),
        };

        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        // The sender lives in the side, which the link's state holds, so no task feeds the
        // stream: a panic in one is raised through the stream's next poll.
        let panics = self.panics(self.app_runtime.clone().unwrap_or_else(|| Arc::new(runtime.clone())));
        let inbound = panics.guard(receiver_stream(inbound));
        let (writes, queued) = ordered::channel(WRITE_QUEUE);
        let writer = runtime.handle().spawn(panics.watch("writer", server_writer(producer, queued))).abort_handle();
        let (control, control_topics) = control;
        let mut watched = vec![(Arc::clone(&control), control_topics)];
        watched.extend(every.as_ref().map(|(every, topics)| (Arc::clone(every), topics.clone())));
        let side = Arc::new(ServerSide {
            consumer,
            every: every.as_ref().map(|(every, _)| Arc::clone(every)),
            writes,
            codec: self.codec,
            runtime: runtime.clone(),
            calls: Arc::new(Calls::new()),
            phase,
            deliveries: Mutex::new(Some(deliveries)),
            inflight: Arc::new(watch::channel(0).0),
            limit: self.max_inflight,
        });
        // The writer is aborted with the lanes when the link drops; `close` lets it finish.
        let mut tasks = vec![
            runtime.handle().spawn(panics.watch("request lane", request_lane(Arc::clone(&side), Arc::clone(&side.consumer)))).abort_handle(),
            runtime.handle().spawn(panics.watch("control lane", control_lane(Arc::clone(&side), control))).abort_handle(),
            writer,
            runtime.handle().spawn(panics.watch("partition watcher", watch_partitions(side.phase.subscribe(), watched, timer))).abort_handle(),
        ];
        if let Some((every, _)) = every {
            tasks.push(runtime.handle().spawn(panics.watch("request lane", request_lane(Arc::clone(&side), every))).abort_handle());
        }
        lock(&self.state).server = Some((side, tasks));
        Ok(inbound)
    }

    async fn connect(&self, client_runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let runtime = self.runtime()?;
        let base = Brokers::parse(&self.brokers)?.config();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let reply_topic = self.reply_topic.clone().unwrap_or_else(|| format!("ulo.rpc.reply.{id}"));
        let (partitions, replication, client_producer) = (self.partitions, self.replication, self.client_producer);
        let (replies, producer) = {
            let reply_topic = reply_topic.clone();
            runtime
                .run(async move {
                    create_topics(&base, std::slice::from_ref(&reply_topic), partitions, replication).await?;

                    // One group per reply topic reads every partition of it; `earliest` keeps a
                    // reply that lands before the group's first assignment.
                    let mut config = base.clone();
                    config.set("group.id", &reply_topic).set("enable.auto.commit", "true").set("auto.offset.reset", "earliest");
                    // Detached from its creation, so a step after it that fails drops it on a
                    // thread of its own rather than on the worker polling this.
                    let replies: Detached<StreamConsumer> = Detached::new(config.create()?);
                    replies.subscribe(&[reply_topic.as_str()])?;
                    Ok::<_, BoxError>((replies, client_producer(&base)?))
                })
                .await??
        };

        let (closed, closing) = oneshot::channel();
        let side = Arc::new(ClientSide {
            producer: Mutex::new(Some(producer)),
            codec: self.codec,
            id,
            reply_topic,
            calls: Mutex::new(HashMap::new()),
            closed: Mutex::new(Some(closed)),
            routing: Mutex::new(None),
            writing: Mutex::new(None),
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
        let panics = self.panics(Arc::clone(client_runtime));
        let writing = runtime.handle().spawn(panics.watch("writer", client_writer(Arc::clone(&side), queued)));
        *lock(&side.writing) = Some(writing.abort_handle());
        let (frames, replies_out) = mpsc::unbounded_channel();
        let replies_out = panics.guard(receiver_stream(replies_out));
        let explaining = panics.clone();
        let routing = runtime.handle().spawn(panics.watch_feeder("reply router", route_replies(Arc::clone(&side), replies, frames, closing, writes.clone())));
        *lock(&side.routing) = Some(routing);
        lock(&self.state).client = Some(Arc::clone(&side));

        Ok(Outbound {
            // Queued at the call, so frames go out in the order their sends were called.
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let (answer, answered) = oneshot::channel();
                let room = writes.send(Job::Send { pattern, frame, answer });
                let explaining = explaining.clone();
                Box::pin(async move {
                    let sent = async {
                        room.await.map_err(|_| BoxError::from(CLOSED))?;
                        answered.await.map_err(|_| BoxError::from(CLOSED))?
                    };
                    // A writer that panicked dropped the answer: the call is told the panic.
                    match sent.await {
                        Ok(()) => Ok(()),
                        Err(error) => Err(explaining.explain(error).await),
                    }
                })
            }),
            replies: replies_out,
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.as_ref().map(|(side, _)| Arc::clone(side));
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        pause(&side.consumer);
        if let Some(every) = &side.every {
            pause(every);
        }
        if let Err(error) = side.consumer.commit_consumer_state(CommitMode::Async) {
            tracing::debug!(%error, "the Kafka link had no offset to commit at the drain");
        }
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
        let (server, client) = {
            let mut state = lock(&self.state);
            (state.server.take(), state.client.take())
        };
        if let Some((side, _)) = server {
            side.phase.send_replace(Phase::Closed);
            side.calls.clear();
            lock(&side.deliveries).take();
            // A synchronous commit blocks its thread until the broker answers, or until
            // librdkafka's request timeout once the broker is gone, so it runs on one of its own.
            let consumer = Arc::clone(&side.consumer);
            match blocking(move || consumer.commit_consumer_state(CommitMode::Sync)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::debug!(%error, "the Kafka link had no offset to commit at close"),
                Err(error) => tracing::warn!(%error, "the Kafka link's commit at close failed"),
            }
        }
        if let Some(side) = client {
            // The probe's hooks hold a sender of the client writer's queue, which would keep the
            // writer and the side past the close in a probe build; production builds have no probe.
            #[cfg(feature = "conformance-probe")]
            self.probe.attach(|_| {}, || 0);
            side.close().await;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// The broker list as librdkafka takes it, and whether it asked for SSL.
struct Brokers {
    list: String,
    ssl: bool,
}

impl Brokers {
    fn parse(text: &str) -> Result<Brokers, BoxError> {
        let mut entries = Vec::new();
        let mut ssl = None;
        for entry in text.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
            let lower = entry.to_ascii_lowercase();
            let (secure, address) = if lower.starts_with("ssl://") {
                (true, &entry["ssl://".len()..])
            } else if lower.starts_with("plaintext://") {
                (false, &entry["plaintext://".len()..])
            } else {
                (false, entry)
            };
            if *ssl.get_or_insert(secure) != secure {
                return Err("the Kafka link's brokers mix `ssl://` and plaintext entries".into());
            }
            let port = address.rsplit_once(':').map(|(_, port)| port);
            if port.is_none_or(|port| port.parse::<u16>().is_err()) {
                return Err(format!("the Kafka link's broker `{entry}` is not `host:port`").into());
            }
            entries.push(address.to_owned());
        }
        if entries.is_empty() {
            return Err("the Kafka link names no broker".into());
        }
        let ssl = ssl.unwrap_or(false);
        if ssl && !cfg!(feature = "tls") {
            return Err("the Kafka link's brokers ask for `ssl://`, which needs the crate's `tls` feature".into());
        }
        Ok(Brokers { list: entries.join(","), ssl })
    }

    fn config(&self) -> ClientConfig {
        let mut config = ClientConfig::new();
        config.set("bootstrap.servers", &self.list);
        if self.ssl {
            config.set("security.protocol", "ssl");
        }
        config
    }
}

/// Creates `topics`, an existing one being no failure. The admin client is [`Detached`]: its drop
/// joins its polling thread, which polls with a 100 ms timeout, and on a runtime worker that drop
/// stalls every task scheduled there for as long.
async fn create_topics(base: &ClientConfig, topics: &[String], partitions: i32, replication: i32) -> Result<(), BoxError> {
    let admin: Detached<AdminClient<DefaultClientContext>> = Detached::new(base.create()?);
    let new: Vec<NewTopic<'_>> =
        topics.iter().map(|topic| NewTopic::new(topic, partitions, TopicReplication::Fixed(replication))).collect();
    for result in admin.create_topics(&new, &AdminOptions::new()).await? {
        if let Err((topic, code)) = result
            && code != RDKafkaErrorCode::TopicAlreadyExists
        {
            return Err(format!("the Kafka link could not create the topic `{topic}`: {code}").into());
        }
    }
    Ok(())
}

/// Commits the end offset of each of `topics`' partitions on which `group` has none committed, so
/// a partition the group has never consumed starts where the group first bound. Left at
/// `auto.offset.reset`, such a partition would start at its end each time it is assigned, and a
/// record produced while no instance consumed it, or while a rebalance moved it, would be skipped.
/// Kafka accepts a commit from outside a group only while the group is empty, so an instance
/// joining a running group leaves the offsets to the instances already in it.
///
/// Reading the committed offsets asks the group's coordinator, which a broker that has just
/// started, or is moving the coordinator, may not have ready: those three errors are retried
/// within the call's own timeout (`coordinator_retried`), waiting on the app's `timer` between
/// attempts, until the timer refuses a wait, which fails `bind` as a permanent `RuntimeShutDown`; any other fails `bind` at once. Each broker call blocks its thread and runs on one
/// of its own ([`blocking`]).
async fn anchor(base: &ClientConfig, group: &str, topics: &[String], timer: &Arc<dyn Timer>) -> Result<(), BoxError> {
    let mut config = base.clone();
    config.set("group.id", group).set("enable.auto.commit", "false");
    let consumer: Arc<Detached<BaseConsumer>> = Arc::new(Detached::new(config.create()?));
    let timeout = Timeout::After(ANCHOR_TIMEOUT);
    let reader = Arc::clone(&consumer);
    let topics = topics.to_vec();
    let every = blocking(move || -> Result<TopicPartitionList, BoxError> {
        let mut every = TopicPartitionList::new();
        for topic in &topics {
            let metadata = reader.fetch_metadata(Some(topic), timeout)?;
            for partition in metadata.topics().iter().filter(|found| found.name() == topic).flat_map(|found| found.partitions()) {
                every.add_partition(topic, partition.id());
            }
        }
        Ok(every)
    })
    .await??;
    let deadline = timer.now() + ANCHOR_TIMEOUT;
    let committed = coordinator_retried(
        deadline,
        |remaining| {
            let reader = Arc::clone(&consumer);
            let every = every.clone();
            async move { blocking(move || reader.committed_offsets(every, Timeout::After(remaining)).map_err(BoxError::from)).await? }
        },
        |backoff| timer.try_sleep(backoff),
        || timer.now(),
    )
    .await?;
    let group = group.to_owned();
    blocking(move || -> Result<(), BoxError> {
        let mut start = TopicPartitionList::new();
        for unset in committed.elements().into_iter().filter(|element| element.offset() == Offset::Invalid) {
            let (_, end) = consumer.fetch_watermarks(unset.topic(), unset.partition(), timeout)?;
            start.add_partition_offset(unset.topic(), unset.partition(), Offset::Offset(end))?;
        }
        if start.count() > 0
            && let Err(error) = consumer.commit(&start, CommitMode::Sync)
        {
            tracing::debug!(%error, group, "the Kafka link left the starting offsets to the group's running instances");
        }
        Ok(())
    })
    .await?
}

/// How long each of `anchor`'s broker calls may take, the committed-offsets read with its
/// retries included.
const ANCHOR_TIMEOUT: Duration = Duration::from_secs(10);

/// The first wait before a retry of a coordinator not yet ready, doubled after each up to
/// [`RETRY_BACKOFF_MAX`].
const RETRY_BACKOFF: Duration = Duration::from_millis(100);
const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// `attempt`, given the time left before `deadline`, retried while it fails with one of the
/// three errors a group coordinator answers while it loads or moves, which librdkafka classes
/// retriable, and a retry still fits before `deadline`; the last error is returned once one does
/// not. A `sleep` that refuses the wait before a retry, its runtime shut down, ends the loop with a
/// `Permanent` [`RuntimeShutDown`] whose source is the last attempt's error. Any other error is
/// returned at once. `sleep` and `now` are the clock, the app's `Timer` in `anchor`.
async fn coordinator_retried<T, A, S>(
    deadline: Instant,
    mut attempt: impl FnMut(Duration) -> A,
    mut sleep: impl FnMut(Duration) -> S,
    now: impl Fn() -> Instant,
) -> Result<T, BoxError>
where
    A: Future<Output = Result<T, BoxError>>,
    S: Future<Output = Result<(), Refused>>,
{
    let mut backoff = RETRY_BACKOFF;
    loop {
        let outcome = attempt(deadline.saturating_duration_since(now())).await;
        match &outcome {
            Err(error) if coordinator_not_ready(error) && now() + backoff < deadline => {
                tracing::debug!(%error, ?backoff, "the Kafka group coordinator is not ready; retrying");
                if let Err(refused) = sleep(backoff).await {
                    tracing::debug!(%refused, "the runtime timing the Kafka coordinator's backoff refused it; the read is not retried");
                    let last = outcome.err().expect("the match arm holds an error");
                    return Err(Permanent::new(RuntimeShutDown::after(last)).into());
                }
                backoff = (backoff * 2).min(RETRY_BACKOFF_MAX);
            }
            _ => return outcome,
        }
    }
}

fn coordinator_not_ready(error: &BoxError) -> bool {
    matches!(
        error.downcast_ref::<KafkaError>(),
        Some(KafkaError::MetadataFetch(
            RDKafkaErrorCode::NotCoordinator | RDKafkaErrorCode::CoordinatorLoadInProgress | RDKafkaErrorCode::CoordinatorNotAvailable
        ))
    )
}

/// Runs `call` on a thread of its own and answers its result: a librdkafka call that blocks its
/// thread, kept off the runtime's workers as a consumer's drop is ([`Detached`]). Not a
/// `spawn_blocking` task, which needs tokio's runtime and which a runtime waits for as it shuts
/// down. A thread that cannot be spawned runs `call` here; a call that panics is an error.
async fn blocking<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> Result<T, BoxError> {
    let (done, answered) = oneshot::channel();
    let call = Arc::new(Mutex::new(Some(call)));
    let on_thread = Arc::clone(&call);
    let spawned = std::thread::Builder::new().name("ulo-kafka-blocking".to_owned()).spawn(move || {
        if let Some(call) = lock(&on_thread).take() {
            let _ = done.send(call());
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the Kafka link could not spawn a thread for a blocking call, and made it here");
        let call = lock(&call).take().ok_or("the Kafka link's blocking call was taken twice")?;
        return Ok(call());
    }
    answered.await.map_err(|_| BoxError::from("the Kafka link's blocking call panicked"))
}

fn producer(base: &ClientConfig) -> Result<FutureProducer, BoxError> {
    let mut config = base.clone();
    config.set("message.max.bytes", MAX_MESSAGE.to_string()).set("message.timeout.ms", DELIVERY_TIMEOUT);
    Ok(config.create()?)
}

/// A consumer of `topics` to be assigned every partition of each at its end, outside any group's
/// rebalancing, so it reads from `bind` on rather than from a first assignment seconds later: the
/// control topic's, and the one reading the events every instance receives. Its `group.id` is
/// this instance's own and it commits nothing.
///
/// Each partition's end, its high watermark, is read here and assigned as an offset by
/// [`Unassigned::assign`]. librdkafka resolves a logical `Offset::End` only after `assign` has
/// returned, when its fetcher first asks the partition's leader, so a record produced between
/// `bind` returning and that answer would be skipped; an offset read first puts every record
/// produced after the read at or past it.
async fn own_consumer(base: &ClientConfig, group: String, topics: Vec<String>) -> Result<Unassigned, BoxError> {
    let mut config = base.clone();
    config.set("group.id", group).set("enable.auto.commit", "false").set("enable.auto.offset.store", "false").set("auto.offset.reset", "latest");
    let consumer: Arc<Detached<StreamConsumer>> = Arc::new(Detached::new(config.create()?));
    let reader = Arc::clone(&consumer);
    // Each topic with its partitions' ids and ends.
    type Ends = Vec<(String, Vec<(i32, i64)>)>;
    let ends = blocking(move || -> Result<Ends, BoxError> {
        let mut ends = Vec::new();
        for topic in topics {
            let partitions = partition_ids(&reader, &topic)?;
            let mut read = Vec::new();
            for partition in partitions {
                let (_, high) = reader.fetch_watermarks(&topic, partition, Timeout::After(METADATA_TIMEOUT))?;
                read.push((partition, high));
            }
            ends.push((topic, read));
        }
        Ok(ends)
    })
    .await??;
    let mut assignment = TopicPartitionList::new();
    for (topic, partitions) in &ends {
        for &(partition, end) in partitions {
            assignment.add_partition_offset(topic, partition, Offset::Offset(end))?;
        }
    }
    let topics = ends.into_iter().map(|(topic, partitions)| (topic, partitions.len())).collect();
    Ok(Unassigned { consumer, assignment, topics })
}

/// A consumer of the server's own whose partitions' ends have been read, not yet assigned.
struct Unassigned {
    consumer: Arc<Detached<StreamConsumer>>,
    /// Every partition of each topic at the end read.
    assignment: TopicPartitionList,
    /// Each topic and how many partitions it has in `assignment`.
    topics: Vec<(String, usize)>,
}

impl Unassigned {
    fn assign(self) -> Result<Assigned, BoxError> {
        self.consumer.assign(&self.assignment)?;
        Ok((self.consumer, self.topics))
    }
}

/// A consumer of the server's own, with each topic it was assigned every partition of and how
/// many partitions that was.
type Assigned = (Arc<Detached<StreamConsumer>>, Vec<(String, usize)>);

/// What [`Kafka::before_assign`] is unless a test replaces it: nothing to wait for.
fn no_hold(_topics: &[&str]) -> BoxFuture<'static, Result<(), BoxError>> {
    Box::pin(std::future::ready(Ok(())))
}

/// The ids of `topic`'s partitions as the broker's metadata lists them, failing when it lists
/// none, with the error the metadata reports for the topic if any. A topic whose metadata reports
/// an error beside its partitions, a leader not yet elected among them, answers the partitions.
/// Blocks its thread.
fn partition_ids(consumer: &StreamConsumer, topic: &str) -> Result<Vec<i32>, BoxError> {
    let metadata = consumer.fetch_metadata(Some(topic), Timeout::After(METADATA_TIMEOUT))?;
    let found = metadata.topics().iter().find(|found| found.name() == topic);
    let ids: Vec<i32> = found.map(|found| found.partitions().iter().map(|partition| partition.id()).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Err(match found.and_then(|found| found.error()) {
            Some(error) => format!("the Kafka link found no partition of `{topic}`: {}", RDKafkaErrorCode::from(error)).into(),
            None => format!("the Kafka link found no partition of `{topic}`").into(),
        });
    }
    Ok(ids)
}

/// Reads, every [`METADATA_REFRESH`] on the app's `timer`, the partition count of each topic in
/// `watched`, each a consumer of the server's own beside the topics it was assigned every
/// partition of and the count it was assigned, and logs a `warn` naming the topic and both counts
/// when the count read differs from the last one known. Such a consumer reads the partitions it
/// was assigned at `bind` and no other, so a record produced to a partition added since reaches
/// none of the instances already listening. A failed read is logged at `debug` and read again at
/// the next refresh. Ends once the server stops serving, or once the timer refuses a wait.
async fn watch_partitions(mut phase: watch::Receiver<Phase>, mut watched: Vec<Assigned>, timer: Arc<dyn Timer>) {
    loop {
        let refreshed = async {
            if timer.try_sleep(METADATA_REFRESH).await.is_err() {
                return false;
            }
            for (consumer, topics) in &mut watched {
                let reader = Arc::clone(consumer);
                let names: Vec<String> = topics.iter().map(|(topic, _)| topic.clone()).collect();
                let read = blocking(move || names.iter().map(|topic| partition_ids(&reader, topic).map(|ids| ids.len())).collect::<Result<Vec<_>, _>>()).await;
                match read.and_then(|counts| counts) {
                    Ok(counts) => {
                        for ((topic, known), now) in topics.iter_mut().zip(counts) {
                            if *known != now {
                                tracing::warn!(
                                    topic,
                                    before = *known,
                                    after = now,
                                    "a topic the Kafka link reads every partition of has {now} partitions where it had {}; this instance reads the \
                                     partitions it was assigned at `bind` and no other, so a record produced to another reaches no instance already \
                                     listening",
                                    *known
                                );
                                *known = now;
                            }
                        }
                    }
                    Err(error) => tracing::debug!(%error, "the Kafka link could not read the partition counts of the topics it reads every partition of"),
                }
            }
            true
        };
        tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() != Phase::Serving {
                    break;
                }
            }
            kept = refreshed => {
                if !kept {
                    break;
                }
            }
        }
    }
}

/// The server side of a bound link: the group's consumer, the reply producer, the calls it holds,
/// and where deliveries go.
struct ServerSide {
    consumer: Arc<Detached<StreamConsumer>>,
    /// The consumer of the topics whose events reach every instance, when the server has any.
    every: Option<Arc<Detached<StreamConsumer>>>,
    /// The server's writer, which every reply and `opened` is produced through.
    writes: ordered::Sender<Record>,
    codec: Codec,
    /// The link's runtime, which the drain's watcher is spawned on.
    runtime: Tokio,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
    /// The records of the group's topics handed over and not yet settled: each is counted until
    /// its `Ack` settles or is dropped, which the server does only after freeing the call's place.
    inflight: Arc<watch::Sender<usize>>,
    /// `Link::max_inflight`'s bound, at which the request lane pauses the assignment.
    limit: Option<usize>,
}

/// A held call's reply path holds the table it is held in and the writer's queue, so a side
/// dropped with calls held, its link dropped without `close`, would keep them, the writer and its
/// producer (F429).
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

    /// One record `consumer` read, whose offset its `Ack` stores there.
    async fn on_request(&self, consumer: &Arc<Detached<StreamConsumer>>, record: OwnedMessage) {
        let pattern = record.topic().to_owned();
        let headers = record.headers();
        let kind = header(headers, KIND);
        let reply = header(headers, REPLY_TO);
        let correlation = header(headers, CORRELATION);
        let call_headers = call_headers(headers);
        let data = Data::new(record.payload().map(Bytes::copy_from_slice).unwrap_or_default());
        let consumer = Arc::clone(consumer);
        let (partition, offset) = (record.partition(), record.offset());
        let topic = pattern.clone();
        // Stored once the handler completes and committed by the next auto-commit, for a
        // rejected event too, so it cannot loop on redelivery. Unsettled, a restarted group
        // reads the record again. Counted in flight until settled or dropped.
        let counted = InFlight::new(Arc::clone(&self.inflight));
        let ack = Ack::new(move |_accepted| {
            if let Err(error) = consumer.store_offset(&topic, partition, offset) {
                tracing::warn!(%error, topic, "the Kafka link could not store an offset");
            }
            drop(counted);
        });
        match (reply, kind.as_deref()) {
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
                // control topic after the call is held here.
                if let Err(error) = write(&self.writes, reply.clone(), correlation.clone(), Some(OPENED), Bytes::new()).await {
                    tracing::warn!(%error, pattern, "the Kafka link could not acknowledge a streamed request");
                }
                self.deliver(Delivery { frame: Frame::Open { id, pattern, headers: call_headers }, reply: Some(path), ack });
            }
            (_, Some(kind)) => {
                tracing::warn!(pattern, kind, "the Kafka link dropped a record of an unknown kind");
                ack.reject();
            }
        }
    }

    fn on_control(&self, record: OwnedMessage) {
        let headers = record.headers();
        let Some(key) = header(headers, CORRELATION) else { return };
        let frame = match header(headers, KIND).as_deref() {
            Some(IN) => self.calls.get(&key).map(|(id, path)| {
                let data = Data::new(record.payload().map(Bytes::copy_from_slice).unwrap_or_default());
                (Frame::In { id, data }, path)
            }),
            Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
            Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
            _ => None,
        };
        if let Some((frame, path)) = frame {
            self.deliver(Delivery { frame, reply: Some(path), ack: Ack::none() });
        }
    }

    /// Holds a call under `key`, its correlation id, the key its control records carry.
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

/// `consumer`'s records until the drain, which pauses the partitions and leaves the consumer to
/// the acknowledgments still owed: the group's consumer, or the one reading the events every
/// instance receives. At the server's in-flight bound the lane pauses the assignment until a
/// record settles ([`at_bound`]), so the requests over it wait in their topics.
async fn request_lane(side: Arc<ServerSide>, consumer: Arc<Detached<StreamConsumer>>) {
    let mut phase = side.phase.subscribe();
    loop {
        // The borrowed record is detached inside the arm: it borrows the consumer and is not
        // `Send`, so it must not live across an await.
        let record = tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() != Phase::Serving {
                    break;
                }
                continue;
            }
            record = consumer.recv() => record.map(|record| record.detach()),
        };
        match record {
            Ok(record) => {
                side.on_request(&consumer, record).await;
                if !at_bound(&side, &consumer, &mut phase).await {
                    break;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Holds the request lane while the records in flight reach the server's bound: the assignment
/// paused, then resumed once one settles. `pause` is local to librdkafka and drops the records it
/// fetched ahead, so the paused partitions resume from the next record the lane has not read. The
/// consumer is still read meanwhile, which serves a rebalance; a record from a partition assigned
/// since the pause is kept here, the new assignment paused in turn, and handed over once there is
/// room. `false` once the server drains or closes, the partitions left paused for the drain.
async fn at_bound(side: &ServerSide, consumer: &Arc<Detached<StreamConsumer>>, phase: &mut watch::Receiver<Phase>) -> bool {
    let Some(limit) = side.limit else { return true };
    if *side.inflight.borrow() < limit {
        return true;
    }
    pause(consumer);
    let mut inflight = side.inflight.subscribe();
    let mut kept = VecDeque::new();
    loop {
        if *inflight.borrow_and_update() < limit {
            match kept.pop_front() {
                Some(record) => {
                    side.on_request(consumer, record).await;
                    continue;
                }
                None => break,
            }
        }
        let record = tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() != Phase::Serving {
                    return false;
                }
                continue;
            }
            _ = inflight.changed() => continue,
            record = consumer.recv() => record.map(|record| record.detach()),
        };
        match record {
            Ok(record) => {
                pause(consumer);
                kept.push_back(record);
            }
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's consumer failed a read while paused; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    if *phase.borrow() == Phase::Serving {
        match consumer.assignment() {
            Ok(assignment) => {
                if let Err(error) = consumer.resume(&assignment) {
                    tracing::warn!(%error, "the Kafka link could not resume its partitions");
                }
            }
            Err(error) => tracing::warn!(%error, "the Kafka link could not read its assignment"),
        }
    }
    true
}

/// Pauses every partition currently assigned to `consumer`.
fn pause(consumer: &StreamConsumer) {
    match consumer.assignment() {
        Ok(assignment) => {
            if let Err(error) = consumer.pause(&assignment) {
                tracing::warn!(%error, "the Kafka link could not pause its partitions");
            }
        }
        Err(error) => tracing::warn!(%error, "the Kafka link could not read its assignment"),
    }
}

/// One record counted in [`ServerSide::inflight`] until dropped.
struct InFlight(Arc<watch::Sender<usize>>);

impl InFlight {
    fn new(count: Arc<watch::Sender<usize>>) -> Self {
        count.send_modify(|count| *count += 1);
        InFlight(count)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count = count.saturating_sub(1));
    }
}

async fn control_lane(side: Arc<ServerSide>, control: Arc<Detached<StreamConsumer>>) {
    let mut phase = side.phase.subscribe();
    loop {
        let record = tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() == Phase::Closed {
                    break;
                }
                continue;
            }
            record = control.recv() => record.map(|record| record.detach()),
        };
        match record {
            Ok(record) => side.on_control(record),
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's control consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// The calls a server holds, by the key their control records carry, each with its local id and
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

/// One reply-lane record for the server's writer, its delivery's outcome answered on `answer`.
struct Record {
    topic: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
    answer: oneshot::Sender<Result<(), BoxError>>,
}

/// The delivery reports a writer waits on while it queues the next records.
type Delivering = FuturesUnordered<BoxFuture<'static, ()>>;

/// Queues the server's replies with librdkafka one after another, in the order they were queued,
/// waiting for each delivery report beside the records that follow, until every sender is gone.
/// The producer drops as the writer ends, at `close` or once a dropped link's side has gone, on a
/// thread of its own.
async fn server_writer(producer: Detached<FutureProducer>, mut queued: ordered::Receiver<Record>) {
    let mut delivering = Delivering::new();
    loop {
        let Record { topic, correlation, kind, body, answer } = tokio::select! {
            record = queued.recv() => match record {
                Some(record) => record,
                None => break,
            },
            Some(()) = delivering.next(), if !delivering.is_empty() => continue,
        };
        match produce(&producer, &topic, correlation.as_deref(), kind, &CallHeaders::new(), &body).await {
            Ok(delivery) => delivering.push(Box::pin(async move {
                let _ = answer.send(delivery.await);
            })),
            Err(error) => {
                let _ = answer.send(Err(error));
            }
        }
    }
    while delivering.next().await.is_some() {}
}

/// Queues one message on the server's writer at the call and answers a wait for it to be
/// published.
fn write(
    writes: &ordered::Sender<Record>,
    topic: String,
    correlation: Option<String>,
    kind: Option<&'static str>,
    body: Bytes,
) -> impl Future<Output = Result<(), BoxError>> + Send + 'static {
    let (answer, answered) = oneshot::channel();
    let room = writes.send(Record { topic, correlation, kind, body, answer });
    async move {
        let gone = || BoxError::from("the Kafka link's server side is closed");
        room.await.map_err(|_| gone())?;
        answered.await.map_err(|_| gone())?
    }
}

/// The client side: the producer, the reply topic every reply lands on, each call's correlation
/// id `<id>.<call>`.
struct ClientSide {
    /// `None` once the link has closed.
    producer: Mutex<Option<FutureProducer>>,
    codec: Codec,
    id: String,
    reply_topic: String,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// Stops the reply router at close.
    closed: Mutex<Option<oneshot::Sender<()>>>,
    /// The reply router, which `close` awaits: it drops the reply consumer as it ends.
    routing: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The writer, aborted when the link drops; `close` lets it end with its queue.
    writing: Mutex<Option<AbortHandle>>,
    /// The link's runtime, on which a held `cancel`'s hold runs out.
    runtime: Tokio,
    #[cfg(feature = "conformance-probe")]
    probe: Arc<ClientProbe>,
}

enum ClientCall {
    Unary,
    /// A streamed request: until the server acknowledges the `open`, its `in` and `in_end` frames
    /// wait in `held`, and the writer produces them in order once it does.
    /// `until` is the deadline its `open` carried.
    Streaming { opened: bool, held: VecDeque<Frame>, until: HoldUntil },
    /// A streamed request cancelled before the server acknowledged its `open`: the `cancel` waits
    /// for the acknowledgment and is produced alone, the frames held before it dropped. The entry
    /// stays until then, or until the hold runs out, the call's deadline or `CANCEL_HOLD` after
    /// the `cancel` for an `open` carrying none, when it is dropped.
    Cancelled,
}

const CLOSED: &str = "the Kafka link's client is closed";

/// What the client's writer takes, in order: a frame to send, or the server's `opened` for a
/// streamed request, which releases the frames held for it.
enum Job {
    Send { pattern: Pattern, frame: Frame, answer: oneshot::Sender<Result<(), BoxError>> },
    Opened(u64),
}

/// Queues the client's records with librdkafka in the order they were queued, waiting for each
/// delivery report beside the records that follow, until every sender is gone.
async fn client_writer(side: Arc<ClientSide>, mut queued: ordered::Receiver<Job>) {
    let mut delivering = Delivering::new();
    loop {
        let job = tokio::select! {
            job = queued.recv() => match job {
                Some(job) => job,
                None => break,
            },
            Some(()) = delivering.next(), if !delivering.is_empty() => continue,
        };
        match job {
            Job::Send { pattern, frame, answer } => side.send(pattern, frame, answer, &mut delivering).await,
            Job::Opened(call) => {
                for frame in side.opened(call) {
                    side.produce_held(call, frame, &mut delivering).await;
                }
            }
        }
    }
    while delivering.next().await.is_some() {}
}

impl ClientSide {
    async fn send(self: &Arc<Self>, pattern: Pattern, frame: Frame, answer: oneshot::Sender<Result<(), BoxError>>, delivering: &mut Delivering) {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                lock(&self.calls).insert(id, ClientCall::Unary);
                let queued = self.produce_request(&pattern, id, &headers, None, data.as_bytes()).await;
                self.settle(queued, Some(id), answer, delivering);
            }
            Frame::Evt { headers, data, .. } => {
                let record_headers = append(OwnedHeaders::new(), &headers);
                let record = FutureRecord::to(pattern.as_str()).key(self.id.as_str()).payload(data.as_bytes()).headers(record_headers);
                let queued = match self.live_producer() {
                    Ok(producer) => enqueue(&producer, record, pattern.as_str(), data.len()).await,
                    Err(error) => Err(error),
                };
                self.settle(queued, None, answer, delivering);
            }
            Frame::Open { id, headers, .. } => {
                lock(&self.calls).insert(id, ClientCall::Streaming { opened: false, held: VecDeque::new(), until: HoldUntil::at_open(&headers) });
                let queued = self.produce_request(&pattern, id, &headers, Some(OPEN), &[]).await;
                self.settle(queued, Some(id), answer, delivering);
            }
            Frame::In { id, data } => {
                if data.len() as u64 > MAX_MESSAGE {
                    let _ = answer.send(Err(Box::new(FrameTooLarge { size: data.len() as u64, limit: MAX_MESSAGE })));
                    return;
                }
                self.control(id, Frame::In { id, data }, delivering).await;
                let _ = answer.send(Ok(()));
            }
            Frame::InEnd { id } => {
                self.control(id, Frame::InEnd { id }, delivering).await;
                let _ = answer.send(Ok(()));
            }
            Frame::Cancel { id } => {
                let call = lock(&self.calls).remove(&id);
                match call {
                    Some(ClientCall::Unary) => {
                        let queued = self.produce_control(id, CANCEL, &[]).await;
                        self.settle(queued, None, answer, delivering);
                    }
                    Some(ClientCall::Streaming { opened: true, .. }) => {
                        self.produce_held(id, Frame::Cancel { id }, delivering).await;
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

    /// Answers a produce once its delivery report arrives; a request that failed is forgotten.
    fn settle(self: &Arc<Self>, queued: Result<Report, BoxError>, call: Option<u64>, answer: oneshot::Sender<Result<(), BoxError>>, delivering: &mut Delivering) {
        let side = Arc::clone(self);
        let delivery = match queued {
            Ok(delivery) => delivery,
            Err(error) => {
                if let Some(call) = call {
                    lock(&side.calls).remove(&call);
                }
                let _ = answer.send(Err(error));
                return;
            }
        };
        delivering.push(Box::pin(async move {
            let outcome = delivery.await;
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

    /// A streamed request's `in` or `in_end`: produced once the server has acknowledged the
    /// `open`, held until then, and dropped for a call that has ended.
    async fn control(&self, id: u64, frame: Frame, delivering: &mut Delivering) {
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
        self.produce_held(id, held, delivering).await;
    }

    /// The frames held for a streamed request the server has now acknowledged, which the writer
    /// produces before anything queued after the acknowledgment.
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

    /// Produces one control frame of a streamed request; a failure is logged, as the call's caller
    /// has gone on.
    async fn produce_held(&self, call: u64, frame: Frame, delivering: &mut Delivering) {
        let (kind, body) = match frame {
            Frame::In { data, .. } => (IN, data.into_bytes()),
            Frame::InEnd { .. } => (IN_END, Bytes::new()),
            Frame::Cancel { .. } => (CANCEL, Bytes::new()),
            _ => return,
        };
        match self.produce_control(call, kind, &body).await {
            Ok(delivery) => delivering.push(Box::pin(async move {
                if let Err(error) = delivery.await {
                    tracing::debug!(%error, kind, "the Kafka link could not publish a control frame");
                }
            })),
            Err(error) => tracing::debug!(%error, kind, "the Kafka link could not publish a control frame"),
        }
    }

    fn correlation(&self, call: u64) -> String {
        format!("{}.{call}", self.id)
    }

    fn call_of(&self, correlation: &str) -> Option<u64> {
        correlation.strip_prefix(self.id.as_str())?.strip_prefix('.')?.parse().ok()
    }

    /// Keyed by this client's id, so one caller's requests share a partition and stay ordered.
    async fn produce_request(&self, pattern: &Pattern, call: u64, headers: &CallHeaders, kind: Option<&str>, body: &[u8]) -> Result<Report, BoxError> {
        let correlation = self.correlation(call);
        let mut record_headers = OwnedHeaders::new()
            .insert(Header { key: REPLY_TO, value: Some(self.reply_topic.as_str()) })
            .insert(Header { key: CORRELATION, value: Some(correlation.as_str()) });
        if let Some(kind) = kind {
            record_headers = record_headers.insert(Header { key: KIND, value: Some(kind) });
        }
        record_headers = append(record_headers, headers);
        let record = FutureRecord::to(pattern.as_str()).key(self.id.as_str()).payload(body).headers(record_headers);
        enqueue(&self.live_producer()?, record, pattern.as_str(), body.len()).await
    }

    async fn produce_control(&self, call: u64, kind: &str, body: &[u8]) -> Result<Report, BoxError> {
        let correlation = self.correlation(call);
        let record_headers = OwnedHeaders::new()
            .insert(Header { key: CORRELATION, value: Some(correlation.as_str()) })
            .insert(Header { key: KIND, value: Some(kind) });
        let record = FutureRecord::to(CONTROL).key(correlation.as_str()).payload(body).headers(record_headers);
        enqueue(&self.live_producer()?, record, CONTROL, body.len()).await
    }

    fn live_producer(&self) -> Result<FutureProducer, BoxError> {
        lock(&self.producer).clone().ok_or_else(|| CLOSED.into())
    }

    /// Stops the reply router and waits for it to end, the reply consumer leaving its group while
    /// the broker is still there to answer, then drops the producer, closing its connections. The
    /// `RpcClient` keeps the connection's send half until its next call replaces it, so the
    /// producer is taken out of it here.
    async fn close(&self) {
        if let Some(closed) = lock(&self.closed).take() {
            let _ = closed.send(());
        }
        let routing = lock(&self.routing).take();
        if let Some(routing) = routing {
            let _ = routing.await;
        }
        let producer = lock(&self.producer).take();
        if let Some(producer) = producer {
            let _ = drop_detached(producer).await;
        }
    }
}

/// Reads the client's replies. `replies` is [`Detached`], so a router dropped before it ends, its
/// client never closed and the runtime shutting down, still drops the consumer on a thread of its
/// own.
async fn route_replies(
    side: Arc<ClientSide>,
    replies: Detached<StreamConsumer>,
    frames: mpsc::UnboundedSender<Frame>,
    mut closing: oneshot::Receiver<()>,
    writes: ordered::Sender<Job>,
) {
    loop {
        let record = tokio::select! {
            _ = &mut closing => break,
            record = replies.recv() => record.map(|record| record.detach()),
        };
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(%error, "the Kafka link's reply consumer failed a read; retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let headers = record.headers();
        let Some(call) = header(headers, CORRELATION).and_then(|correlation| side.call_of(&correlation)) else { continue };
        if header(headers, KIND).as_deref() == Some(OPENED) {
            #[cfg(feature = "conformance-probe")]
            if side.probe.withholds(call) {
                continue;
            }
            // To the writer, behind the frames already queued.
            if writes.send(Job::Opened(call)).await.is_err() {
                break;
            }
            continue;
        }
        let frame = match side.codec.decode_frame(record.payload().unwrap_or_default()) {
            Ok(frame) => with_id(frame, call),
            Err(error) => {
                tracing::warn!(%error, "the Kafka link dropped a reply that does not decode");
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
    let _ = drop_detached(replies.into_inner()).await;
}

/// A record's delivery report, once librdkafka has queued the record.
type Report = BoxFuture<'static, Result<(), BoxError>>;

/// One reply-lane record, keyed by the call's correlation id so one call's frames share a
/// partition and stay ordered.
async fn produce(
    producer: &FutureProducer,
    topic: &str,
    correlation: Option<&str>,
    kind: Option<&str>,
    headers: &CallHeaders,
    body: &[u8],
) -> Result<Report, BoxError> {
    let mut record_headers = OwnedHeaders::new();
    if let Some(correlation) = correlation {
        record_headers = record_headers.insert(Header { key: CORRELATION, value: Some(correlation) });
    }
    if let Some(kind) = kind {
        record_headers = record_headers.insert(Header { key: KIND, value: Some(kind) });
    }
    record_headers = append(record_headers, headers);
    let record = FutureRecord::to(topic).key(correlation.unwrap_or_default()).payload(body).headers(record_headers);
    enqueue(producer, record, topic, body.len()).await
}

/// Queues `record` with librdkafka, waiting up to five seconds for room in its queue, and answers
/// the record's delivery report. An unknown topic is the miss signal where the broker does not
/// auto-create topics; an oversized record is `FrameTooLarge`. The queueing is the writer's, in
/// order; the report is waited on beside the records queued after it.
async fn enqueue(producer: &FutureProducer, record: FutureRecord<'_, str, [u8]>, topic: &str, size: usize) -> Result<Report, BoxError> {
    if size as u64 > MAX_MESSAGE {
        return Err(Box::new(FrameTooLarge { size: size as u64, limit: MAX_MESSAGE }));
    }
    let started = Instant::now();
    let mut record = record;
    loop {
        match producer.send_result(record) {
            Ok(report) => {
                let topic = topic.to_owned();
                return Ok(Box::pin(async move {
                    match report.await {
                        Ok(Ok(_)) => Ok(()),
                        Ok(Err((error, _))) => Err(refused(error, &topic, size)),
                        Err(_) => Err("the Kafka producer was dropped before the record's delivery report".into()),
                    }
                }));
            }
            Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), back)) if started.elapsed() < QUEUE_TIMEOUT => {
                record = back;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err((error, _)) => return Err(refused(error, topic, size)),
        }
    }
}

/// A produce librdkafka refused, mapped as `RpcClient` reads a link's refusal.
fn refused(error: KafkaError, topic: &str, size: usize) -> BoxError {
    match error {
        KafkaError::MessageProduction(RDKafkaErrorCode::UnknownTopicOrPartition | RDKafkaErrorCode::UnknownTopic) => {
            Box::new(NoDestination { pattern: topic.to_owned() })
        }
        KafkaError::MessageProduction(RDKafkaErrorCode::MessageSizeTooLarge) => Box::new(FrameTooLarge { size: size as u64, limit: MAX_MESSAGE }),
        error => Box::new(error),
    }
}

fn append(mut record_headers: OwnedHeaders, headers: &CallHeaders) -> OwnedHeaders {
    for (name, value) in headers.iter() {
        record_headers = record_headers.insert(Header { key: name, value: Some(value) });
    }
    record_headers
}

fn header<H: Headers>(headers: Option<&H>, name: &str) -> Option<String> {
    let headers = headers?;
    (0..headers.count())
        .map(|index| headers.get(index))
        .find(|header| header.key == name)
        .and_then(|header| header.value)
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

/// The call's headers from the record's, the link's reserved names left out.
fn call_headers<H: Headers>(headers: Option<&H>) -> CallHeaders {
    let mut out = CallHeaders::new();
    if let Some(headers) = headers {
        for index in 0..headers.count() {
            let header = headers.get(index);
            if matches!(header.key, KIND | REPLY_TO | CORRELATION) {
                continue;
            }
            if let Some(value) = header.value {
                out.insert(header.key, String::from_utf8_lossy(value).into_owned());
            }
        }
    }
    out
}

/// The reply frame with the caller's own `id`, read from the correlation id: the server wrote its
/// local id.
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

/// A value whose drop runs on a thread of its own, for a librdkafka consumer, whose drop leaves the
/// group and polls until librdkafka confirms, about a tenth of a second with the broker up and
/// without bound once the broker is gone, for an admin client, whose drop joins its polling
/// thread, about a tenth of a second, and for the server's producer, whose drop joins its polling
/// thread and flushes for up to half a second. On a runtime worker each stalls every task
/// scheduled there.
struct Detached<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> Detached<T> {
    fn new(value: T) -> Self {
        Detached(Some(value))
    }

    /// The value, for a caller that drops it with [`drop_detached`] and waits for the drop.
    fn into_inner(mut self) -> T {
        self.0.take().expect("the value is taken only once")
    }
}

impl<T: Send + 'static> Deref for Detached<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_ref().expect("the value is taken only by the drop")
    }
}

impl<T: Send + 'static> Drop for Detached<T> {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            drop(drop_detached(value));
        }
    }
}

/// Drops `value` on a thread of its own and answers once the drop has finished: a consumer, or a
/// producer, whose drop flushes for up to half a second before closing its connections.
/// Not a `spawn_blocking` task: a runtime waits for those as it shuts down, and a consumer whose
/// broker is gone would hold it there. A thread that cannot be spawned drops `value` here.
fn drop_detached<T: Send + 'static>(value: T) -> oneshot::Receiver<()> {
    let (done, finished) = oneshot::channel();
    let spawned = std::thread::Builder::new().name("ulo-kafka-drop".to_owned()).spawn(move || {
        drop(value);
        let _ = done.send(());
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the Kafka link could not spawn a thread to drop a librdkafka handle on, and dropped it here");
    }
    finished
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Ready, ready};

    use futures_util::FutureExt;

    use super::*;

    /// The server's shed log names the link by it as answered.
    #[test]
    fn the_endpoint_is_each_broker_without_its_credentials() {
        let brokers = "user:pass@broker1:9092, ssl://u:pass@broker2:9093?x=pass,broker3:9094";
        let endpoint = Link::endpoint(&Kafka::brokers(brokers)).expect("an endpoint");
        assert_eq!(endpoint, "broker1:9092,ssl://broker2:9093,broker3:9094");
        assert!(!endpoint.contains("pass") && !endpoint.contains("user"), "a credential reached the endpoint: {endpoint}");
    }

    /// A clock that `sleep` advances, starting at `start`, or, `refusing`, one whose runtime has
    /// shut down: each sleep is refused and the clock stands still.
    struct Clock {
        now: Cell<Instant>,
        slept: RefCell<Vec<Duration>>,
        refusing: bool,
    }

    impl Clock {
        fn new(start: Instant) -> Self {
            Clock { now: Cell::new(start), slept: RefCell::new(Vec::new()), refusing: false }
        }

        fn sleep(&self, d: Duration) -> Ready<Result<(), Refused>> {
            self.slept.borrow_mut().push(d);
            if self.refusing {
                return ready(Err(Refused));
            }
            self.now.set(self.now.get() + d);
            ready(Ok(()))
        }
    }

    fn failing(code: RDKafkaErrorCode) -> BoxError {
        BoxError::from(KafkaError::MetadataFetch(code))
    }

    /// `coordinator_retried` run to its end: every attempt and sleep here is ready at once.
    fn retried<T>(
        deadline: Instant,
        mut attempt: impl FnMut(Duration) -> Result<T, BoxError>,
        clock: &Clock,
    ) -> Result<T, BoxError> {
        coordinator_retried(deadline, |left| ready(attempt(left)), |d| clock.sleep(d), || clock.now.get())
            .now_or_never()
            .expect("every attempt and sleep is ready at once")
    }

    fn code_of(outcome: &Result<(), BoxError>) -> Option<RDKafkaErrorCode> {
        match outcome.as_ref().err().and_then(|error| error.downcast_ref::<KafkaError>()) {
            Some(KafkaError::MetadataFetch(code)) => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn each_coordinator_error_is_retried_until_the_read_succeeds() {
        for code in [
            RDKafkaErrorCode::NotCoordinator,
            RDKafkaErrorCode::CoordinatorLoadInProgress,
            RDKafkaErrorCode::CoordinatorNotAvailable,
        ] {
            let start = Instant::now();
            let clock = Clock::new(start);
            let mut calls = 0;
            let outcome = retried(
                start + ANCHOR_TIMEOUT,
                |_| {
                    calls += 1;
                    if calls < 3 { Err(failing(code)) } else { Ok(calls) }
                },
                &clock,
            );
            assert_eq!(outcome.ok(), Some(3), "{code:?} was not retried until the read succeeded");
            assert_eq!(*clock.slept.borrow(), vec![RETRY_BACKOFF, RETRY_BACKOFF * 2], "{code:?}: the waits between attempts");
        }
    }

    #[test]
    fn any_other_error_fails_at_once() {
        let start = Instant::now();
        let clock = Clock::new(start);
        let mut calls = 0;
        let outcome: Result<(), BoxError> = retried(
            start + ANCHOR_TIMEOUT,
            |_| {
                calls += 1;
                Err(failing(RDKafkaErrorCode::BrokerTransportFailure))
            },
            &clock,
        );
        assert_eq!(code_of(&outcome), Some(RDKafkaErrorCode::BrokerTransportFailure), "{outcome:?}");
        assert_eq!(calls, 1, "an error other than the coordinator's three was retried");
        assert!(clock.slept.borrow().is_empty());
    }

    #[test]
    fn retries_stop_before_the_deadline_and_return_the_last_error() {
        let start = Instant::now();
        let clock = Clock::new(start);
        let mut remaining = Vec::new();
        let outcome: Result<(), BoxError> = retried(
            start + ANCHOR_TIMEOUT,
            |left| {
                remaining.push(left);
                Err(failing(RDKafkaErrorCode::CoordinatorLoadInProgress))
            },
            &clock,
        );
        assert_eq!(code_of(&outcome), Some(RDKafkaErrorCode::CoordinatorLoadInProgress), "{outcome:?}");
        let slept: Duration = clock.slept.borrow().iter().sum();
        assert!(slept < ANCHOR_TIMEOUT, "the retries slept {slept:?}, past the {ANCHOR_TIMEOUT:?} timeout");
        assert_eq!(remaining.first(), Some(&ANCHOR_TIMEOUT), "the first attempt was not given the whole timeout");
        assert!(remaining.windows(2).all(|pair| pair[1] < pair[0]), "each attempt was not given what was left: {remaining:?}");
    }

    /// The attempts after which a loop still retrying is taken to spin: its clock stands still, so
    /// nothing else would end it.
    const TURNS: usize = 3;

    /// N3: a clock that refuses the backoff, its runtime shut down, ends the retries after the one
    /// refused wait, as a `Permanent` `RuntimeShutDown` whose source is the last attempt's error.
    #[test]
    fn a_refused_backoff_ends_the_retries() {
        let start = Instant::now();
        let clock = Clock { refusing: true, ..Clock::new(start) };
        let mut calls = 0;
        let outcome: Result<(), BoxError> = retried(
            start + ANCHOR_TIMEOUT,
            |_| {
                calls += 1;
                assert!(calls <= TURNS, "coordinator_retried retried {calls} times on a clock that refused every backoff");
                Err(failing(RDKafkaErrorCode::CoordinatorNotAvailable))
            },
            &clock,
        );
        let error = outcome.expect_err("the retries ended without an error");
        let permanent = error.downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the refusal is not `Permanent`: {error:?}"));
        let shut_down = std::error::Error::source(permanent)
            .and_then(|inner| inner.downcast_ref::<RuntimeShutDown>())
            .unwrap_or_else(|| panic!("the `Permanent` does not hold a `RuntimeShutDown`: {error:?}"));
        let last = std::error::Error::source(shut_down).expect("the refusal carries the last attempt's error");
        assert!(
            matches!(last.downcast_ref::<KafkaError>(), Some(KafkaError::MetadataFetch(RDKafkaErrorCode::CoordinatorNotAvailable))),
            "the refusal's source is not the last attempt's error: {last:?}"
        );
        assert_eq!((calls, clock.slept.borrow().len()), (1, 1), "the attempts and backoffs on a clock refusing the backoff");
    }

    /// A client's `connect` failing after its reply consumer exists, against a broker in a
    /// container of its own.
    #[cfg(feature = "integration")]
    mod failed_connect {
        use std::net::{Ipv4Addr, SocketAddr};
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        use std::thread::{self, ThreadId};

        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{ContainerAsync, GenericImage, ImageExt};
        use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
        use ulo_rpc_conformance::{Budget, Harness, OnTokio, report, startup_failed};

        use super::*;

        const PORT: u16 = 9092;

        /// How many connects the test fails, each with a reply consumer of its own to drop.
        const CONNECTS: usize = 3;

        /// The planted producer failures, one per connect.
        static FAILED: AtomicUsize = AtomicUsize::new(0);

        /// The step after the reply consumer, failing.
        fn failing_producer(_: &ClientConfig) -> Result<FutureProducer, BoxError> {
            FAILED.fetch_add(1, Ordering::SeqCst);
            Err("a producer the test refuses".into())
        }

        /// A single-node KRaft broker whose one client listener advertises a relay bound before the
        /// container starts, since a Kafka client connects wherever the metadata says.
        pub(super) async fn kraft() -> (Relay, ContainerAsync<GenericImage>) {
            let relay = Relay::bind().await;
            let advertised = format!(
                "CLIENT://{},BROKER://localhost:9094",
                relay.local_addr().unwrap_or_else(|error| startup_failed!("the relay has no address: {}", report(&error)))
            );
            let (container, addrs) = unshadowed("the Kafka container", || {
                let advertised = advertised.clone();
                async move {
                    let container = GenericImage::new("apache/kafka-native", "3.8.0")
                        .with_exposed_port(PORT.tcp())
                        .with_wait_for(WaitFor::message_on_stdout("Kafka Server started"))
                        .with_env_var("CLUSTER_ID", "ulo-rpc-kafka-failed-connect")
                        .with_env_var("KAFKA_NODE_ID", "1")
                        .with_env_var("KAFKA_PROCESS_ROLES", "broker,controller")
                        .with_env_var("KAFKA_CONTROLLER_QUORUM_VOTERS", "1@localhost:9093")
                        .with_env_var("KAFKA_CONTROLLER_LISTENER_NAMES", "CONTROLLER")
                        .with_env_var("KAFKA_LISTENERS", "CLIENT://:9092,CONTROLLER://:9093,BROKER://:9094")
                        .with_env_var("KAFKA_ADVERTISED_LISTENERS", advertised)
                        .with_env_var("KAFKA_LISTENER_SECURITY_PROTOCOL_MAP", "CLIENT:PLAINTEXT,CONTROLLER:PLAINTEXT,BROKER:PLAINTEXT")
                        .with_env_var("KAFKA_INTER_BROKER_LISTENER_NAME", "BROKER")
                        .with_env_var("KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR", "1")
                        .with_env_var("KAFKA_OFFSETS_TOPIC_NUM_PARTITIONS", "1")
                        .with_env_var("KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR", "1")
                        .with_env_var("KAFKA_TRANSACTION_STATE_LOG_MIN_ISR", "1")
                        .with_env_var("KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS", "0")
                        .start()
                        .await
                        .unwrap_or_else(|error| startup_failed!("the Kafka container did not start: {}", report(&error)));
                    let port = container
                        .get_host_port_ipv4(PORT)
                        .await
                        .unwrap_or_else(|error| startup_failed!("the Kafka listener is not mapped: {}", report(&error)));
                    (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
                }
            })
            .await;
            reachable(addrs[0], Duration::from_secs(10)).await;
            (Relay::listen(relay, addrs[0]), container)
        }

        /// F419: a client's `connect` that fails after its reply consumer exists drops the consumer
        /// on a thread of its own. Polled on one thread beside a task ticking every tenth of the
        /// suite's [`Budget::stall`], three connects whose producer the test refuses each create
        /// the reply topic, create and subscribe the consumer and fail; the longest gap between two
        /// ticks stays under the bound. A consumer dropped on the thread polling the connect leaves
        /// its group there, which librdkafka confirms only after polling.
        #[test]
        fn a_connect_failing_after_its_reply_consumer_does_not_stall_the_thread() {
            OnTokio::block_on_one_thread(async {
                let (relay, _container) = kraft().await;
                let bound = Budget::default().stall;
                let mut link = Kafka::brokers(relay.addr().to_string());
                link.client_producer = failing_producer;
                let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());

                let stop = Arc::new(AtomicBool::new(false));
                let ticker = tokio::spawn({
                    let stop = Arc::clone(&stop);
                    async move {
                        let (mut longest, mut count, mut threads) = (Duration::ZERO, 0usize, Vec::<ThreadId>::new());
                        let mut last = Instant::now();
                        while !stop.load(Ordering::Acquire) {
                            tokio::time::sleep(bound / 10).await;
                            let now = Instant::now();
                            longest = longest.max(now - last);
                            last = now;
                            count += 1;
                            if !threads.contains(&thread::current().id()) {
                                threads.push(thread::current().id());
                            }
                        }
                        (longest, count, threads)
                    }
                });
                for _ in 0..CONNECTS {
                    match link.connect(&runtime).await {
                        Ok(_) => panic!("a connect whose producer was refused succeeded"),
                        Err(error) => assert!(error.to_string().contains("a producer the test refuses"), "the connect failed otherwise: {error}"),
                    }
                }
                stop.store(true, Ordering::Release);
                let (longest, count, threads) = ticker.await.expect("the ticker does not panic");

                assert_eq!(FAILED.load(Ordering::SeqCst), CONNECTS, "a connect failed before it reached its producer");
                assert_eq!(threads, vec![thread::current().id()], "the ticker was polled on a thread other than the test's");
                assert!(count >= 1, "the ticker never ticked while {CONNECTS} connects failed");
                assert!(
                    longest < bound,
                    "the test's thread went {:.1} ms without polling the ticker while {CONNECTS} connects failed after their reply consumer; the bound is {:.1} ms",
                    longest.as_secs_f64() * 1e3,
                    bound.as_secs_f64() * 1e3
                );
            });
        }
    }

    /// A server's consumers of its own, against a broker in a container of its own per test:
    /// `listen` returns once each is assigned, within `connect_within`, and the partition watcher
    /// logs a topic whose partition count changed after `bind`. The connect bound and the
    /// watcher's refresh run on a scripted clock, so neither elapses in real time.
    #[cfg(feature = "integration")]
    mod own_consumers {
        use std::sync::Mutex as StdMutex;

        use rdkafka::admin::NewPartitions;
        use tokio::sync::Semaphore;
        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id, Record as SpanRecord};
        use tracing::{Event, Level, Metadata};
        use ulo::testing::ScriptedTimer;
        use ulo_rpc_conformance::relay::Relay;

        use super::failed_connect::kraft;
        use super::*;

        /// How long a test waits, in real time, for the link to reach a state it then acts on.
        const PATIENCE: Duration = Duration::from_secs(30);

        /// The link's `connect_within` in these tests, a length the scripted clock times.
        const WITHIN: Duration = Duration::from_secs(3);

        /// The `every_instance` topics, one per test.
        const RELEASED: &str = "own.released.every";
        const HELD: &str = "own.held.every";
        const WATCHED: &str = "own.watched.every";
        const REFUSED: &str = "own.refused.every";

        /// What the refusing seam fails the assignment with.
        const REFUSAL: &str = "the test refuses the assignment";

        /// Events produced while the assignment is held, and after `listen` returns.
        const WHILE_HELD: u32 = 5;
        const AFTER_BIND: u32 = 20;

        /// The app's clock as the link reads it: a wait of [`WITHIN`] or of [`METADATA_REFRESH`]
        /// ends once the test advances the scripted clock past it, and every other, the anchor's
        /// backoff among them, in real time.
        #[derive(Clone, Default)]
        struct Clock(ScriptedTimer);

        impl Clock {
            fn scripted(length: Duration) -> bool {
                length == WITHIN || length == METADATA_REFRESH
            }
        }

        impl Timer for Clock {
            fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
                if Clock::scripted(d) { self.0.sleep(d) } else { Box::pin(tokio::time::sleep(d)) }
            }

            fn now(&self) -> Instant {
                Instant::now()
            }

            fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
                if Clock::scripted(d) {
                    self.0.try_sleep(d)
                } else {
                    Box::pin(async move {
                        tokio::time::sleep(d).await;
                        Ok(())
                    })
                }
            }
        }

        /// Holds the assignment of a consumer whose topics include the gate's, entering it, until
        /// the test releases it; a consumer of other topics passes.
        struct Gate {
            topic: &'static str,
            entered: Semaphore,
            released: Semaphore,
        }

        impl Gate {
            const fn new(topic: &'static str) -> Self {
                Gate { topic, entered: Semaphore::const_new(0), released: Semaphore::const_new(0) }
            }

            fn hold(&'static self, topics: &[&str]) -> BoxFuture<'static, Result<(), BoxError>> {
                let holds = topics.contains(&self.topic);
                Box::pin(async move {
                    if holds {
                        self.entered.add_permits(1);
                        if let Ok(permit) = self.released.acquire().await {
                            permit.forget();
                        }
                    }
                    Ok(())
                })
            }

            /// Waits until a consumer has entered the gate, failing naming `listening` if it
            /// returned first.
            async fn entered_before(&self, listening: &mut tokio::task::JoinHandle<Result<Inbound, BoxError>>) {
                let entered = tokio::time::timeout(PATIENCE, async {
                    tokio::select! {
                        biased;
                        entered = self.entered.acquire() => entered.expect("the gate is never closed").forget(),
                        listened = &mut *listening => {
                            panic!("`listen` returned before its own consumer was assigned: {:?}", listened.map(|outcome| outcome.map(drop)))
                        }
                    }
                })
                .await;
                assert!(entered.is_ok(), "no consumer reached its assignment within {PATIENCE:?}");
            }
        }

        static RELEASED_GATE: Gate = Gate::new(RELEASED);
        static HELD_GATE: Gate = Gate::new(HELD);

        fn released_gate(topics: &[&str]) -> BoxFuture<'static, Result<(), BoxError>> {
            RELEASED_GATE.hold(topics)
        }

        fn held_gate(topics: &[&str]) -> BoxFuture<'static, Result<(), BoxError>> {
            HELD_GATE.hold(topics)
        }

        fn refusing(_topics: &[&str]) -> BoxFuture<'static, Result<(), BoxError>> {
            Box::pin(std::future::ready(Err(REFUSAL.into())))
        }

        fn server(relay: &Relay, clock: &Clock, before_assign: fn(&[&str]) -> BoxFuture<'static, Result<(), BoxError>>) -> Arc<Kafka> {
            let mut link = Kafka::brokers(relay.addr().to_string()).connect_within(WITHIN);
            link.group = Some("ulo-own-consumers".to_owned());
            link.timer = Some(Arc::new(clock.clone()));
            link.before_assign = before_assign;
            Arc::new(link)
        }

        fn listening(link: &Arc<Kafka>, topic: &'static str) -> tokio::task::JoinHandle<Result<Inbound, BoxError>> {
            let link = Arc::clone(link);
            tokio::spawn(async move { link.listen(&[Subscription::new(topic, Instances::Every)]).await })
        }

        /// Produces each of `events` to `topic` as an event, its number the payload, and waits
        /// for the broker to acknowledge each.
        async fn produce(relay: &Relay, topic: &str, events: std::ops::Range<u32>) {
            let mut config = ClientConfig::new();
            config.set("bootstrap.servers", relay.addr().to_string());
            let producer: FutureProducer = config.create().expect("a producer");
            for event in events {
                let payload = event.to_be_bytes();
                let record = FutureRecord::<str, [u8]>::to(topic).payload(&payload[..]);
                producer.send(record, Timeout::After(PATIENCE)).await.map_err(|(error, _)| error).expect("the broker took the event");
            }
            drop_detached(producer);
        }

        /// The numbers of the first `count` events `inbound` delivers, or of those it delivered
        /// within [`PATIENCE`].
        async fn events(inbound: &mut Inbound, count: usize) -> Vec<u32> {
            let mut seen = Vec::new();
            let _ = tokio::time::timeout(PATIENCE, async {
                while seen.len() < count {
                    match inbound.next().await {
                        Some(Delivery { frame: Frame::Evt { data, .. }, .. }) => {
                            seen.push(u32::from_be_bytes(data.as_bytes().try_into().expect("an event's payload is its number")));
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            })
            .await;
            seen
        }

        /// Waits, in real time, until `done` holds, failing naming `what` after [`PATIENCE`].
        async fn until(what: &str, done: impl Fn() -> bool) {
            let deadline = Instant::now() + PATIENCE;
            while !done() {
                assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }

        /// `listen` returns only once the consumer reading an `every_instance` event's topic is
        /// assigned. Held before its `assign`, `listen` has not returned while events are
        /// produced, and once released it returns and every event reaches it: those produced
        /// while the assignment was held, after the partition's end was read, and the twenty
        /// produced after `bind`. Planted break: the partitions assigned at librdkafka's logical
        /// end rather than at the end read before `assign`.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn bind_returns_once_its_own_consumer_is_assigned() {
            let (relay, _container) = kraft().await;
            let link = server(&relay, &Clock::default(), released_gate);
            let mut listening = listening(&link, RELEASED);
            RELEASED_GATE.entered_before(&mut listening).await;
            produce(&relay, RELEASED, 0..WHILE_HELD).await;
            assert!(!listening.is_finished(), "`listen` returned while its own consumer's assignment was held");

            RELEASED_GATE.released.add_permits(1);
            let listened = tokio::time::timeout(PATIENCE, listening).await.expect("`listen` returned once released").expect("the task completes");
            let mut inbound = listened.unwrap_or_else(|error| panic!("`listen` failed: {error}"));
            produce(&relay, RELEASED, WHILE_HELD..WHILE_HELD + AFTER_BIND).await;
            let seen = events(&mut inbound, (WHILE_HELD + AFTER_BIND) as usize).await;
            assert_eq!(
                seen,
                (0..WHILE_HELD + AFTER_BIND).collect::<Vec<_>>(),
                "the events read of {WHILE_HELD} produced while the assignment was held and {AFTER_BIND} produced after `bind`"
            );
            link.close().await.expect("the link closes");
        }

        /// An assignment still held when `connect_within` passes fails `listen` with
        /// `ConnectTimedOut` naming the link, the bound and the brokers. Planted break: `listen`
        /// not bounded.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_assignment_held_past_connect_within_fails_bind_naming_the_link() {
            let (relay, _container) = kraft().await;
            let clock = Clock::default();
            let link = server(&relay, &clock, held_gate);
            let mut listening = listening(&link, HELD);
            HELD_GATE.entered_before(&mut listening).await;
            assert_eq!(clock.0.live(WITHIN), 1, "the connect bound is not armed while `listen` waits on the assignment");

            clock.0.advance(WITHIN);
            let listened = tokio::time::timeout(PATIENCE, listening).await.expect("`listen` ended at the bound").expect("the task completes");
            let error = match listened {
                Ok(_) => panic!("`listen` returned with its own consumer's assignment held past the bound"),
                Err(error) => error,
            };
            let timed_out = error.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("`listen` failed otherwise: {error}"));
            assert_eq!(
                (timed_out.link(), timed_out.within(), timed_out.address()),
                ("Kafka", WITHIN, relay.addr().to_string().as_str()),
                "{error}"
            );
            assert!(lock(&link.state).server.is_none(), "a `listen` that failed left a server side");
        }

        /// The assignment of the consumers of the server's own failing fails `listen`, and no
        /// server side is left: `listen` waits on the assignment, so its outcome is the
        /// assignment's. The refusal decides the outcome rather than a timing, so a `listen` that
        /// leaves the assignment to a task fails this test however quickly or slowly it returns.
        /// Planted break: the assignment made on a task `listen` does not wait for.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_failed_assignment_fails_bind() {
            let (relay, _container) = kraft().await;
            let link = server(&relay, &Clock::default(), refusing);
            let error = match link.listen(&[Subscription::new(REFUSED, Instances::Every)]).await {
                Ok(_) => panic!("`listen` returned with its own consumers' assignment refused"),
                Err(error) => error,
            };
            assert_eq!(error.to_string(), REFUSAL, "`listen` failed otherwise");
            assert!(lock(&link.state).server.is_none(), "a `listen` that failed left a server side");
        }

        /// Advances the scripted clock past one refresh and waits for the watcher to have read
        /// the partition counts and armed its next wait.
        async fn refresh(clock: &Clock) {
            until("the partition watcher arming its wait", || clock.0.live(METADATA_REFRESH) == 1).await;
            clock.0.advance(METADATA_REFRESH);
            let now = clock.0.elapsed();
            until("the partition watcher reading the counts and arming its next wait", || clock.0.pending_at(METADATA_REFRESH) == Some(now)).await;
        }

        /// A topic read in full by a consumer of the server's own whose partition count changes
        /// after `bind` is logged at `warn` once, at the next refresh, naming the topic and both
        /// counts; a refresh finding no change logs nothing. Planted break: the watcher not
        /// spawned.
        #[test]
        fn a_partition_count_change_after_bind_is_logged_at_warn() {
            let capture = Capture::default();
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a current-thread runtime");
            tracing::subscriber::with_default(capture.clone(), || {
                runtime.block_on(async {
                    let (relay, _container) = kraft().await;
                    let clock = Clock::default();
                    let link = server(&relay, &clock, no_hold);
                    let _inbound = link.listen(&[Subscription::new(WATCHED, Instances::Every)]).await.unwrap_or_else(|error| panic!("`listen` failed: {error}"));

                    refresh(&clock).await;
                    assert_eq!(capture.warned(), Vec::<String>::new(), "a refresh finding no change logged");

                    let mut config = ClientConfig::new();
                    config.set("bootstrap.servers", relay.addr().to_string());
                    let admin: Detached<AdminClient<DefaultClientContext>> = Detached::new(config.create().expect("an admin client"));
                    for created in admin.create_partitions(&[NewPartitions::new(WATCHED, 3)], &AdminOptions::new()).await.expect("the partitions are created") {
                        created.unwrap_or_else(|(topic, code)| panic!("`{topic}` refused new partitions: {code}"));
                    }
                    let reader = Arc::clone(&lock(&link.state).server.as_ref().expect("a server side").0.every.clone().expect("an every-instance consumer"));
                    let mut counted = 0;
                    let deadline = Instant::now() + PATIENCE;
                    while counted != 3 {
                        assert!(Instant::now() < deadline, "the broker's metadata showed {counted} partitions of `{WATCHED}` after {PATIENCE:?}");
                        let reading = Arc::clone(&reader);
                        counted = blocking(move || partition_ids(&reading, WATCHED).map(|ids| ids.len())).await.expect("the read ran").expect("the metadata reads");
                    }
                    drop(reader);

                    refresh(&clock).await;
                    let warned = capture.warned();
                    let [line] = warned.as_slice() else { panic!("one `warn` for one change: {warned:?}") };
                    assert!(line.contains(&format!("topic={WATCHED}")) && line.contains("before=1") && line.contains("after=3"), "{line}");

                    refresh(&clock).await;
                    assert_eq!(capture.warned().len(), 1, "a refresh finding the changed count again logged again: {:?}", capture.warned());
                    link.close().await.expect("the link closes");
                })
            });
        }

        /// Every event on the thread it is the default subscriber of, as its level, its message
        /// and `name=value` for each other field.
        #[derive(Clone, Default)]
        struct Capture {
            events: Arc<StdMutex<Vec<(Level, String)>>>,
        }

        impl Capture {
            fn warned(&self) -> Vec<String> {
                let events = lock(&self.events);
                events.iter().filter(|(level, line)| *level == Level::WARN && line.contains("reads every partition of")).map(|(_, line)| line.clone()).collect()
            }
        }

        impl tracing::Subscriber for Capture {
            fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
                true
            }

            fn new_span(&self, _span: &Attributes<'_>) -> Id {
                Id::from_u64(1)
            }

            fn record(&self, _span: &Id, _values: &SpanRecord<'_>) {}

            fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

            fn event(&self, event: &Event<'_>) {
                let mut line = Line::default();
                event.record(&mut line);
                lock(&self.events).push((*event.metadata().level(), line.0));
            }

            fn enter(&self, _span: &Id) {}

            fn exit(&self, _span: &Id) {}
        }

        #[derive(Default)]
        struct Line(String);

        impl Visit for Line {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0.insert_str(0, &format!("{value:?}"));
                } else {
                    self.0.push_str(&format!(" {}={value:?}", field.name()));
                }
            }

            fn record_str(&mut self, field: &Field, value: &str) {
                self.0.push_str(&format!(" {}={value}", field.name()));
            }
        }
    }
}
