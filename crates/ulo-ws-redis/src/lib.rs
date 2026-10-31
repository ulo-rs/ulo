//! The Redis broadcast adapter for `ulo-ws` (transports DESIGN §4.3): one Pub/Sub channel per room
//! or client, each process delivering to its own members. Membership stays local to each process
//! by construction, delivery is best effort and ordered per sender within one process, and the
//! API is the in-memory adapter's.
//!
//! ```
//! # use ulo::module;
//! #[module(imports = [ulo_ws::WsModule::for_root().broadcast(ulo_ws_redis::Redis::url("redis://cache:6379"))])]
//! pub struct AppModule;
//! ```
//!
//! The crate ships the adapter value and no module: a second module binding
//! `dyn BroadcastAdapter` beside `WsModule`'s would be `DuplicateBinding` at `wire()`.
//!
//! The channels: `ulo:ws:room:<room>` for a room, `ulo:ws:node:<node>` for a client, addressed to
//! the process its id names, and `ulo:ws:all` for everyone. Each message is the target as JSON, a
//! newline, then the encoded broadcast. A process subscribes to its node's channel and
//! `ulo:ws:all`, and pattern-subscribes to `ulo:ws:room:*`, filtering rooms against its own
//! members on delivery, since the adapter learns of no membership change.
//!
//! A server's `listen()` returns once Redis has confirmed the adapter's subscription, so a
//! broadcast published after it reaches the process's own members as well as the other
//! processes'. A subscription not confirmed within [`Redis::connect_within`] fails `listen()` as
//! `Bind`, the last connection error as its cause. The bound and the reconnect delay run on the
//! app's `Timer`, so the adapter's runtime needs no time driver.
//!
//! The adapter's connections and its subscription task live on the tokio runtime it holds, the one
//! current where it was built or the one [`Redis::with_handle`] names, so `publish` and the
//! subscription's stream may be polled on any executor. An adapter built outside a runtime and
//! given none fails its `prepare`, naming `.with_handle(..)`, which `WsModule`'s init hook calls,
//! so the app fails `connect` whether or not it serves a gateway. A publish reaching such an
//! adapter anyway fails with the same refusal.

use std::fmt;
use std::pin::pin;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use redis::aio::MultiplexedConnection;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, mpsc, oneshot};
use ulo::{BoxError, BoxFuture, Cut, Permanent, Refused, RuntimeShutDown, Spawn, Timer, timeout};
use ulo_runtime_tokio::Tokio;
use ulo_ws::{Audience, BroadcastAdapter, NodeId, Target};

/// How long a lost subscription waits before connecting again.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// How long `subscribe` waits for Redis to confirm the subscription, unless
/// [`Redis::connect_within`] says otherwise.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// The Redis adapter. The URL is parsed and the connection made lazily, at the first publish or
/// subscribe; `rediss://` selects TLS.
#[derive(Clone)]
pub struct Redis {
    pub(crate) url: String,
    runtime: Option<Tokio>,
    connect_within: Duration,
    publisher: Arc<Mutex<Option<MultiplexedConnection>>>,
}

impl Redis {
    /// The adapter on `url`, `redis://host:6379` or `rediss://host:6380`.
    pub fn url(url: impl Into<String>) -> Self {
        Redis {
            url: url.into(),
            runtime: Tokio::try_current(),
            connect_within: CONNECT_WITHIN,
            publisher: Arc::new(Mutex::new(None)),
        }
    }

    /// How long a server's `listen()` waits for Redis to confirm the adapter's subscription, 30
    /// seconds by default, timed on the app's `Timer` and not on the adapter runtime's clock. A
    /// subscription that is not live by then fails `listen()` as `Bind`, with the last connection
    /// error as its source; a `Timer` whose runtime has shut down fails it as `Permanent`
    /// `RuntimeShutDown`. A zero bound is refused by `prepare`.
    pub fn connect_within(mut self, bound: Duration) -> Self {
        self.connect_within = bound;
        self
    }

    /// The tokio runtime the adapter's connections and subscription run on, in place of the one
    /// current where it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }
}

