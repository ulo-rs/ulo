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
//! the process its id names, and `ulo:ws:all` for everyone. Each message is the broadcast in
//! [`ulo_ws::wire`]'s versioned layout. A process subscribes to its node's channel and
//! `ulo:ws:all`, and pattern-subscribes to `ulo:ws:room:*`, filtering rooms against its own
//! members on delivery, since the adapter learns of no membership change.
//!
//! A message the subscription cannot read is dropped with a `warn` and the subscription reads on.
//! One in another wire version, in the unversioned layout or naming an audience kind this build
//! does not know comes from another version of the application, as during a rolling deploy: its
//! `warn` is written once per process for each version or kind, later ones at `debug`, for up to
//! 64 of them, an audience kind remembered and logged at its first 64 bytes. A malformed message
//! is logged each time.
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
use std::sync::atomic::{AtomicBool, Ordering};
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
use ulo_ws::wire::{self, Undecodable};
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

/// How many distinct versions and audience kinds a process remembers, each logged at `warn` once
/// and held for the process's life. The first kind past them is logged at `warn` saying so, and
/// every later new kind at `debug`.
const KINDS_LOGGED: usize = 64;

/// The bytes of an audience kind a process remembers and logs. The peer names the kind at any
/// length, so a longer one is cut at a character boundary and marked with `…`, and two kinds
/// sharing their first [`KIND_SHOWN`] bytes are remembered and logged as one.
const KIND_SHOWN: usize = 64;

/// `kind`, cut to [`KIND_SHOWN`] bytes at a character boundary and marked when it was longer.
fn shown(kind: &str) -> String {
    if kind.len() <= KIND_SHOWN {
        return kind.to_owned();
    }
    let mut end = KIND_SHOWN;
    while !kind.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &kind[..end])
}

/// The versions and audience kinds of other builds this process has logged.
static SIGHTINGS: Sightings = Sightings::new();

struct Sightings {
    kinds: StdMutex<Vec<String>>,
    full: AtomicBool,
}

#[derive(Debug, PartialEq, Eq)]
enum Sighting {
    First,
    Again,
    /// The first kind past [`KINDS_LOGGED`].
    Full,
}

impl Sightings {
    const fn new() -> Self {
        Sightings { kinds: StdMutex::new(Vec::new()), full: AtomicBool::new(false) }
    }

    fn sight(&self, kind: String) -> Sighting {
        let mut kinds = self.kinds.lock().unwrap_or_else(PoisonError::into_inner);
        if kinds.contains(&kind) {
            Sighting::Again
        } else if kinds.len() < KINDS_LOGGED {
            kinds.push(kind);
            Sighting::First
        } else if self.full.swap(true, Ordering::Relaxed) {
            Sighting::Again
        } else {
            Sighting::Full
        }
    }
}

/// Logs a message the subscription on `channel` drops.
fn dropped(refused: &Undecodable, channel: &str) {
    let cut;
    let (kind, refused) = match refused {
        Undecodable::Unversioned => ("unversioned".to_owned(), refused),
        Undecodable::Version(version) => (format!("version {version}"), refused),
        Undecodable::Audience(kind) => {
            let kind = shown(kind);
            cut = Undecodable::Audience(kind.clone());
            (format!("audience {kind}"), &cut)
        }
        _ => {
            tracing::warn!(channel, %refused, "a Redis broadcast did not decode; it is dropped");
            return;
        }
    };
    match SIGHTINGS.sight(kind) {
        Sighting::First => tracing::warn!(
            channel,
            %refused,
            "a Redis broadcast from another version of the application is dropped; later ones of this kind are logged at `debug`"
        ),
        Sighting::Full => tracing::warn!(
            channel,
            %refused,
            "a Redis broadcast from another version of the application is dropped; {KINDS_LOGGED} kinds have been logged, so later new kinds are logged at `debug`"
        ),
        Sighting::Again => tracing::debug!(channel, %refused, "a Redis broadcast from another version of the application is dropped"),
    }
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
                let message = wire::encode(&target, &frame);
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
        match wire::decode(message.get_payload_bytes()) {
            Ok(item) => {
                if sender.send(item).is_err() {
                    return Ok(());
                }
            }
            Err(refused) => dropped(&refused, message.get_channel_name()),
        }
    }
    Err(BoxError::from("the Redis subscription ended"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata};
    use ulo_ws::wire::Undecodable;

    use super::{KIND_SHOWN, KINDS_LOGGED, SIGHTINGS, Sighting, Sightings, dropped, shown};

    #[test]
    fn a_kind_is_first_once_then_again() {
        let sightings = Sightings::new();
        assert_eq!(sightings.sight("version 2".to_owned()), Sighting::First);
        assert_eq!(sightings.sight("version 2".to_owned()), Sighting::Again);
        assert_eq!(sightings.sight("version 3".to_owned()), Sighting::First);
        assert_eq!(sightings.sight("version 2".to_owned()), Sighting::Again);
    }

    #[test]
    fn the_first_kind_past_the_bound_is_full_once_and_kinds_seen_stay_again() {
        let sightings = Sightings::new();
        for n in 0..KINDS_LOGGED {
            assert_eq!(sightings.sight(format!("audience {n}")), Sighting::First, "kind {n}");
        }
        assert_eq!(sightings.sight("audience past".to_owned()), Sighting::Full);
        assert_eq!(sightings.sight("audience further".to_owned()), Sighting::Again);
        assert_eq!(sightings.sight("audience past".to_owned()), Sighting::Again);
        assert_eq!(sightings.sight("audience 0".to_owned()), Sighting::Again);
    }

    /// The process's sightings are one static, which no test resets: each test that reaches it
    /// drops kinds no other test in this binary drops.
    #[test]
    fn a_broadcast_from_another_version_is_logged_at_warn_once_per_kind() {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            dropped(&Undecodable::Version(201), "ulo:ws:all");
            dropped(&Undecodable::Version(201), "ulo:ws:all");
            dropped(&Undecodable::Audience("tests-only".to_owned()), "ulo:ws:room:lobby");
            dropped(&Undecodable::Version(201), "ulo:ws:all");
            dropped(&Undecodable::Audience("tests-only".to_owned()), "ulo:ws:room:lobby");
        });
        let warned: Vec<String> = capture.lines(Level::WARN);
        assert_eq!(warned.len(), 2, "one `warn` per kind: {warned:?}");
        assert!(warned[0].contains("version 201") && warned[0].contains("channel=ulo:ws:all"), "{}", warned[0]);
        assert!(warned[1].contains("`tests-only`") && warned[1].contains("channel=ulo:ws:room:lobby"), "{}", warned[1]);
        assert_eq!(capture.lines(Level::DEBUG).len(), 3, "the repeats at `debug`");
    }

    /// Each version and each audience kind is a kind of its own: a second one is logged at `warn`
    /// beside the first.
    #[test]
    fn each_version_and_each_audience_kind_is_logged_at_warn() {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            dropped(&Undecodable::Version(202), "ulo:ws:all");
            dropped(&Undecodable::Version(203), "ulo:ws:all");
            dropped(&Undecodable::Audience("tests-only-a".to_owned()), "ulo:ws:all");
            dropped(&Undecodable::Audience("tests-only-b".to_owned()), "ulo:ws:all");
        });
        let warned = capture.lines(Level::WARN);
        assert_eq!(warned.len(), 4, "one `warn` for each of two versions and two audience kinds: {warned:?}");
    }

    const MIB: usize = 1 << 20;

    /// A kind is kept whole up to [`KIND_SHOWN`] bytes, and a longer one is cut at the last
    /// character boundary within them and marked.
    #[test]
    fn a_kind_past_the_bytes_shown_is_cut_at_a_character_boundary_and_marked() {
        let exact = "k".repeat(KIND_SHOWN);
        assert_eq!(shown(&exact), exact);
        assert_eq!(shown(&format!("{exact}k")), format!("{exact}…"));
        // "é" is two bytes, so byte 64 falls inside one and the cut moves back to byte 63.
        let wide = format!("k{}", "é".repeat(KIND_SHOWN));
        assert_eq!(shown(&wide), format!("{}…", &wide[..KIND_SHOWN - 1]));
    }

    /// A mebibyte audience kind is remembered and logged at its first [`KIND_SHOWN`] bytes: it
    /// holds no more of the process's memory, and writes no more into its log, than a short one.
    #[test]
    fn a_long_audience_kind_is_remembered_and_logged_short() {
        let kind = format!("tests-long-{}", "k".repeat(MIB));
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || dropped(&Undecodable::Audience(kind.clone()), "ulo:ws:all"));
        let warned = capture.lines(Level::WARN);
        let [line] = warned.as_slice() else { panic!("one `warn`: {} lines", warned.len()) };
        let cut = format!("{}…", &kind[..KIND_SHOWN]);
        assert!(line.contains(&format!("`{cut}`")), "the `warn` names the kind cut short");
        assert!(line.len() < 512, "the `warn` is {} bytes long", line.len());
        let remembered = SIGHTINGS.kinds.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(remembered.contains(&format!("audience {cut}")), "the kind is remembered cut short");
        let longest = remembered.iter().map(String::len).max().unwrap_or(0);
        assert!(longest <= "audience ".len() + KIND_SHOWN + '…'.len_utf8(), "a remembered kind is {longest} bytes long");
    }

    /// Two kinds that differ only past their first [`KIND_SHOWN`] bytes are one kind: the second
    /// is logged at `debug`, as a repeat of the first.
    #[test]
    fn two_long_kinds_sharing_the_bytes_shown_are_one_kind() {
        let prefix = format!("tests-shared-{}", "k".repeat(KIND_SHOWN));
        let first = format!("{prefix}{}", "a".repeat(MIB));
        let second = format!("{prefix}{}", "b".repeat(MIB));
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            dropped(&Undecodable::Audience(first), "ulo:ws:all");
            dropped(&Undecodable::Audience(second), "ulo:ws:all");
        });
        assert_eq!(capture.lines(Level::WARN).len(), 1, "the second kind was logged as a new one");
        assert_eq!(capture.lines(Level::DEBUG).len(), 1, "the second kind was not logged as a repeat");
    }

    #[test]
    fn a_malformed_broadcast_is_logged_at_warn_each_time() {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            dropped(&Undecodable::Malformed("no newline ends the target".to_owned()), "ulo:ws:all");
            dropped(&Undecodable::Malformed("no newline ends the target".to_owned()), "ulo:ws:all");
        });
        let warned = capture.lines(Level::WARN);
        assert_eq!(warned.len(), 2, "{warned:?}");
        assert!(warned.iter().all(|line| line.contains("did not decode") && line.contains("no newline")), "{warned:?}");
    }

    /// Every event on the thread it is the default subscriber of, as its level, its message and
    /// `name=value` for each other field.
    #[derive(Clone, Default)]
    struct Capture {
        events: Arc<Mutex<Vec<(Level, String)>>>,
    }

    impl Capture {
        fn lines(&self, level: Level) -> Vec<String> {
            let events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
            events.iter().filter(|(at, _)| *at == level).map(|(_, line)| line.clone()).collect()
        }
    }

    impl tracing::Subscriber for Capture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut line = Line::default();
            event.record(&mut line);
            self.events.lock().unwrap_or_else(PoisonError::into_inner).push((*event.metadata().level(), line.0));
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