const NO_RUNTIME: &str = "the Redis broadcast adapter has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

fn channel(target: &Target) -> String {
    match &target.audience {
        Audience::Room(room) => format!("ulo:ws:room:{room}"),
        Audience::Client(id) => format!("ulo:ws:node:{}", id.node()),
        _ => "ulo:ws:all".to_owned(),
    }
}

fn encode(target: &Target, frame: &[u8]) -> Result<Vec<u8>, serde_json::Error> {
    let mut message = serde_json::to_vec(target)?;
    message.push(b'\n');
    message.extend_from_slice(frame);
    Ok(message)
}

fn decode(message: &[u8]) -> Option<(Target, Bytes)> {
    let split = message.iter().position(|&byte| byte == b'\n')?;
    let target = serde_json::from_slice(&message[..split]).ok()?;
    Some((target, Bytes::copy_from_slice(&message[split + 1..])))
}

impl BroadcastAdapter for Redis {
    /// Refuses an adapter with no tokio runtime, naming `.with_handle(..)`, one whose runtime was
    /// built without its IO driver ([`Tokio::require_io`]), and a zero
    /// [`connect_within`](Redis::connect_within).
    fn prepare(&self) -> Result<(), BoxError> {
        self.runtime()?.require_io()?;
        if self.connect_within.is_zero() {
            return Err("the Redis broadcast adapter's `connect_within` is zero, so no subscription could be confirmed".into());
        }
        Ok(())
    }

    /// A failed publish drops the cached connection, so the next publish connects again. The
    /// connection and the `PUBLISH` run on the adapter's runtime: redis times each answer on
    /// tokio's clock.
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        let runtime = self.runtime();
        let url = self.url.clone();
        let publisher = Arc::clone(&self.publisher);
        Box::pin(async move {
            let published = runtime?.run(async move {
                let message = encode(&target, &frame)?;
                let mut cached = publisher.lock().await;
                let mut connection = match cached.take() {
                    Some(connection) => connection,
                    None => redis::Client::open(url.as_str())?.get_multiplexed_async_connection().await?,
                };
                redis::cmd("PUBLISH").arg(channel(&target)).arg(message.as_slice()).query_async::<i64>(&mut connection).await?;
                *cached = Some(connection);
                Ok::<_, BoxError>(())
            });
            published.await?
        })
    }

    /// Runs the subscription on its own task on the adapter's runtime and resolves once Redis has
    /// confirmed every `SUBSCRIBE`, so a broadcast published after it resolves reaches the stream,
    /// this process's own members' included. The task reconnects after a second when Redis goes
    /// away, and broadcasts published while it is away are lost, as Pub/Sub delivers to
    /// subscribers present. Redis that confirms nothing within [`connect_within`](Redis::connect_within)
    /// fails the future with the last connection error as its source, and the task ends. The bound
    /// and the reconnect delay run on `timer`, the app's clock, never the adapter runtime's: a
    /// clock that refuses, its runtime having shut down, fails the future as `Permanent`
    /// `RuntimeShutDown` and ends the task, as does an adapter runtime that has shut down. The
    /// task ends, closing its Pub/Sub connection, once the returned stream or the future drops,
    /// whether it was waiting on Redis, on a message or on the reconnect delay. A server subscribes
    /// only once `prepare` has passed, so an adapter with no runtime is refused before this; called
    /// anyway, the future fails with the same refusal.
    fn subscribe(&self, node: NodeId, timer: Arc<dyn Timer>) -> BoxFuture<'static, Result<BoxStream<'static, (Target, Bytes)>, BoxError>> {
        let runtime = self.runtime();
        let url = self.url.clone();
        let within = self.connect_within;
        Box::pin(async move {
            let runtime = runtime?;
            let (sender, receiver) = mpsc::unbounded_channel::<(Target, Bytes)>();
            let (confirm, confirmed) = oneshot::channel::<()>();
            let progress = Arc::new(StdMutex::new(Progress::default()));
            let recorded = Arc::clone(&progress);
            let pacing = Arc::clone(&timer);
            // Dropping `receiver`, with this future or the stream, ends the task through `sender.closed()`.
            let subscription = async move {
                let subscription = async {
                    let mut confirm = Some(confirm);
                    loop {
                        if let Err(error) = listen(&url, node, &sender, &mut confirm).await {
                            tracing::warn!(%error, "the Redis broadcast subscription failed; reconnecting");
                            recorded.lock().unwrap_or_else(PoisonError::into_inner).last_error = Some(error);
                        }
                        if pacing.try_sleep(RECONNECT_DELAY).await.is_err() {
                            recorded.lock().unwrap_or_else(PoisonError::into_inner).refused = true;
                            return;
                        }
                    }
                };
                futures_util::future::select(pin!(sender.closed()), pin!(subscription)).await;
            };
            if let Err(refused) = runtime.try_spawn(Box::pin(subscription)) {
                return Err(Permanent::new(RuntimeShutDown::new(refused)).into());
            }
            let last = || progress.lock().unwrap_or_else(PoisonError::into_inner).last_error.take();
            match timeout(&*timer, within, confirmed).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    let refused = progress.lock().unwrap_or_else(PoisonError::into_inner).refused;
                    return Err(if refused {
                        let ended = last().map_or_else(|| RuntimeShutDown::new(Refused), RuntimeShutDown::after);
                        Permanent::new(ended).into()
                    } else {
                        "the Redis broadcast subscription ended before Redis confirmed it".into()
                    });
                }
                Err(Cut::Expired) => return Err(SubscribeTimedOut { within, source: last() }.into()),
                Err(Cut::Refused(refused)) => return Err(Permanent::new(RuntimeShutDown::new(refused)).into()),
            }
            let carried: BoxStream<'static, (Target, Bytes)> = Box::pin(futures_util::stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|item| (item, receiver))
            }));
            Ok(carried)
        })
    }
}

/// What the subscription task tells the waiting `subscribe`.
#[derive(Default)]
struct Progress {
    last_error: Option<BoxError>,
    /// The app's clock refused the reconnect delay, so the task ended.
    refused: bool,
}

/// Redis confirmed no subscription within the adapter's connect bound; the last connection error,
/// when an attempt failed rather than hung, is the source.
#[derive(Debug)]
struct SubscribeTimedOut {
    within: Duration,
    source: Option<BoxError>,
}

impl fmt::Display for SubscribeTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Redis did not confirm the broadcast subscription within {:?}", self.within)?;
        match &self.source {
            Some(source) => write!(f, ": {source}"),
            None => f.write_str(": no connection attempt failed, Redis did not answer"),
        }
    }
}

impl std::error::Error for SubscribeTimedOut {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// One subscription, until Redis goes away or a send finds the stream's reader gone; the caller
/// drops it as the reader drops. `confirm` fires once, when every subscription is in place.
async fn listen(
    url: &str,
    node: NodeId,
    sender: &mpsc::UnboundedSender<(Target, Bytes)>,
    confirm: &mut Option<oneshot::Sender<()>>,
) -> Result<(), BoxError> {
    let mut pubsub = redis::Client::open(url)?.get_async_pubsub().await?;
    pubsub.subscribe("ulo:ws:all").await?;
    pubsub.subscribe(format!("ulo:ws:node:{node}")).await?;
    pubsub.psubscribe("ulo:ws:room:*").await?;
    // Each call above returns once Redis has answered it; the first pass to get here releases the
    // waiting `subscribe`.
    if let Some(confirm) = confirm.take() {
        let _ = confirm.send(());
    }
    let mut messages = pubsub.into_on_message();
    while let Some(message) = messages.next().await {
        match decode(message.get_payload_bytes()) {
            Some(item) => {
                if sender.send(item).is_err() {
                    return Ok(());
                }
            }
            None => tracing::warn!(channel = message.get_channel_name(), "a Redis broadcast did not decode; it is dropped"),
        }
    }
    Err(BoxError::from("the Redis subscription ended"))
}
