use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use async_nats::connection::State as Connection;
use async_nats::rustls::{ClientConfig, RootCertStore};
use async_nats::{Client, ConnectOptions, Event, HeaderMap, Message, ServerAddr, StatusCode, Subscriber};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use rustls_native_certs::CertificateResult;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use ulo::{AppHandle, Bound, BoxError, BoxFuture, Runtime};
use ulo_runtime_tokio::Tokio;
#[cfg(feature = "conformance-probe")]
use ulo_rpc::__private::ClientProbe;
use ulo_rpc::__private::HoldUntil;
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link, Misconfigured,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::prepare::{Failures, zero_bound};
use ulo_transport::{Detail, Details, ErrorKind};

/// The header naming a message's frame kind where the message alone does not tell it: `open` on
/// the request lane, `in`, `in_end` and `cancel` on the control lane, `opened` on the reply lane.
/// A request-lane message without it is a `req` when it carries a reply subject and an `evt` when
/// it does not, so a body published from the NATS CLI reaches a handler.
const KIND: &str = "ulo-t";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The subject every server instance subscribes to without a queue group, so each sees every
/// control message and acts on the ones whose reply subject names a call it holds. A streamed
/// request's items and every `cancel` travel here: the request lane hands each message to one
/// instance of the group, and these have to reach the instance that took the call.
const CONTROL: &str = "ulo.rpc.control";

/// The `ErrorInfo` domain of the `no_destination` error this link writes for no-responders.
const DOMAIN: &str = "ulo.rpc";

/// The NATS link.
///
/// The link's connections and tasks live on the tokio runtime it holds, the one current where it
/// was built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may
/// be polled on any executor or on a plain thread: each connection is opened there and its task
/// runs there, and a publish, a subscription, a flush or a drain reaches that task through
/// async-nats's command channel. A link built outside a runtime and given none refuses in
/// `usable`, so a client is refused where it takes the link and a server's `listen()` fails, and
/// in `connect`. A link dropped without `close` aborts its lanes and reply routers, and the
/// connections they held close with them.
///
/// A connection over TLS trusts the platform's root certificates. The process reads them once
/// while any link holds them, off the runtime's workers, on the blocking pool of the link that
/// first needs them, a server's in `prepare` or a client's at its first connect, and writes them
/// as PEM to one file in the OS temp directory, `ulo-rpc-nats-roots-<pid>-<n>.pem`. A link that
/// needs them while another link or a connection holds that file reuses it, and a process holds
/// one load and one file at a time; the file is removed once its last holder drops it, and a link
/// needing them after that loads again. A process killed before the drop leaves its one file in the
/// temp directory; the roots are public data and the name is unique, so a leftover costs only
/// space, and no link removes a file another process wrote.
///
/// async-nats reads the file on the runtime's blocking pool whenever a connection or a reconnect
/// handed it uses TLS: a `tls://` url requires it, and a `nats://` connection is upgraded when the
/// server's `INFO` requires it. A connection that stays plain never reads it. async-nats keeps the
/// options a connection was made with for every reconnect of it, so a connection made without the
/// file reconnects without it for as long as it lives; if its server upgrades it, async-nats reads
/// the platform's store itself, on the thread polling the connection, at that connect and at each
/// reconnect.
///
/// A server's `listen` therefore waits for the load, on `nats://` and `tls://` alike, since its
/// one connection serves the app's whole life, and so does a `tls://` client's connect, which
/// cannot be made without the file; each waits at most [`roots_timeout`](Self::roots_timeout). A
/// `nats://` client waits for nothing: a connection it makes once the load has ended is handed
/// the file, and one made before is not, and its next connection after a lost one is made afresh.
///
/// Over `tls://`, a store yielding no root, a file that cannot be written, a load outlasting
/// `roots_timeout`, or a process that links more than one rustls crypto provider and installs
/// none fails a server's `bind` and every client call `Internal` with
/// `reason: "misconfigured_link"`. Over `nats://` the first two are logged at `warn` once and the
/// link connects without the file, since a plain server needs none of it; a connection the server
/// upgrades then fails as async-nats's own read of the store does. A `nats://` server whose load
/// outlasts `roots_timeout` connects without the file and logs it at `warn`. The provider is
/// resolved for `tls://` alone, so a provider conflict is neither checked nor logged over
/// `nats://`.
pub struct Nats {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The server's `max_payload` from its `INFO`, zero until a connection reads it.
    pub(crate) max_payload: Arc<AtomicU64>,
    pub(crate) roots: Roots,
    pub(crate) roots_timeout: Bound,
    pub(crate) state: Mutex<State>,
    #[cfg(feature = "conformance-probe")]
    pub(crate) probe: Arc<ClientProbe>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<Arc<ServerSide>>,
    client: Option<Client>,
    /// The server's lanes and each connection's reply router, aborted when the link drops. Each
    /// holds its connection, which would otherwise stay open for the life of the process after a
    /// link dropped without `close` (F428).
    tasks: Vec<AbortOnDrop>,
}

impl State {
    /// Holds `task` until the link drops, letting go of tasks that have ended.
    fn hold(&mut self, task: AbortHandle) {
        self.tasks.retain(|held| !held.finished());
        self.tasks.push(AbortOnDrop(task));
    }
}

/// Aborts a task when dropped.
struct AbortOnDrop(AbortHandle);

impl AbortOnDrop {
    fn finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// How long a link that waits for its roots waits at `Bound::Default`.
const ROOTS_TIMEOUT: Duration = Duration::from_secs(30);

impl Nats {
    /// The link on `url`, `nats://host:4222` or `tls://host:4222`, parsed in `prepare` and
    /// connected lazily: the server side at `bind`, the client side on its first call. Several
    /// servers are written comma-separated.
    pub fn url(url: impl Into<String>) -> Self {
        Nats {
            url: url.into(),
            group: None,
            codec: Codec::Json,
            runtime: Tokio::try_current(),
            default_group: None,
            max_payload: Arc::new(AtomicU64::new(0)),
            roots: Roots::platform(),
            roots_timeout: Bound::Default,
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

    /// The queue group server instances share, in place of the root module's full type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// The tokio runtime the link's connections and tasks run on, in place of the one current
    /// where it was built.
    pub fn with_handle(mut self, handle: Handle) -> Self {
        self.runtime = Some(Tokio::from_handle(handle));
        self
    }

    /// How long a server's `listen`, and a `tls://` client's connect, wait for the platform's root
    /// certificates to load: 30 s at `Bound::Default`, as long as the load takes at
    /// `Bound::Unbounded`, timed on the link's runtime. On expiry a `tls://` link is refused: a
    /// server's `bind` fails `StartupError::Configure`, and each client call `Internal` with
    /// `reason: "misconfigured_link"`. A `nats://` server connects without the file and logs it at
    /// `warn`; if its server upgrades the connection, async-nats reads the platform's store on the
    /// thread polling it, at that connect and at every reconnect. A `nats://` client never waits.
    /// The load goes on after an expiry, and a later wait joins it. `Bound::After(Duration::ZERO)`
    /// is refused: by a client when it is built (`Link::usable`), by a server in `prepare`.
    pub fn roots_timeout(mut self, timeout: Bound) -> Self {
        self.roots_timeout = timeout;
        self
    }

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }

    /// The load of root certificates the servers in `url` connect with, started for every url:
    /// async-nats upgrades a `nats://` connection whose server requires TLS, and the link cannot
    /// know before connecting whether one will. A url naming a `tls://` server resolves the crypto
    /// provider (see [`resolve_provider`]), waits for the load under `roots_timeout` and is
    /// refused if it failed or outlasted the bound. A url naming none waits under the bound when
    /// `waits`, a server's `listen`, and connects without the file on a failure or an expiry;
    /// otherwise it waits for nothing: `Some` once the file is written, `None` while the load runs
    /// or after it failed.
    async fn roots(&self, servers: &[ServerAddr], runtime: &Tokio, waits: bool) -> Result<Option<Arc<Load>>, BoxError> {
        if servers.iter().any(ServerAddr::tls_required) {
            self.roots.wait(runtime, true, self.roots_timeout).await.map(Some)
        } else if waits {
            Ok(self.roots.awaited(runtime, self.roots_timeout).await)
        } else {
            Ok(self.roots.ready(runtime))
        }
    }
}

const NO_RUNTIME: &str = "the NATS link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

impl Link for Nats {
    const NAME: &'static str = "nats";

    fn capabilities(&self) -> Capabilities {
        let max_payload = self.max_payload.load(Ordering::Relaxed);
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerPublisher)
            .miss_signal(true)
            .confirms_drain(false)
            .max_frame((max_payload > 0).then_some(max_payload))
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime()?;
        if let Some(zero) = zero_bound(
            "roots_timeout",
            self.roots_timeout,
            "refuse every `tls://` link and connect every `nats://` server without its root certificates",
        ) {
            let mut failures = Failures::new();
            failures.push(zero);
            return Err(failures.into_error().into());
        }
        Ok(())
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.usable()?;
        let servers = servers(&self.url)?;
        match &self.group {
            Some(group) => check_group(group)?,
            None => self.default_group = Some(queue_group(&format!("{:#}", app.root().name()))),
        }
        // A server starts its roots' load here, so over `tls://` a store that yields none fails
        // `bind` before `listen`; over `nats://` `listen` waits for it.
        self.roots(&servers, &self.runtime()?, false).await?;
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the NATS link's queue group is unset: `listen` ran before `prepare`")?;
        let runtime = self.runtime()?;
        let servers = servers(&self.url)?;
        // The server's one connection reconnects with the options it is made with for the app's
        // life, so it waits for the file rather than connect without it.
        let roots = self.roots(&servers, &runtime, true).await?;
        let client = runtime.run(open(servers, ConnectOptions::new(), roots.as_deref().and_then(Load::path))).await??;
        self.max_payload.store(client.server_info().max_payload as u64, Ordering::Relaxed);

        // Every subscription is made before any lane starts, so a failure leaves nothing
        // subscribed: a dropped `Subscriber` unsubscribes.
        let mut lanes = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            match client.queue_subscribe(pattern.as_str().to_owned(), group.clone()).await {
                Ok(subscriber) => lanes.push(subscriber),
                Err(error) => {
                    drop(lanes);
                    let _ = client.drain().await;
                    return Err(format!("the NATS link could not subscribe to `{pattern}`: {error}").into());
                }
            }
        }
        let control = match client.subscribe(CONTROL).await {
            Ok(subscriber) => subscriber,
            Err(error) => {
                drop(lanes);
                let _ = client.drain().await;
                return Err(format!("the NATS link could not subscribe to `{CONTROL}`: {error}").into());
            }
        };
        if let Err(error) = client.flush().await {
            let _ = client.drain().await;
            return Err(format!("the NATS link's subscriptions did not reach the server: {error}").into());
        }

        let (phase, _) = watch::channel(Phase::Serving);
        let side = Arc::new(ServerSide { client, codec: self.codec, calls: Arc::new(Calls::new()), phase, _roots: roots });
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let handle = runtime.handle();
        let mut state = lock(&self.state);
        for subscriber in lanes {
            state.hold(handle.spawn(request_lane(Arc::clone(&side), subscriber, deliveries.clone())).abort_handle());
        }
        state.hold(handle.spawn(control_lane(Arc::clone(&side), control, deliveries)).abort_handle());
        state.server = Some(side);
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        // async-nats reconnects by itself and resubscribes the inbox, but what was published to
        // the inbox while the connection was down is gone, so a disconnect ends the reply lane:
        // `RpcClient` fails the calls waiting on it `Unavailable` and connects again for the next.
        let runtime = self.runtime()?;
        let servers = servers(&self.url)?;
        // A client's next connection after a lost one is made here afresh, so on `nats://` it
        // waits for no load: a connection made before the load ends costs that connection alone.
        let roots = self.roots(&servers, &runtime, false).await?;
        let (lost, disconnected) = watch::channel(false);
        let options = ConnectOptions::new().event_callback(move |event| {
            if matches!(event, Event::Disconnected) {
                lost.send_replace(true);
            }
            async {}
        });
        let client = runtime.run(open(servers, options, roots.as_deref().and_then(Load::path))).await??;
        self.max_payload.store(client.server_info().max_payload as u64, Ordering::Relaxed);
        let prefix = client.new_inbox();
        let replies = client.subscribe(format!("{prefix}.*")).await?;
        // The reply subscription reaches the server before the first request can, so no reply
        // arrives ahead of it.
        client.flush().await?;

        let side = Arc::new(ClientSide {
            client: client.clone(),
            codec: self.codec,
            runtime: runtime.clone(),
            prefix,
            max_payload: Arc::clone(&self.max_payload),
            calls: Mutex::new(HashMap::new()),
            _roots: roots,
            #[cfg(feature = "conformance-probe")]
            probe: Arc::clone(&self.probe),
        });
        #[cfg(feature = "conformance-probe")]
        {
            let (late, counted) = (Arc::downgrade(&side), Arc::downgrade(&side));
            self.probe.attach(
                move |call| {
                    if let Some(side) = late.upgrade() {
                        side.open_gate(call);
                    }
                },
                move || counted.upgrade().map_or(0, |side| lock(&side.calls).len()),
            );
        }
        let (frames, replies_out) = mpsc::unbounded_channel();
        let router = runtime.handle().spawn(route_replies(Arc::clone(&side), replies, frames, disconnected));
        let mut state = lock(&self.state);
        state.client = Some(client);
        state.hold(router.abort_handle());
        drop(state);

        Ok(Outbound {
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let side = Arc::clone(&side);
                Box::pin(async move { side.send(pattern, frame).await })
            }),
            replies: receiver_stream(replies_out),
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.clone();
        if let Some(side) = server {
            side.phase.send_replace(Phase::Draining);
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        let (server, client) = {
            let mut state = lock(&self.state);
            (state.server.take(), state.client.take())
        };
        let mut failure = None;
        if let Some(side) = server {
            side.phase.send_replace(Phase::Closed);
            side.calls.clear();
            if let Err(error) = shut(&side.client).await {
                failure = Some(error);
            }
        }
        if let Some(client) = client
            && let Err(error) = shut(&client).await
        {
            failure = Some(error);
        }
        match failure {
            Some(error) => Err(format!("the NATS link did not close cleanly: {error}").into()),
            None => Ok(()),
        }
    }
}

/// Closes one connection once the server has routed everything it published.
///
/// async-nats's `publish` and `drain` only queue a command for the connection's task, and a task
/// that finds its command channel closed exits without writing what it already took from it. Once
/// `close` drops the last `Client`, the replies of the calls the drain let finish could be lost
/// that way. NATS routes one connection's messages in order, so a message to the connection's own
/// inbox coming back shows every earlier one was routed. Skipped while disconnected: async-nats
/// writes nothing it queued until it reconnects, and the close drops the connection first.
async fn shut(client: &Client) -> Result<(), BoxError> {
    if client.connection_state() == Connection::Connected {
        let subject = client.new_inbox();
        let mut echo = client.subscribe(subject.clone()).await?;
        client.publish(subject, Bytes::new()).await?;
        if echo.next().await.is_none() {
            return Err("the connection closed before its last messages were routed".into());
        }
    }
    client.drain().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// The server side of a bound link: the connection, the calls it holds, and the phase its lanes
/// watch.
struct ServerSide {
    client: Client,
    codec: Codec,
    calls: Arc<Calls>,
    phase: watch::Sender<Phase>,
    /// The load whose file the connection was handed, held while the connection may reconnect,
    /// since each reconnect reads the file again.
    _roots: Option<Arc<Load>>,
}

/// A held call's reply path holds the table it is held in, so a side dropped with calls held, its
/// link dropped without `close`, would keep them and the connection their paths hold (F428).
impl Drop for ServerSide {
    fn drop(&mut self) {
        self.calls.clear();
    }
}

impl ServerSide {
    async fn on_request(&self, message: Message, deliveries: &mpsc::UnboundedSender<Delivery>) {
        let pattern = message.subject.to_string();
        let kind = kind_of(message.headers.as_ref());
        let headers = call_headers(message.headers.as_ref());
        let (frame, path) = match (message.reply, kind.as_deref()) {
            (None, None) => {
                let frame = Frame::Evt { pattern, headers, data: Data::new(message.payload) };
                let _ = deliveries.send(Delivery { frame, reply: None, ack: Ack::none() });
                return;
            }
            (Some(reply), None) => {
                let (id, path) = self.hold(reply.to_string());
                (Frame::Req { id, pattern, headers, data: Data::new(message.payload) }, path)
            }
            (Some(reply), Some(OPEN)) => {
                let subject = reply.to_string();
                let (id, path) = self.hold(subject.clone());
                // The caller holds the request's items until this arrives, so they reach the
                // control lane after the call is held here.
                let opened = publish(&self.client, subject, None, &CallHeaders::new(), Some(OPENED), Bytes::new()).await;
                if let Err(error) = opened {
                    tracing::warn!(%error, pattern, "the NATS link could not acknowledge a streamed request");
                }
                (Frame::Open { id, pattern, headers }, path)
            }
            (reply, Some(kind)) => {
                tracing::warn!(pattern, kind, reply = reply.is_some(), "the NATS link dropped a request-lane message of an unknown kind");
                return;
            }
        };
        let _ = deliveries.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
    }

    fn on_control(&self, message: Message, deliveries: &mpsc::UnboundedSender<Delivery>) {
        let Some(key) = message.reply.as_ref().map(ToString::to_string) else { return };
        let frame = match kind_of(message.headers.as_ref()).as_deref() {
            Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(message.payload) }, path)),
            Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
            Some(CANCEL) => self.calls.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
            _ => None,
        };
        if let Some((frame, path)) = frame {
            let _ = deliveries.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
        }
    }

    /// Holds a call under its reply subject, the key its control messages carry.
    fn hold(&self, subject: String) -> (u64, ReplyPath) {
        let key = subject.clone();
        let client = self.client.clone();
        let codec = self.codec;
        let calls = Arc::clone(&self.calls);
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            let client = client.clone();
            let calls = Arc::clone(&calls);
            let subject = subject.clone();
            Box::pin(async move {
                if is_terminal(&frame) {
                    calls.release(&subject);
                }
                let bytes = codec.encode_frame(&frame)?;
                client.publish(subject, bytes).await?;
                Ok(())
            })
        });
        let id = self.calls.hold(key, path.clone());
        (id, path)
    }
}

/// The calls a server holds, by the key their control messages carry, each with its local id and
/// its reply path. The local id is this link's own, unique across every caller, so a frame's
/// `id` never collides between two clients that each count from zero.
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

async fn request_lane(side: Arc<ServerSide>, mut subscriber: Subscriber, deliveries: mpsc::UnboundedSender<Delivery>) {
    let mut phase = side.phase.subscribe();
    let mut draining = false;
    loop {
        tokio::select! {
            changed = phase.changed(), if !draining => {
                let now = if changed.is_ok() { *phase.borrow_and_update() } else { Phase::Closed };
                match now {
                    Phase::Serving => {}
                    // NATS's drain: the server stops sending to this subscription once it has
                    // processed the UNSUB, and the subscriber ends. async-nats removes the
                    // subscription on its connection task's next poll, without waiting for the
                    // server, and discards a message for it that arrives later, which the link
                    // declares as `confirms_drain(false)`.
                    Phase::Draining => {
                        draining = true;
                        if let Err(error) = subscriber.drain().await {
                            tracing::warn!(%error, "the NATS link could not drain a subscription");
                            break;
                        }
                    }
                    Phase::Closed => break,
                }
            }
            message = subscriber.next() => match message {
                Some(message) => side.on_request(message, &deliveries).await,
                None => break,
            },
        }
    }
}

/// Runs until the link closes, or once draining until no call is held, since a held call may
/// still receive its items or a `cancel`; the inbound stream ends when this and every request
/// lane have.
async fn control_lane(side: Arc<ServerSide>, mut subscriber: Subscriber, deliveries: mpsc::UnboundedSender<Delivery>) {
    let mut phase = side.phase.subscribe();
    let mut count = side.calls.count.subscribe();
    loop {
        let draining = *phase.borrow() == Phase::Draining;
        tokio::select! {
            changed = phase.changed() => {
                if changed.is_err() || *phase.borrow() == Phase::Closed {
                    break;
                }
            }
            () = async { let _ = count.wait_for(|held| *held == 0).await; }, if draining => break,
            message = subscriber.next() => match message {
                Some(message) => side.on_control(message, &deliveries),
                None => break,
            },
        }
    }
}

/// The client side: one inbox subscription for every reply, each call's reply subject
/// `<prefix>.<id>`.
struct ClientSide {
    client: Client,
    codec: Codec,
    /// The link's runtime, which a streamed request's pump is spawned on.
    runtime: Tokio,
    prefix: String,
    max_payload: Arc<AtomicU64>,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// The load whose file the connection was handed, held while the connection may reconnect,
    /// since each reconnect reads the file again.
    _roots: Option<Arc<Load>>,
    #[cfg(feature = "conformance-probe")]
    probe: Arc<ClientProbe>,
}

enum ClientCall {
    Unary { pattern: Pattern },
    /// A streamed request: its `in`, `in_end` and `cancel` frames wait in `queue` until the server
    /// acknowledges the `open`, then go out in order.
    /// `until` is the deadline its `open` carried.
    Streaming { pattern: Pattern, gate: Option<oneshot::Sender<()>>, queue: mpsc::UnboundedSender<Frame>, until: HoldUntil },
    /// A streamed request cancelled before the server acknowledged its `open`: its `cancel` waits
    /// in the pump's queue and goes out alone once the acknowledgment opens `gate`, the frames
    /// queued before it dropped. The entry stays until then, or until the hold runs out, the
    /// call's deadline or `CANCEL_HOLD` after the `cancel` for an `open` carrying none, when
    /// dropping it closes `gate` and the pump ends.
    Cancelled { pattern: Pattern, gate: oneshot::Sender<()> },
}

impl ClientCall {
    fn pattern(&self) -> &Pattern {
        match self {
            ClientCall::Unary { pattern } | ClientCall::Streaming { pattern, .. } | ClientCall::Cancelled { pattern, .. } => pattern,
        }
    }
}

impl ClientSide {
    async fn send(self: &Arc<Self>, pattern: Pattern, frame: Frame) -> Result<(), BoxError> {
        match frame {
            Frame::Req { id, headers, data, .. } => {
                self.fits(data.len())?;
                lock(&self.calls).insert(id, ClientCall::Unary { pattern: pattern.clone() });
                let sent = publish(&self.client, pattern.to_string(), Some(self.reply_subject(id)), &headers, None, data.into_bytes()).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::Evt { headers, data, .. } => {
                self.fits(data.len())?;
                publish(&self.client, pattern.to_string(), None, &headers, None, data.into_bytes()).await
            }
            Frame::Open { id, headers, .. } => {
                let (gate, opened) = oneshot::channel();
                let (queue, queued) = mpsc::unbounded_channel();
                lock(&self.calls).insert(id, ClientCall::Streaming { pattern: pattern.clone(), gate: Some(gate), queue, until: HoldUntil::at_open(&headers) });
                self.runtime.handle().spawn(pump(self.client.clone(), self.reply_subject(id), opened, queued));
                let sent = publish(&self.client, pattern.to_string(), Some(self.reply_subject(id)), &headers, Some(OPEN), Bytes::new()).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::In { id, data } => {
                self.fits(data.len())?;
                self.enqueue(id, Frame::In { id, data });
                Ok(())
            }
            Frame::InEnd { id } => {
                self.enqueue(id, Frame::InEnd { id });
                Ok(())
            }
            Frame::Cancel { id } => {
                let unary = {
                    let mut calls = lock(&self.calls);
                    match calls.remove(&id) {
                        Some(ClientCall::Unary { .. }) => true,
                        // The server takes the `open` and holds the call, so the `cancel` follows
                        // once it acknowledges it.
                        Some(ClientCall::Streaming { pattern, gate: Some(gate), queue, until }) => {
                            let _ = queue.send(Frame::Cancel { id });
                            calls.insert(id, ClientCall::Cancelled { pattern, gate });
                            self.expire_cancelled(id, until);
                            false
                        }
                        Some(ClientCall::Streaming { gate: None, queue, .. }) => {
                            let _ = queue.send(Frame::Cancel { id });
                            false
                        }
                        Some(cancelled @ ClientCall::Cancelled { .. }) => {
                            calls.insert(id, cancelled);
                            false
                        }
                        None => false,
                    }
                };
                if unary { publish_control(&self.client, self.reply_subject(id), CANCEL, Bytes::new()).await } else { Ok(()) }
            }
            other => Err(format!("a client does not send a `{}` frame", other.kind()).into()),
        }
    }

    /// Drops call `id`'s held `cancel` once the hold runs out, if no `opened` released it first.
    fn expire_cancelled(self: &Arc<Self>, id: u64, until: HoldUntil) {
        let side = Arc::downgrade(self);
        let hold = until.remaining(self.fallback_hold());
        self.runtime.handle().spawn(async move {
            tokio::time::sleep(hold).await;
            if let Some(side) = side.upgrade() {
                let mut calls = lock(&side.calls);
                if matches!(calls.get(&id), Some(ClientCall::Cancelled { .. })) {
                    calls.remove(&id);
                }
            }
        });
    }

    fn reply_subject(&self, id: u64) -> String {
        format!("{}.{id}", self.prefix)
    }

    fn id_of(&self, subject: &str) -> Option<u64> {
        subject.strip_prefix(self.prefix.as_str())?.strip_prefix('.')?.parse().ok()
    }

    fn fits(&self, size: usize) -> Result<(), BoxError> {
        let limit = self.max_payload.load(Ordering::Relaxed);
        if limit > 0 && size as u64 > limit {
            return Err(Box::new(FrameTooLarge { size: size as u64, limit }));
        }
        Ok(())
    }

    fn enqueue(&self, id: u64, frame: Frame) {
        if let Some(ClientCall::Streaming { queue, .. }) = lock(&self.calls).get(&id) {
            let _ = queue.send(frame);
        }
    }

    /// How long a held `cancel` whose `open` carried no deadline is kept.
    fn fallback_hold(&self) -> Duration {
        #[cfg(feature = "conformance-probe")]
        return self.probe.hold();
        #[cfg(not(feature = "conformance-probe"))]
        ulo_rpc::__private::CANCEL_HOLD
    }

    fn open_gate(&self, id: u64) {
        let mut calls = lock(&self.calls);
        match calls.get_mut(&id) {
            Some(ClientCall::Streaming { gate, .. }) => {
                if let Some(gate) = gate.take() {
                    let _ = gate.send(());
                }
            }
            Some(ClientCall::Cancelled { .. }) => {
                if let Some(ClientCall::Cancelled { gate, .. }) = calls.remove(&id) {
                    let _ = gate.send(());
                }
            }
            _ => {}
        }
    }

    fn take(&self, id: u64) -> Option<ClientCall> {
        lock(&self.calls).remove(&id)
    }
}

/// Routes the inbox's messages until the connection is lost or the inbox subscription ends;
/// either ends the reply lane.
async fn route_replies(
    side: Arc<ClientSide>,
    mut replies: Subscriber,
    frames: mpsc::UnboundedSender<Frame>,
    mut disconnected: watch::Receiver<bool>,
) {
    loop {
        let message = tokio::select! {
            _ = disconnected.wait_for(|lost| *lost) => break,
            message = replies.next() => match message {
                Some(message) => message,
                None => break,
            },
        };
        let Some(id) = side.id_of(message.subject.as_str()) else { continue };
        // No-responders arrives on the reply subject after the publish returned, so the miss is
        // reported as the reply rather than as the `send` failure.
        if message.status == Some(StatusCode::NO_RESPONDERS) {
            if let Some(call) = side.take(id)
                && frames.send(no_destination(id, call.pattern())).is_err()
            {
                break;
            }
            continue;
        }
        if kind_of(message.headers.as_ref()).as_deref() == Some(OPENED) {
            #[cfg(feature = "conformance-probe")]
            if side.probe.withholds(id) {
                continue;
            }
            side.open_gate(id);
            continue;
        }
        let frame = match side.codec.decode_frame(&message.payload) {
            Ok(frame) => with_id(frame, id),
            Err(error) => {
                tracing::warn!(%error, "the NATS link dropped a reply that does not decode");
                continue;
            }
        };
        if is_terminal(&frame) {
            side.take(id);
        }
        if frames.send(frame).is_err() {
            break;
        }
    }
}

/// Publishes one streamed request's control frames once the server has acknowledged its `open`.
async fn pump(client: Client, reply: String, opened: oneshot::Receiver<()>, mut queued: mpsc::UnboundedReceiver<Frame>) {
    if opened.await.is_err() {
        return;
    }
    // A `cancel` queued before the acknowledgment goes out alone: the call is over.
    let mut held = Vec::new();
    while let Ok(frame) = queued.try_recv() {
        held.push(frame);
    }
    if let Some(at) = held.iter().position(|frame| matches!(frame, Frame::Cancel { .. })) {
        held = vec![held.swap_remove(at)];
        queued.close();
    }
    let mut held = held.into_iter();
    loop {
        let frame = match held.next() {
            Some(frame) => frame,
            None => match queued.recv().await {
                Some(frame) => frame,
                None => break,
            },
        };
        let (kind, body) = match frame {
            Frame::In { data, .. } => (IN, data.into_bytes()),
            Frame::InEnd { .. } => (IN_END, Bytes::new()),
            Frame::Cancel { .. } => (CANCEL, Bytes::new()),
            _ => continue,
        };
        if let Err(error) = publish_control(&client, reply.clone(), kind, body).await {
            tracing::debug!(%error, kind, "the NATS link could not publish a control frame");
        }
    }
}

async fn publish(
    client: &Client,
    subject: String,
    reply: Option<String>,
    headers: &CallHeaders,
    kind: Option<&str>,
    body: Bytes,
) -> Result<(), BoxError> {
    let map = nats_headers(headers, kind);
    match (reply, map) {
        (None, None) => client.publish(subject, body).await?,
        (None, Some(map)) => client.publish_with_headers(subject, map, body).await?,
        (Some(reply), None) => client.publish_with_reply(subject, reply, body).await?,
        (Some(reply), Some(map)) => client.publish_with_reply_and_headers(subject, reply, map, body).await?,
    }
    Ok(())
}

/// A control-lane message: the call's reply subject is its key on every server instance.
async fn publish_control(client: &Client, reply: String, kind: &str, body: Bytes) -> Result<(), BoxError> {
    publish(client, CONTROL.to_owned(), Some(reply), &CallHeaders::new(), Some(kind), body).await
}

fn nats_headers(headers: &CallHeaders, kind: Option<&str>) -> Option<HeaderMap> {
    if headers.is_empty() && kind.is_none() {
        return None;
    }
    let mut map = HeaderMap::new();
    for (name, value) in headers.iter() {
        map.append(name, value);
    }
    if let Some(kind) = kind {
        map.insert(KIND, kind);
    }
    Some(map)
}

fn kind_of(headers: Option<&HeaderMap>) -> Option<String> {
    headers?.get(KIND).map(|value| value.as_str().to_owned())
}

fn call_headers(headers: Option<&HeaderMap>) -> CallHeaders {
    let mut out = CallHeaders::new();
    if let Some(headers) = headers {
        for (name, values) in headers.iter() {
            let name: &str = name.as_ref();
            if name == KIND {
                continue;
            }
            for value in values {
                out.insert(name, value.as_str());
            }
        }
    }
    out
}

/// What a caller receives for a miss this link signals: `Unavailable` with
/// `reason: "no_destination"`, as `RpcClient` maps a `NoDestination` from `send`.
fn no_destination(id: u64, pattern: &Pattern) -> Frame {
    let mut details = Details::new();
    details.push(Detail::ErrorInfo { reason: "no_destination".to_owned(), domain: DOMAIN.to_owned(), metadata: BTreeMap::new() });
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, format!("nothing listens on pattern `{pattern}`"), details) }
}

/// The reply frame with the caller's own `id`, read from the reply subject: the server wrote its
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

fn servers(url: &str) -> Result<Vec<ServerAddr>, BoxError> {
    let servers = url
        .split(',')
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(|server| server.parse::<ServerAddr>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("the NATS link's url does not parse: {error}"))?;
    if servers.is_empty() {
        return Err("the NATS link's url names no server".into());
    }
    Ok(servers)
}

/// A connection opened, and its task started, on the runtime polling this: `Tokio::run` for the
/// link. Given `roots`, async-nats trusts the certificates in that file alone and skips its own
/// read of the platform's store, which it makes on the thread polling every TLS connection and
/// reconnect otherwise; the file it reads on tokio's blocking pool.
async fn open(servers: Vec<ServerAddr>, options: ConnectOptions, roots: Option<PathBuf>) -> Result<Client, BoxError> {
    let options = match roots {
        Some(path) => options.add_root_certificates(path),
        None => options,
    };
    Ok(options.connect(servers).await?)
}

/// A link's root certificates: the process's load, or one of the link's own where a test sets a
/// loader. The platform's store takes 71 ms warm and up to 3 s cold to read on macOS, and thirty
/// reads at once up to 2.55 s each, so the process reads it once while any link holds the load,
/// on the blocking pool of the link that started it. async-nats, left to itself, reads the store
/// on the thread polling each TLS connection and reconnect, stalling every task scheduled there.
/// For a `tls://` url the link first resolves rustls's crypto provider, as async-nats's TLS
/// configuration will at the first connection (see [`resolve_provider`]), and keeps the outcome.
/// A load that fails is kept while it is held, since no retry mends a store that cannot be read or
/// holds no root, or a directory that cannot be written: over `tls://` a server's `prepare`
/// refuses it, and every client call answers it as [`Misconfigured`]. A wait for the load is
/// bounded by the link's `roots_timeout`; the load itself is not, and a wait that gives up leaves
/// it running for the next.
pub(crate) struct Roots {
    /// The loader a test sets, which gives the link a load of its own in place of the process's.
    pub(crate) own: Option<Loader>,
    /// [`resolve_provider`], which a test replaces.
    pub(crate) resolve: fn() -> Result<(), String>,
    /// The provider's resolution, made at a `tls://` url's first need.
    resolved: OnceLock<Result<(), String>>,
    /// The load the link holds, from its first need until it drops.
    held: Mutex<Option<Arc<Load>>>,
}

/// Where a load reads the roots and writes their file.
#[derive(Clone, Copy)]
pub(crate) struct Loader {
    /// `rustls_native_certs::load_native_certs`, which a test replaces.
    pub(crate) load: fn() -> CertificateResult,
    /// The directory the file is written in, `std::env::temp_dir`, which a test replaces.
    pub(crate) dir: fn() -> PathBuf,
}

impl Loader {
    pub(crate) const PLATFORM: Loader = Loader { load: rustls_native_certs::load_native_certs, dir: std::env::temp_dir };
}

/// The process's load, held weakly: every link that needs roots while another holds the load
/// joins it, and once the last holder drops it the next link starts a new one. A strong slot would
/// keep the file until the process exits.
static PROCESS: Mutex<Weak<Load>> = Mutex::new(Weak::new());

/// The loader the process's load uses, which a test replaces.
#[cfg(test)]
pub(crate) static PROCESS_LOADER: Mutex<Loader> = Mutex::new(Loader::PLATFORM);

fn process_loader() -> Loader {
    #[cfg(test)]
    return *lock(&PROCESS_LOADER);
    #[cfg(not(test))]
    Loader::PLATFORM
}

impl Roots {
    fn platform() -> Roots {
        Roots { own: None, resolve: resolve_provider, resolved: OnceLock::new(), held: Mutex::new(None) }
    }

    /// The load this link holds, joined or started at its first need. A load whose task never ran,
    /// its runtime having shut down first, is replaced, so the next need, on a runtime still
    /// running, loads them again.
    fn held(&self, runtime: &Tokio) -> Arc<Load> {
        let mut held = lock(&self.held);
        if let Some(load) = held.as_ref().filter(|load| !load.abandoned()) {
            return Arc::clone(load);
        }
        let load = match self.own {
            Some(loader) => Load::start(runtime, loader),
            None => Load::shared(runtime),
        };
        *held = Some(Arc::clone(&load));
        load
    }

    /// The load once its file is written, waited for under `bound`. `tls` is whether the link's
    /// url names a `tls://` server, constant for a link: it resolves the provider first and names
    /// `tls://` in a refusal. A load outlasting the bound is refused as a failed one is; it keeps
    /// running, and a later wait joins it.
    async fn wait(&self, runtime: &Tokio, tls: bool, bound: Bound) -> Result<Arc<Load>, BoxError> {
        if tls && let Err(refusal) = self.resolved.get_or_init(self.resolve) {
            return Err(Misconfigured { source: refusal.clone().into() }.into());
        }
        let load = self.held(runtime);
        match within(runtime, bound, load.wait()).await {
            Ok(Some(Ok(_))) => Ok(load),
            Ok(Some(Err(cause))) => Err(Misconfigured { source: format!("{}: {cause}", unloaded(tls)).into() }.into()),
            Ok(None) => Err(STOPPED.into()),
            Err(after) => Err(Misconfigured {
                source: format!(
                    "{}: the load did not end within `roots_timeout`'s {after:?}; raise it with `.roots_timeout(..)`, or write `Bound::Unbounded` to wait for as long as the load takes",
                    unloaded(tls)
                )
                .into(),
            }
            .into()),
        }
    }

    /// The load once its file is written, waited for under `bound`, for a `nats://` server's
    /// `listen`: `None` for a load that failed, logged once per load, or that outlasted the bound
    /// or ended unanswered, each logged here. A load ends unanswered when the runtime it ran on
    /// shut down first, which may be another link's when this one joined the process's load.
    async fn awaited(&self, runtime: &Tokio, bound: Bound) -> Option<Arc<Load>> {
        let load = self.held(runtime);
        match within(runtime, bound, load.wait()).await {
            Ok(Some(Ok(_))) => Some(load),
            Ok(Some(Err(cause))) => {
                load.warn(&cause);
                None
            }
            Ok(None) => {
                tracing::warn!(
                    "the NATS link's root certificates load ended without an answer, the runtime it ran on having shut down, so its server connects to its `nats://` url without them; if the server there upgrades the connection to TLS, async-nats reads the platform's store on the thread polling the connection, at that connect and at each of its reconnects"
                );
                None
            }
            Err(after) => {
                tracing::warn!(
                    timeout = ?after,
                    "the NATS link's root certificates did not load within `roots_timeout`, so its server connects to its `nats://` url without them; if the server there upgrades the connection to TLS, async-nats reads the platform's store on the thread polling the connection, at that connect and at each of its reconnects"
                );
                None
            }
        }
    }

    /// The load if its file is written, or `None` at once, for a url naming no `tls://` server. A
    /// failed load is logged at `warn` once, when this finds it failed or, if it still runs, when
    /// it ends.
    fn ready(&self, runtime: &Tokio) -> Option<Arc<Load>> {
        let load = self.held(runtime);
        let failed = load.answer.borrow().as_ref().map(|written| written.as_ref().err().cloned());
        match failed {
            Some(None) => Some(load),
            Some(Some(cause)) => {
                load.warn(&cause);
                None
            }
            None => {
                load.warn_when_failed(runtime);
                None
            }
        }
    }

    /// Where the link's roots were written, once they have been.
    #[cfg(test)]
    pub(crate) fn path(&self) -> Option<PathBuf> {
        lock(&self.held).as_ref()?.path()
    }

    /// The link's load once it has answered: its file, or `None` for a failed load or one the link
    /// never started.
    #[cfg(test)]
    pub(crate) async fn settled(&self) -> Option<PathBuf> {
        let load = lock(&self.held).clone()?;
        load.wait().await?.ok()
    }
}

/// One load of the roots, a task on a runtime's blocking pool whose answer every holder shares. A
/// caller that stops waiting drops its own wait and nothing else, so the load is never restarted
/// while anything holds it. The file goes once the last holder and the task have dropped it.
pub(crate) struct Load {
    /// `None` until the task answers: the file, or the whole of the refusal's cause.
    answer: watch::Receiver<Option<Result<RootsFile, String>>>,
    /// A task waits to log the load's failure for a `nats://` link.
    watched: AtomicBool,
    /// The failure has been logged for a `nats://` link.
    warned: AtomicBool,
}

impl Load {
    /// The process's load, joined while another holder keeps it, started otherwise.
    fn shared(runtime: &Tokio) -> Arc<Load> {
        let mut slot = lock(&PROCESS);
        if let Some(load) = slot.upgrade().filter(|load| !load.abandoned()) {
            return load;
        }
        let load = Load::start(runtime, process_loader());
        *slot = Arc::downgrade(&load);
        load
    }

    /// A load spawned on `runtime`'s blocking pool. A panic in it is kept as a refusal naming the
    /// panic; a task the runtime drops unrun leaves the load abandoned.
    fn start(runtime: &Tokio, loader: Loader) -> Arc<Load> {
        let (answering, answer) = watch::channel(None);
        drop(runtime.handle().spawn_blocking(move || {
            let written = std::panic::catch_unwind(AssertUnwindSafe(|| RootsFile::write((loader.load)(), &(loader.dir)())))
                .unwrap_or_else(|panic| Err(format!("loading them panicked: {}", panic_text(&*panic))));
            // Refused once every holder has dropped the load, which drops the file here.
            let _ = answering.send(Some(written));
        }));
        Arc::new(Load { answer, watched: AtomicBool::new(false), warned: AtomicBool::new(false) })
    }

    fn path(&self) -> Option<PathBuf> {
        self.answer.borrow().as_ref()?.as_ref().ok().map(|file| file.path.clone())
    }

    /// The task ended without answering. Checked closed first: once closed the answer cannot
    /// change.
    fn abandoned(&self) -> bool {
        self.answer.has_changed().is_err() && self.answer.borrow().is_none()
    }

    /// The task's answer, or `None` if it ended without one.
    async fn wait(&self) -> Option<Result<PathBuf, String>> {
        let mut answer = self.answer.clone();
        let answered = answer.wait_for(Option::is_some).await.ok()?;
        answered.as_ref().map(|written| written.as_ref().map(|file| file.path.clone()).map_err(Clone::clone))
    }

    fn warn(&self, cause: &str) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                cause = %format!("{}: {cause}", unloaded(false)),
                "the NATS link connects to its `nats://` url without root certificates of its own; a server there that requires TLS fails the handshake or reads the platform's store on the thread polling the connection"
            );
        }
    }

    /// Logs the failure once the load ends, if it fails, from a task on `runtime` that holds the
    /// load until then.
    fn warn_when_failed(self: &Arc<Self>, runtime: &Tokio) {
        if self.watched.swap(true, Ordering::Relaxed) {
            return;
        }
        let load = Arc::clone(self);
        drop(runtime.handle().spawn(async move {
            if let Some(Err(cause)) = load.wait().await {
                load.warn(&cause);
            }
        }));
    }
}

/// The scheme is named only for a link that requires TLS: a `nats://` link's text becomes the
/// cause of its `warn`.
fn unloaded(tls: bool) -> &'static str {
    if tls {
        "the NATS link could not load the platform's root certificates for `tls://`"
    } else {
        "the NATS link could not load the platform's root certificates"
    }
}

const STOPPED: &str = "the NATS link's runtime shut down while it loaded the platform's root certificates";

/// `fut`'s output, or `Err` with the bound once `bound` passes first: [`ROOTS_TIMEOUT`] at
/// `Bound::Default`, none at `Bound::Unbounded`. The sleep is made against the link's runtime, so
/// the wait is timed wherever it is polled. `fut` is polled first, so an answer already in is
/// taken however short the bound.
async fn within<F: Future>(runtime: &Tokio, bound: Bound, fut: F) -> Result<F::Output, Duration> {
    let after = match bound {
        Bound::Default => ROOTS_TIMEOUT,
        Bound::After(after) => after,
        Bound::Unbounded => return Ok(fut.await),
    };
    match select(pin!(fut), ulo::Timer::sleep(runtime, after)).await {
        Either::Left((output, _)) => Ok(output),
        Either::Right(((), _)) => Err(after),
    }
}

/// The refusal of a process whose build links more than one of rustls's crypto providers and
/// installs none.
const PROVIDERS: &str =
    "this process links more than one rustls crypto provider and installs none; install one with `CryptoProvider::install_default` before the app starts";

/// rustls's own resolution of the process's crypto provider, the call async-nats's TLS
/// configuration makes through `ClientConfig::builder()` on each TLS connection: the provider the
/// process installed, or the one rustls's features name, which rustls installs as the process's
/// default, as async-nats's first TLS connection would. Where the features name two and none is
/// installed, rustls panics, which here is caught and refused rather than left to panic a
/// connection. The link chooses no provider: the process's default is the application's. rustls
/// panics before it installs anything, and the call holds no lock or shared state, so catching the
/// panic leaves nothing half-done; the panic hook still prints rustls's message. A `nats://` url
/// is not resolved for: rustls would install a default for a link that may never use TLS.
fn resolve_provider() -> Result<(), String> {
    std::panic::catch_unwind(|| drop(ClientConfig::builder())).map_err(|_| PROVIDERS.to_owned())
}

/// A file of root certificates a load wrote, removed when the load drops it: once every link and
/// connection holding the load has. A process killed before the drop leaves it in the temp
/// directory.
pub(crate) struct RootsFile {
    path: PathBuf,
}

/// Names the files one process writes apart.
static WRITTEN: AtomicU64 = AtomicU64::new(0);

impl RootsFile {
    /// The roots `loaded` yielded that async-nats accepts, written as PEM to a file of their own in
    /// `dir`, refused when it yielded none: a store that cannot be read and one that holds none
    /// would each fail every handshake. async-nats fails a connection on any certificate in the
    /// file that is not a trust anchor, so one is written only once a root store has taken it; a
    /// load yielding roots beside errors keeps the roots and logs the errors at `warn`. The file
    /// is created anew, never opened where one exists.
    fn write(loaded: CertificateResult, dir: &Path) -> Result<RootsFile, String> {
        let mut store = RootCertStore::empty();
        let mut pem = String::new();
        let mut unparsable = 0usize;
        for cert in loaded.certs {
            if store.add(cert.clone()).is_ok() {
                pem.push_str("-----BEGIN CERTIFICATE-----\n");
                let encoded = BASE64.encode(cert.as_ref());
                for line in encoded.as_bytes().chunks(64) {
                    pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
                    pem.push('\n');
                }
                pem.push_str("-----END CERTIFICATE-----\n");
            } else {
                unparsable += 1;
            }
        }
        let added = store.len();
        let errors: Vec<String> = loaded.errors.iter().map(ToString::to_string).collect();
        if added == 0 {
            return Err(match (errors.as_slice(), unparsable) {
                ([], 0) => "the platform's store holds none".to_owned(),
                ([], unparsable) => format!("none of the {unparsable} the platform's store holds parses"),
                (errors, _) => errors.join("; "),
            });
        }
        if !errors.is_empty() || unparsable > 0 {
            tracing::warn!(
                added,
                unparsable,
                errors = %errors.join("; "),
                "the NATS link loaded some of the platform's root certificates and could not load others"
            );
        }
        let (mut file, path) = loop {
            let path = dir.join(format!("ulo-rpc-nats-roots-{}-{}.pem", std::process::id(), WRITTEN.fetch_add(1, Ordering::Relaxed)));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break (file, path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("could not create a file for them in `{}`: {error}", dir.display())),
            }
        };
        // Removed by its drop if the write fails.
        let written = RootsFile { path };
        if let Err(error) = file.write_all(pem.as_bytes()).and_then(|()| file.flush()) {
            return Err(format!("could not write them to `{}`: {error}", written.path.display()));
        }
        Ok(written)
    }
}

impl Drop for RootsFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::debug!(%error, path = %self.path.display(), "the NATS link could not remove its file of root certificates");
        }
    }
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "a payload that is not text".to_owned())
}

/// A queue group name from the root module's path: NATS refuses whitespace and the wildcards in
/// one.
fn queue_group(text: &str) -> String {
    text.chars().map(|c| if c.is_whitespace() || c == '*' || c == '>' { '_' } else { c }).collect()
}

fn check_group(group: &str) -> Result<(), BoxError> {
    if group.is_empty() || group.chars().any(|c| c.is_whitespace() || c == '*' || c == '>') {
        return Err(format!(
            "the NATS link's group `{group}` is not a queue group name: it must be non-empty, with no whitespace, `*` or `>`"
        )
        .into());
    }
    Ok(())
}

fn receiver_stream<T: Send + 'static>(receiver: mpsc::UnboundedReceiver<T>) -> futures_core::stream::BoxStream<'static, T> {
    futures_util::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) }).boxed()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::Condvar;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use async_nats::rustls::pki_types::CertificateDer;
    use async_nats::rustls::pki_types::pem::PemObject;
    use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError};
    use ulo_rpc::{RpcClient, Server};

    use super::*;

    /// Root certificates loaded per test, each test's loader counting in a slot of its own.
    static LOADS: [AtomicUsize; 29] = [const { AtomicUsize::new(0) }; 29];

    /// A store that cannot be read: rustls-native-certs reading a file that is not there, as it
    /// reads one `SSL_CERT_FILE` names.
    fn unreadable<const SLOT: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        rustls_native_certs::load_certs_from_paths(Some(Path::new("/nonexistent/ulo-rpc-nats-roots.pem")), None)
    }

    /// A store holding no root, read without an error.
    fn empty<const SLOT: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        CertificateResult::default()
    }

    /// Providers resolved per test, each test's resolver counting in a slot of its own.
    static RESOLVES: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

    /// rustls's resolution as a build linking two providers and installing none answers it.
    fn two_providers<const SLOT: usize>() -> Result<(), String> {
        RESOLVES[SLOT].fetch_add(1, Ordering::SeqCst);
        Err(PROVIDERS.to_owned())
    }

    /// A store holding the test certificate alone.
    fn test_root<const SLOT: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        let mut loaded = CertificateResult::default();
        loaded.certs.push(ulo_test_certs::localhost().cert_der().clone());
        loaded
    }

    /// The test certificate after 300 ms, as a store slow to read yields it.
    fn slow_root<const SLOT: usize>() -> CertificateResult {
        std::thread::sleep(Duration::from_millis(300));
        test_root::<SLOT>()
    }

    /// How long a gated loader blocks unless its test opens the gate first: twice the RPC
    /// conformance suite's boot budget.
    const HELD: Duration = Duration::from_secs(10);

    /// A loader's wait, which its test opens.
    struct Gate {
        open: Mutex<bool>,
        opened: Condvar,
    }

    impl Gate {
        const fn new() -> Gate {
            Gate { open: Mutex::new(false), opened: Condvar::new() }
        }

        fn open(&self) {
            *lock(&self.open) = true;
            self.opened.notify_all();
        }

        /// Blocks until the gate opens or `HELD` passes.
        fn wait(&self) {
            let deadline = Instant::now() + HELD;
            let mut open = lock(&self.open);
            while !*open {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                open = self.opened.wait_timeout(open, left).unwrap_or_else(PoisonError::into_inner).0;
            }
        }
    }

    static GATES: [Gate; 5] = [const { Gate::new() }; 5];

    /// The test certificate once gate `GATE` opens or `HELD` passes, counted in `SLOT`.
    fn gated<const SLOT: usize, const GATE: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        GATES[GATE].wait();
        let mut loaded = CertificateResult::default();
        loaded.certs.push(ulo_test_certs::localhost().cert_der().clone());
        loaded
    }

    /// A store that cannot be read, once gate `GATE` opens or `HELD` passes, counted in `SLOT`.
    #[cfg(feature = "integration")]
    fn gated_unreadable<const SLOT: usize, const GATE: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        GATES[GATE].wait();
        rustls_native_certs::load_certs_from_paths(Some(Path::new("/nonexistent/ulo-rpc-nats-roots.pem")), None)
    }

    /// A directory of the test's own, made empty, which its links write their roots in.
    fn dir<const SLOT: usize>() -> PathBuf {
        std::env::temp_dir().join(format!("ulo-rpc-nats-roots-test-{}-{SLOT}", std::process::id()))
    }

    fn fresh(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).expect("the test's directory is created");
    }

    fn files_in(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir).expect("the test's directory reads").map(|entry| entry.expect("an entry reads").path()).collect()
    }

    /// What `dir` holds once it is empty, or after five seconds: a dropped client closes its link
    /// from a task, which holds the link until the close ends.
    async fn emptied(dir: &Path) -> Vec<PathBuf> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let files = files_in(dir);
            if files.is_empty() || std::time::Instant::now() > deadline {
                return files;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// What `dir` holds once it holds anything, or after five seconds: a `nats://` client's load
    /// is a task its connect does not wait for.
    async fn written(dir: &Path) -> Vec<PathBuf> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let files = files_in(dir);
            if !files.is_empty() || Instant::now() > deadline {
                return files;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// `count` once it reaches `at_least`, or after five seconds.
    async fn counted(count: &AtomicUsize, at_least: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let now = count.load(Ordering::SeqCst);
            if now >= at_least || Instant::now() > deadline {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The process's load is one per test binary, so the tests that use it take turns.
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// A test's turn with the process's load, its loader set to the test's until the turn ends. A
    /// turn starts with no load held, an earlier turn's holders given five seconds to drop theirs.
    struct ProcessTurn {
        _turn: tokio::sync::MutexGuard<'static, ()>,
    }

    impl ProcessTurn {
        async fn take(loader: Loader) -> ProcessTurn {
            let turn = TURN.lock().await;
            let deadline = Instant::now() + Duration::from_secs(5);
            while lock(&PROCESS).strong_count() > 0 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            ProcessTurn::begin(turn, loader)
        }

        /// The turn for a test that runs outside a runtime.
        #[cfg(feature = "integration")]
        fn blocking(loader: Loader) -> ProcessTurn {
            let turn = TURN.blocking_lock();
            let deadline = Instant::now() + Duration::from_secs(5);
            while lock(&PROCESS).strong_count() > 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            ProcessTurn::begin(turn, loader)
        }

        fn begin(turn: tokio::sync::MutexGuard<'static, ()>, loader: Loader) -> ProcessTurn {
            assert_eq!(lock(&PROCESS).strong_count(), 0, "an earlier test's process load is still held");
            *lock(&PROCESS_LOADER) = loader;
            ProcessTurn { _turn: turn }
        }
    }

    impl Drop for ProcessTurn {
        fn drop(&mut self) {
            *lock(&PROCESS_LOADER) = Loader::PLATFORM;
        }
    }

    /// `ring` installed as the process's crypto provider. The crate's tests link `aws-lc-rs` beside
    /// it, through the dev-dependency on rustls, and so does a workspace build through the MQTT
    /// link, so without an installed provider every `tls://` link here would be refused as
    /// `tests/two_providers.rs` shows it is. Installing twice keeps the first.
    fn installed() {
        let _ = async_nats::rustls::crypto::ring::default_provider().install_default();
    }

    /// A link on `url` with a load of its own in place of the process's, its roots from `load`
    /// written in `dir`. Nothing listens at
    /// the urls the tests use: a link that reached a socket would fail `Unavailable`, not with the
    /// load's refusal.
    fn link(url: &str, load: fn() -> CertificateResult, dir: fn() -> PathBuf) -> Nats {
        installed();
        let mut link = Nats::url(url);
        link.roots.own = Some(Loader { load, dir });
        link
    }

    struct Root;

    impl Module for Root {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, _m: &mut ModuleDef<'_>) {}
    }

    async fn app() -> App<ulo::app::Connected> {
        App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects")
    }

    async fn refused_at_bind(link: Nats) -> String {
        match app().await.bind(Server::new(link)).listen().await {
            Err(error @ StartupError::Configure(_)) => error.to_string(),
            Err(other) => panic!("the bind failed other than as a `Configure` refusal: {other:?}"),
            Ok(_) => panic!("a server whose root certificates did not load listened"),
        }
    }

    async fn failed_calls(client: &RpcClient) -> Vec<(ErrorKind, Option<String>, String)> {
        let mut failed = Vec::new();
        for _ in 0..2 {
            let call = client.request::<_, u32>("nats.roots", &()).await.expect_err("a call to a server that is not there");
            failed.push((call.kind(), call.reason().map(str::to_owned), call.message().to_owned()));
        }
        failed
    }

    async fn refused_calls(link: Nats) -> Vec<(ErrorKind, Option<String>, String)> {
        failed_calls(&RpcClient::new(link, Arc::new(Tokio::current())).expect("the link is usable")).await
    }

    /// F417: a store that cannot be read refuses a server's bind in `prepare`, naming the store and
    /// the cause, and the store is read once.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_refuses_the_server_s_bind() {
        let refused = refused_at_bind(link("tls://localhost:1", unreadable::<0>, std::env::temp_dir)).await;
        assert!(
            refused.contains("could not load the platform's root certificates") && refused.contains("/nonexistent/ulo-rpc-nats-roots.pem"),
            "the refusal does not name the store and its cause: {refused}"
        );
        assert_eq!(LOADS[0].load(Ordering::SeqCst), 1, "the server read the store other than once");
    }

    /// F417: a store that cannot be read fails every client call `Internal` with
    /// `reason: "misconfigured_link"`, naming the cause, where the link before the fix failed each
    /// `Unavailable`, and the store is read once for both calls.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_fails_every_call_misconfigured() {
        for (kind, reason, message) in refused_calls(link("tls://localhost:1", unreadable::<1>, std::env::temp_dir)).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains("/nonexistent/ulo-rpc-nats-roots.pem"), "the call does not name the cause: {message}");
        }
        assert_eq!(LOADS[1].load(Ordering::SeqCst), 1, "the client read the store other than once for two calls");
    }

    /// F417: a store holding no root is refused as one that cannot be read, on both sides, where
    /// the link before the fix connected with an empty trust store and failed every handshake
    /// `Unavailable`.
    #[tokio::test]
    async fn a_store_holding_no_root_is_refused_as_misconfigured() {
        let refused = refused_at_bind(link("tls://localhost:1", empty::<2>, std::env::temp_dir)).await;
        assert!(refused.contains("the platform's store holds none"), "the server's refusal does not say the store is empty: {refused}");
        for (kind, reason, message) in refused_calls(link("tls://localhost:1", empty::<3>, std::env::temp_dir)).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains("the platform's store holds none"), "the call does not say the store is empty: {message}");
        }
        assert_eq!((LOADS[2].load(Ordering::SeqCst), LOADS[3].load(Ordering::SeqCst)), (1, 1), "a side read the store other than once");
    }

    /// F417: a directory the roots cannot be written in is refused as a store that cannot be read
    /// is, on both sides, naming the directory.
    #[tokio::test]
    async fn a_file_that_cannot_be_written_is_refused_as_misconfigured() {
        fn missing() -> PathBuf {
            PathBuf::from("/nonexistent/ulo-rpc-nats-dir")
        }
        let refused = refused_at_bind(link("tls://localhost:1", test_root::<4>, missing)).await;
        assert!(refused.contains("could not create a file for them in `/nonexistent/ulo-rpc-nats-dir`"), "the server's refusal does not name the directory: {refused}");
        for (kind, reason, message) in refused_calls(link("tls://localhost:1", test_root::<5>, missing)).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains("/nonexistent/ulo-rpc-nats-dir"), "the call does not name the directory: {message}");
        }
        assert_eq!((LOADS[4].load(Ordering::SeqCst), LOADS[5].load(Ordering::SeqCst)), (1, 1), "a side loaded the roots other than once");
    }

    /// F417: a client writes its roots once, at its first connect, as the PEM async-nats reads, and
    /// the file goes when the client, and the link with it, drops. Each call here fails
    /// `Unavailable`, nothing listening, after the roots were written.
    #[tokio::test]
    async fn a_client_writes_its_roots_once_and_removes_them_when_it_drops() {
        let here = dir::<6>();
        fresh(&here);
        let client = RpcClient::new(link("tls://localhost:1", test_root::<6>, dir::<6>), Arc::new(Tokio::current())).expect("the link is usable");
        for (kind, _, message) in failed_calls(&client).await {
            assert_eq!(kind, ErrorKind::Unavailable, "{message}");
        }
        let written = files_in(&here);
        assert_eq!(written.len(), 1, "the client wrote other than one file: {written:?}");
        let pem = std::fs::read(&written[0]).expect("the roots read back");
        let read: Vec<_> = CertificateDer::pem_slice_iter(&pem).collect::<Result<_, _>>().expect("the file is PEM");
        assert_eq!(read, vec![ulo_test_certs::localhost().cert_der().clone()], "the file holds other than the loaded root");
        assert_eq!(LOADS[6].load(Ordering::SeqCst), 1, "the client loaded its roots other than once for two calls");
        drop(client);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the client");
        let _ = std::fs::remove_dir(&here);
    }

    /// F417: a server's link writes its roots in `prepare` and removes them when it drops, though
    /// it never connected.
    #[tokio::test]
    async fn a_server_s_link_dropped_before_it_listens_removes_its_roots() {
        let here = dir::<7>();
        fresh(&here);
        let app = app().await;
        let mut server = link("tls://localhost:1", test_root::<7>, dir::<7>);
        server.prepare(&app.handle()).await.expect("the server's link prepares");
        assert_eq!(files_in(&here).len(), 1, "`prepare` wrote other than one file");
        drop(server);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the link");
        let _ = std::fs::remove_dir(&here);
    }

    /// F417: two loads in one process write files of their own, and each file goes with its load,
    /// here two links' own loads. The process's load is one at a time, but a load whose holders
    /// have all dropped can still be writing its file when the next starts.
    #[tokio::test]
    async fn two_loads_write_files_of_their_own() {
        let here = dir::<8>();
        fresh(&here);
        let runtime = Tokio::current();
        let (first, second) = (link("tls://localhost:1", test_root::<8>, dir::<8>), link("tls://localhost:1", test_root::<8>, dir::<8>));
        let (a, b) = (first.roots.wait(&runtime, true, Bound::Default).await.expect("the first loads"), second.roots.wait(&runtime, true, Bound::Default).await.expect("the second loads"));
        assert_ne!(a.path(), b.path(), "two loads share one file");
        drop((a, b));
        assert_eq!(files_in(&here).len(), 2, "the two loads do not hold two files");
        drop(first);
        assert_eq!(files_in(&here), vec![second.roots.path().expect("the second wrote its roots")], "dropping one link removed other than its load's file");
        drop(second);
        assert_eq!(files_in(&here), Vec::<PathBuf>::new(), "the second link's roots outlived it");
        let _ = std::fs::remove_dir(&here);
    }

    /// F423: a `nats://` link loads and writes its roots once as a `tls://` one does, since its
    /// server may require TLS, though a client's connect does not wait for them, and removes them
    /// when it drops.
    #[tokio::test]
    async fn a_plain_link_writes_its_roots_too() {
        let here = dir::<9>();
        fresh(&here);
        let client = RpcClient::new(link("nats://localhost:1", test_root::<9>, dir::<9>), Arc::new(Tokio::current())).expect("the link is usable");
        for (kind, _, message) in failed_calls(&client).await {
            assert_eq!(kind, ErrorKind::Unavailable, "{message}");
        }
        assert_eq!(written(&here).await.len(), 1, "a plain link wrote other than one file");
        assert_eq!(LOADS[9].load(Ordering::SeqCst), 1, "a plain link loaded its roots other than once for two calls");
        drop(client);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the client");
        let _ = std::fs::remove_dir(&here);
    }

    /// F423: a `nats://` link whose roots did not load is not refused, a plain server needing
    /// none of them: a server's `prepare` passes, and each client call fails as the unreachable
    /// url fails it, `Unavailable`. Each side loads once.
    #[tokio::test]
    async fn a_plain_link_whose_roots_did_not_load_is_not_refused() {
        let app = app().await;
        let mut server = link("nats://localhost:1", unreadable::<10>, std::env::temp_dir);
        server.prepare(&app.handle()).await.expect("a plain server's link prepares though its roots did not load");
        assert_eq!(server.roots.settled().await, None, "a store that cannot be read yielded a file");
        for (kind, reason, message) in refused_calls(link("nats://localhost:1", empty::<11>, std::env::temp_dir)).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Unavailable, None), "{message}");
        }
        assert_eq!(
            (LOADS[10].load(Ordering::SeqCst), counted(&LOADS[11], 1).await),
            (1, 1),
            "a side loaded its roots other than once"
        );
    }

    /// F423: the refusal kept for a `nats://` link becomes the cause of its `warn`, so it does
    /// not name `tls://`; a `tls://` link's refusal does.
    #[tokio::test]
    async fn a_plain_link_s_logged_cause_does_not_name_tls() {
        fn missing() -> CertificateResult {
            rustls_native_certs::load_certs_from_paths(Some(Path::new("/nonexistent/ulo-rpc-nats-roots.pem")), None)
        }
        let runtime = Tokio::current();
        let plain = link("nats://localhost:1", missing, std::env::temp_dir).roots.wait(&runtime, false, Bound::Default).await.err().expect("the roots did not load");
        assert!(!plain.to_string().contains("tls://"), "{plain}");
        let tls = link("tls://localhost:1", missing, std::env::temp_dir).roots.wait(&runtime, true, Bound::Default).await.err().expect("the roots did not load");
        assert!(tls.to_string().contains("`tls://`"), "{tls}");
    }

    /// F423: a `nats://` link to a plaintext NATS server stays plain though it hands every
    /// connection its roots, async-nats reading the file only for a connection that uses TLS.
    #[cfg(feature = "integration")]
    mod plain {
        use std::net::{Ipv4Addr, SocketAddr};

        use futures_util::StreamExt;
        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{ContainerAsync, GenericImage, ImageExt};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use ulo_rpc_conformance::relay::{reachable, unshadowed};
        use ulo_rpc_conformance::{report, startup_failed};

        use super::*;

        const PATTERN: &str = "nats.plain";

        /// A plaintext NATS server and its monitoring port, whose `/connz` reports a `tls_version`
        /// for each connection over TLS and none for a plain one.
        async fn plain_nats() -> (SocketAddr, SocketAddr, ContainerAsync<GenericImage>) {
            let (container, addrs) = unshadowed("the plain NATS container", || async {
                let container = GenericImage::new("nats", "2.10.14")
                    .with_exposed_port(4222.tcp())
                    .with_exposed_port(8222.tcp())
                    .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
                    .with_cmd(["-m", "8222"])
                    .start()
                    .await
                    .unwrap_or_else(|error| startup_failed!("the plain NATS container did not start: {}", report(&error)));
                let mut addrs = Vec::new();
                for port in [4222, 8222] {
                    let mapped = container
                        .get_host_port_ipv4(port)
                        .await
                        .unwrap_or_else(|error| startup_failed!("the NATS port {port} is not mapped: {}", report(&error)));
                    addrs.push(SocketAddr::from((Ipv4Addr::LOCALHOST, mapped)));
                }
                (container, addrs)
            })
            .await;
            for addr in &addrs {
                reachable(*addr, Duration::from_secs(10)).await;
            }
            (addrs[0], addrs[1], container)
        }

        async fn connz(monitor: SocketAddr) -> String {
            let mut stream = tokio::net::TcpStream::connect(monitor).await.expect("the monitoring port accepts");
            stream.write_all(b"GET /connz HTTP/1.0\r\nHost: localhost\r\n\r\n").await.expect("the request is written");
            let mut response = String::new();
            stream.read_to_string(&mut response).await.expect("the response reads");
            response
        }

        /// A server's link and a client's on `nats://` urls, each having written its roots, reach
        /// each other through a plaintext server, which reports both connections and neither over
        /// TLS.
        #[tokio::test]
        async fn a_nats_url_to_a_plain_server_stays_plain() {
            let (server, monitor, _container) = plain_nats().await;
            let app = app().await;
            let mut serving = link(&format!("nats://{server}"), test_root::<14>, std::env::temp_dir);
            serving.prepare(&app.handle()).await.expect("the server's link prepares");
            let mut inbound = serving.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens");
            let client = link(&format!("nats://{server}"), test_root::<14>, std::env::temp_dir);
            let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
            let outbound = client.connect(&runtime).await.expect("the client's link connects");
            let request = Frame::Req { id: 1, pattern: PATTERN.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
            (outbound.send)(Pattern::from(PATTERN), request, Some(ReplyTo { id: 1 })).await.expect("the request is published");
            tokio::time::timeout(Duration::from_secs(10), inbound.next())
                .await
                .expect("the request did not reach the server within ten seconds")
                .expect("the server's inbound stream ended");
            assert!(serving.roots.settled().await.is_some() && client.roots.settled().await.is_some(), "a link's roots were not written");
            assert_eq!(LOADS[14].load(Ordering::SeqCst), 2, "the two links each loaded their roots other than once");

            let connz = connz(monitor).await;
            assert_eq!(connz.matches("\"cid\"").count(), 2, "the server does not report the two links' connections: {connz}");
            assert!(!connz.contains("tls_version"), "a connection on a `nats://` url to a plain server uses TLS: {connz}");
            let _ = client.close().await;
            let _ = serving.close().await;
        }

        /// F427: a `nats://` client's link connects, and a request reaches the server, within the
        /// suite's boot budget while the process's load of the client's roots blocks for twice
        /// that; once the load ends, the client's link holds its one file and a later connect is
        /// handed it. The server's link has a load of its own, which its `listen` waits for.
        #[tokio::test]
        async fn a_plain_link_connects_within_the_boot_budget_while_its_roots_load() {
            let boot = ulo_rpc_conformance::Budget::default().boot;
            assert_eq!(HELD, boot * 2, "the gate does not hold the load for twice the boot budget");
            let (server, _monitor, _container) = plain_nats().await;
            let _turn = ProcessTurn::take(Loader { load: gated::<17, 1>, dir: dir::<17> }).await;
            let here = dir::<17>();
            fresh(&here);
            let url = format!("nats://{server}");

            let began = Instant::now();
            let app = app().await;
            let mut serving = link(&url, test_root::<20>, std::env::temp_dir);
            serving.prepare(&app.handle()).await.expect("the server's link prepares");
            let mut inbound = serving.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens");
            let client = Nats::url(&url);
            let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
            let outbound = client.connect(&runtime).await.expect("the client's link connects");
            let request = Frame::Req { id: 1, pattern: PATTERN.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
            (outbound.send)(Pattern::from(PATTERN), request, Some(ReplyTo { id: 1 })).await.expect("the request is published");
            tokio::time::timeout(boot, inbound.next())
                .await
                .expect("the request did not reach the server within the boot budget")
                .expect("the server's inbound stream ended");
            let took = began.elapsed();
            assert!(took < boot, "binding, connecting and one request took {took:?}, past the boot budget of {boot:?}");
            assert_eq!(
                (LOADS[17].load(Ordering::SeqCst), client.roots.path(), LOADS[20].load(Ordering::SeqCst)),
                (1, None, 1),
                "the client did not connect while the process's one load ran, or the server did not load its own once"
            );

            GATES[1].open();
            let file = client.roots.settled().await.expect("the roots are written once the load ends");
            assert_ne!(serving.roots.path().as_ref(), Some(&file), "the server's link holds the process's file, not its own load's");
            let handed = client.roots(&servers(&url).expect("the url parses"), &Tokio::current(), false).await.expect("a plain link is not refused");
            assert_eq!(handed.and_then(|load| load.path()).as_ref(), Some(&file), "a connect after the load ended is not handed its file");
            client.connect(&runtime).await.expect("a later connect succeeds");
            assert_eq!((LOADS[17].load(Ordering::SeqCst), files_in(&here)), (1, vec![file]), "the roots loaded other than once, or into other than one file");

            let _ = client.close().await;
            let _ = serving.close().await;
            drop((client, serving, inbound, outbound));
            assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the two links");
            let _ = std::fs::remove_dir(&here);
        }

        /// F425: a server's link bound through an app removes its roots once the app has closed and
        /// `serve` has returned, the app having let go of its server.
        #[tokio::test]
        async fn a_closed_app_s_server_removes_its_roots() {
            let (server, _monitor, _container) = plain_nats().await;
            let here = dir::<15>();
            fresh(&here);
            let app = app().await.bind(Server::new(link(&format!("nats://{server}"), test_root::<15>, dir::<15>))).listen().await.expect("the server binds");
            let files = written(&here).await.len();
            assert_eq!((LOADS[15].load(Ordering::SeqCst), files), (1, 1), "the server's link did not load and write its roots once");
            let handle = app.handle();
            let serving = tokio::spawn(async move { app.serve(std::future::pending::<ulo::Signal>()).await.map(drop).map_err(|error| error.to_string()) });
            handle.close(ulo::Signal::new("test")).await.expect("the app closes");
            serving.await.expect("the serving task completes").expect("`serve` returns `Ok`");
            assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the closed app's server");
            let _ = std::fs::remove_dir(&here);
        }

        /// A `nats://` server whose roots outlast its `roots_timeout` connects without them once
        /// the bound passes, not once the load ends, and logs one `warn` naming the setting; its
        /// side holds no load, and the server reports its connection plain.
        #[tokio::test]
        async fn a_nats_server_whose_roots_outlast_roots_timeout_connects_without_them() {
            const SHORT: Duration = Duration::from_millis(200);
            const EXPIRED: &str = "did not load within `roots_timeout`, so its server connects to its `nats://` url without them";
            let capture = capture();
            let (server, monitor, _container) = plain_nats().await;
            let app = app().await;
            let mut serving = link(&format!("nats://{server}"), gated::<23, 3>, std::env::temp_dir).roots_timeout(Bound::After(SHORT));
            serving.prepare(&app.handle()).await.expect("the server's link prepares");
            let began = Instant::now();
            let listened = serving.listen(&[Pattern::from(PATTERN)]).await;
            let took = began.elapsed();
            GATES[3].open();
            let _inbound = listened.expect("the server's link listens without its roots");
            assert!(took >= SHORT && took < HELD / 2, "`listen` returned after {took:?}, where the bound is {SHORT:?} and the load {HELD:?}");
            let warned: Vec<String> = capture.warnings(std::thread::current().id()).into_iter().filter(|line| line.contains(EXPIRED)).collect();
            assert_eq!(warned.len(), 1, "the expiry was logged other than once: {warned:?}");
            let side = lock(&serving.state).server.clone().expect("the server's side is held");
            assert!(side._roots.is_none(), "the server's side holds a load its connection was not handed");
            let connz = connz(monitor).await;
            assert_eq!(connz.matches("\"cid\"").count(), 1, "the server does not report the link's connection: {connz}");
            assert!(!connz.contains("tls_version"), "the connection made without roots uses TLS: {connz}");
            assert_eq!(LOADS[23].load(Ordering::SeqCst), 1, "the server loaded its roots other than once");
            drop(side);
            let _ = serving.close().await;
        }

        /// The connections the server reports, once they number `count` or after five seconds.
        async fn connections(monitor: SocketAddr, count: usize) -> usize {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let now = connz(monitor).await.matches("\"cid\"").count();
                if now == count || Instant::now() > deadline {
                    return now;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        /// F428: a link dropped without `close` closes its connection, which its tasks held: a
        /// server's link bound through an app dropped unserved, a server's link holding a call, and
        /// a client's link with what its connect answered, each gone from the server's `/connz`
        /// within five seconds of the drop.
        #[tokio::test]
        async fn a_link_dropped_without_close_closes_its_connection() {
            let (server, monitor, _container) = plain_nats().await;
            let url = format!("nats://{server}");

            let bound = app().await.bind(Server::new(link(&url, test_root::<24>, std::env::temp_dir))).listen().await.expect("the server binds");
            assert_eq!(connections(monitor, 1).await, 1, "the bound server's connection is not reported");
            drop(bound);
            assert_eq!(connections(monitor, 0).await, 0, "the server of an app dropped unserved kept its connection");

            let app = app().await;
            let mut serving = link(&url, test_root::<24>, std::env::temp_dir);
            serving.prepare(&app.handle()).await.expect("the server's link prepares");
            let mut inbound = serving.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens");
            let client = link(&url, test_root::<24>, std::env::temp_dir);
            let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
            let outbound = client.connect(&runtime).await.expect("the client's link connects");
            let request = Frame::Req { id: 1, pattern: PATTERN.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
            (outbound.send)(Pattern::from(PATTERN), request, Some(ReplyTo { id: 1 })).await.expect("the request is published");
            let held = tokio::time::timeout(Duration::from_secs(10), inbound.next())
                .await
                .expect("the request did not reach the server within ten seconds")
                .expect("the server's inbound stream ended");
            assert_eq!(connections(monitor, 2).await, 2, "the server's and the client's connections are not reported");
            drop((serving, inbound, held));
            assert_eq!(connections(monitor, 1).await, 1, "the server's link dropped holding a call kept its connection");
            drop((client, outbound));
            assert_eq!(connections(monitor, 0).await, 0, "the client's link dropped without `close` kept its connection");
        }
    }

    /// F422: a `tls://` link whose crypto provider does not resolve is refused naming the fix, at a
    /// server's bind and on every client call, before its store is read; each side resolves once.
    /// `tests/two_providers.rs` holds the same refusal against rustls's own panic.
    #[tokio::test]
    async fn a_tls_link_whose_provider_does_not_resolve_is_refused_naming_the_fix() {
        let mut server = link("tls://localhost:1", test_root::<12>, std::env::temp_dir);
        server.roots.resolve = two_providers::<0>;
        let refused = refused_at_bind(server).await;
        assert!(refused.contains(PROVIDERS), "the server's refusal does not name the fix: {refused}");
        let mut client = link("tls://localhost:1", test_root::<12>, std::env::temp_dir);
        client.roots.resolve = two_providers::<0>;
        for (kind, reason, message) in refused_calls(client).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains(PROVIDERS), "the call does not name the fix: {message}");
        }
        assert_eq!(RESOLVES[0].load(Ordering::SeqCst), 2, "a side resolved its provider other than once");
        assert_eq!(LOADS[12].load(Ordering::SeqCst), 0, "a side read its store though its provider did not resolve");
    }

    /// F422: a `nats://` link resolves no provider, rustls installing a default on resolving one,
    /// and loads and writes its roots though resolving would fail.
    #[tokio::test]
    async fn a_plain_link_resolves_no_provider() {
        let here = dir::<13>();
        fresh(&here);
        let mut client = link("nats://localhost:1", test_root::<13>, dir::<13>);
        client.roots.resolve = two_providers::<1>;
        let client = RpcClient::new(client, Arc::new(Tokio::current())).expect("the link is usable");
        for (kind, reason, message) in failed_calls(&client).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Unavailable, None), "{message}");
        }
        assert_eq!(RESOLVES[1].load(Ordering::SeqCst), 0, "a plain link resolved a provider");
        let files = written(&here).await.len();
        assert_eq!((LOADS[13].load(Ordering::SeqCst), files), (1, 1), "a plain link did not load and write its roots once");
        drop(client);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the client");
        let _ = std::fs::remove_dir(&here);
    }

    /// F427: thirty links needing their roots at once, the process's load reading a store that
    /// takes 300 ms, run one load between them and hold one file, which goes once the thirty have
    /// dropped; a link needing roots after that loads again.
    #[tokio::test]
    async fn thirty_links_at_once_share_one_load_and_one_file() {
        let _turn = ProcessTurn::take(Loader { load: slow_root::<18>, dir: dir::<18> }).await;
        let here = dir::<18>();
        fresh(&here);
        installed();
        let app = app().await;
        let handle = app.handle();
        let mut links: Vec<Nats> = (0..30).map(|_| Nats::url("tls://localhost:1")).collect();
        for prepared in futures_util::future::join_all(links.iter_mut().map(|link| link.prepare(&handle))).await {
            prepared.expect("a server's link prepares");
        }
        assert_eq!(LOADS[18].load(Ordering::SeqCst), 1, "thirty links loaded their roots other than once");
        let file = links[0].roots.path().expect("the first link holds a file");
        assert!(links.iter().all(|link| link.roots.path().as_ref() == Some(&file)), "the thirty links hold other than one file");
        assert_eq!(files_in(&here), vec![file.clone()], "the load wrote other than one file");
        drop(links);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the file outlived the thirty links");

        let mut later = Nats::url("tls://localhost:1");
        later.prepare(&handle).await.expect("a later server's link prepares");
        assert_eq!(LOADS[18].load(Ordering::SeqCst), 2, "a link after the thirty dropped did not load again");
        let again = later.roots.path().expect("the later link holds a file");
        assert_eq!(files_in(&here), vec![again], "the later load wrote other than one file");
        drop(later);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the later file outlived its link");
        let _ = std::fs::remove_dir(&here);
    }

    /// F427: a `tls://` call that gives up while its roots load, its timeout ending first, drops
    /// its own wait and not the load: the next call waits for the same load, which runs once.
    #[tokio::test]
    async fn a_call_that_gives_up_while_the_roots_load_leaves_the_load_running() {
        let here = dir::<16>();
        fresh(&here);
        let client = RpcClient::new(link("tls://localhost:1", gated::<16, 0>, dir::<16>), Arc::new(Tokio::current())).expect("the link is usable");
        let first = client.request::<_, u32>("nats.roots", &()).timeout(Duration::from_millis(100)).await.expect_err("a call cut short by its timeout");
        assert_eq!(first.kind(), ErrorKind::Timeout, "{}", first.message());
        let second = tokio::spawn({
            let client = client.clone();
            async move {
                let call = client.request::<_, u32>("nats.roots", &()).timeout(HELD).await.expect_err("a call to a server that is not there");
                (call.kind(), call.message().to_owned())
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(LOADS[16].load(Ordering::SeqCst), 1, "a call after one that gave up started a load of its own");
        assert!(!second.is_finished(), "the second call ended before the load did");
        GATES[0].open();
        let (kind, message) = second.await.expect("the second call's task completes");
        assert_eq!(kind, ErrorKind::Unavailable, "{message}");
        assert_eq!(LOADS[16].load(Ordering::SeqCst), 1, "the roots loaded other than once for the two calls");
        assert_eq!(files_in(&here).len(), 1, "the load wrote other than one file");
        drop(client);
        assert_eq!(emptied(&here).await, Vec::<PathBuf>::new(), "the roots outlived the client");
        let _ = std::fs::remove_dir(&here);
    }

    /// `roots_timeout`'s bound as each spelling gives it, on a paused clock: 30 s at
    /// `Bound::Default`, the duration at `Bound::After`, and none at `Bound::Unbounded`, whose wait
    /// ends when the load does, an hour later here.
    #[tokio::test(start_paused = true)]
    async fn roots_timeout_bounds_the_wait_as_its_bound_says() {
        let runtime = Tokio::current();
        let hour = Duration::from_secs(3600);
        for (bound, expected) in [
            (Bound::Default, Err(Duration::from_secs(30))),
            (Bound::After(Duration::from_secs(5)), Err(Duration::from_secs(5))),
            (Bound::Unbounded, Ok(())),
        ] {
            let began = tokio::time::Instant::now();
            let waited = within(&runtime, bound, tokio::time::sleep(hour)).await;
            let took = began.elapsed();
            assert_eq!(waited, expected, "{bound:?}");
            assert_eq!(took, expected.err().unwrap_or(hour), "{bound:?} waited {took:?}");
        }
    }

    /// `.roots_timeout(Bound::After(Duration::ZERO))` is refused at a server's bind as a
    /// `Configure` refusal naming the setting and `Bound::Unbounded`, on either scheme, before
    /// any load starts.
    #[tokio::test]
    async fn a_zero_roots_timeout_is_refused_at_bind() {
        for url in ["nats://localhost:1", "tls://localhost:1"] {
            let refused = refused_at_bind(link(url, test_root::<19>, std::env::temp_dir).roots_timeout(Bound::After(Duration::ZERO))).await;
            assert!(
                refused.contains("`.roots_timeout(Bound::After(Duration::ZERO))` would") && refused.contains("write `Bound::Unbounded`"),
                "the refusal on `{url}` does not name the setting and the fix: {refused}"
            );
        }
        assert_eq!(LOADS[19].load(Ordering::SeqCst), 0, "a link refused for its zero bound loaded its roots");
    }

    /// `.roots_timeout(Bound::After(Duration::ZERO))` is refused when a client is built, as
    /// `UnusableLink` naming the setting and `Bound::Unbounded`, on either scheme, before any load
    /// starts.
    #[tokio::test]
    async fn a_zero_roots_timeout_is_refused_when_a_client_is_built() {
        for url in ["nats://localhost:1", "tls://localhost:1"] {
            let link = link(url, test_root::<28>, std::env::temp_dir).roots_timeout(Bound::After(Duration::ZERO));
            let refused = match RpcClient::new(link, Arc::new(Tokio::current())) {
                Err(refused) => refused.to_string(),
                Ok(_) => panic!("a client over a zero `roots_timeout` was built on `{url}`"),
            };
            assert!(
                refused.contains("`.roots_timeout(Bound::After(Duration::ZERO))` would") && refused.contains("write `Bound::Unbounded`"),
                "the refusal on `{url}` does not name the setting and the fix: {refused}"
            );
        }
        assert_eq!(LOADS[28].load(Ordering::SeqCst), 0, "a client refused for its zero bound loaded its roots");
    }

    /// A `tls://` link whose roots outlast its `roots_timeout` is refused once the bound passes,
    /// not once the load ends, naming the setting: a server's bind as a `Configure` refusal, and
    /// each client call `Internal` with `reason: "misconfigured_link"`. Each side loads once, the
    /// client's second call joining the load its first left running.
    #[tokio::test]
    async fn a_tls_link_whose_roots_outlast_roots_timeout_is_refused() {
        const SHORT: Duration = Duration::from_millis(200);
        let began = Instant::now();
        let refused = refused_at_bind(link("tls://localhost:1", gated::<21, 2>, std::env::temp_dir).roots_timeout(Bound::After(SHORT))).await;
        assert!(refused.contains("the load did not end within `roots_timeout`'s 200ms"), "the server's refusal does not name the bound: {refused}");
        for (kind, reason, message) in refused_calls(link("tls://localhost:1", gated::<22, 2>, std::env::temp_dir).roots_timeout(Bound::After(SHORT))).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains("the load did not end within `roots_timeout`'s 200ms"), "the call does not name the bound: {message}");
        }
        let took = began.elapsed();
        GATES[2].open();
        assert!(took < HELD / 2, "three waits bounded at {SHORT:?} took {took:?}, as long as the load");
        assert_eq!((LOADS[21].load(Ordering::SeqCst), LOADS[22].load(Ordering::SeqCst)), (1, 1), "a side loaded its roots other than once");
    }

    /// A `nats://` server's waits for its roots log one `warn` per distinct cause. A load
    /// outlasting `roots_timeout` and failing later logs two, the expiry's from `listen`'s wait and
    /// the failure's, with its cause, from the task `prepare`'s call armed; a load failing inside
    /// the bound logs the failure once, `listen`'s wait and the armed task sharing the load's one
    /// `warn`. No broker is reached: the calls are the two `prepare` and `listen` make.
    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn a_nats_server_s_roots_log_one_warn_per_cause() {
        const SHORT: Duration = Duration::from_millis(200);
        const EXPIRED: &str = "did not load within `roots_timeout`";
        const FAILED: &str = "connects to its `nats://` url without root certificates of its own";
        let capture = capture();
        let here = std::thread::current().id();
        let count = |text: &str| capture.warnings(here).into_iter().filter(|line| line.contains(text)).count();
        let runtime = Tokio::current();
        let servers = servers("nats://localhost:1").expect("the url parses");

        let late = link("nats://localhost:1", gated_unreadable::<25, 4>, std::env::temp_dir).roots_timeout(Bound::After(SHORT));
        assert!(late.roots(&servers, &runtime, false).await.expect("`prepare`'s call answers").is_none(), "a running load answered a file");
        assert!(late.roots(&servers, &runtime, true).await.expect("`listen`'s call answers").is_none(), "an expired wait answered a file");
        GATES[4].open();
        let deadline = Instant::now() + Duration::from_secs(5);
        while count(FAILED) == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!((count(EXPIRED), count(FAILED)), (1, 1), "a load failing after the bound was logged other than once per cause: {:?}", capture.warnings(here));

        let soon = link("nats://localhost:1", unreadable::<26>, std::env::temp_dir).roots_timeout(Bound::After(SHORT));
        assert!(soon.roots(&servers, &runtime, false).await.expect("`prepare`'s call answers").is_none(), "a failing load answered a file");
        assert!(soon.roots(&servers, &runtime, true).await.expect("`listen`'s call answers").is_none(), "a failed load answered a file");
        // Room for the armed task, which ends once the load's answer reaches it.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!((count(EXPIRED), count(FAILED)), (1, 2), "a load failing inside the bound was logged other than once: {:?}", capture.warnings(here));
        assert_eq!((LOADS[25].load(Ordering::SeqCst), LOADS[26].load(Ordering::SeqCst)), (1, 1), "a link loaded its roots other than once");
    }

    /// F431: a `nats://` server whose roots' load ends unanswered, the runtime it ran on having
    /// shut down before the load's task ran, connects without them and logs one `warn` saying
    /// so, as an expiry does. The link's load is started on a runtime that has shut down.
    #[cfg(feature = "integration")]
    #[tokio::test]
    async fn a_nats_server_whose_roots_load_ends_unanswered_says_so() {
        const UNANSWERED: &str = "load ended without an answer";
        let capture = capture();
        let here = std::thread::current().id();
        let stopped = std::thread::spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a runtime to shut down");
            let handle = runtime.handle().clone();
            drop(runtime);
            handle
        })
        .join()
        .expect("the runtime shuts down");
        let servers = servers("nats://localhost:1").expect("the url parses");
        let serving = link("nats://localhost:1", test_root::<27>, std::env::temp_dir).roots_timeout(Bound::Unbounded);
        let roots = serving.roots(&servers, &Tokio::from_handle(stopped), true).await.expect("`listen`'s call answers");
        assert!(roots.is_none(), "a load that never ran answered a file");
        let warned: Vec<String> = capture.warnings(here).into_iter().filter(|line| line.contains(UNANSWERED)).collect();
        assert_eq!(warned.len(), 1, "the unanswered load was logged other than once: {warned:?}");
        assert_eq!(LOADS[27].load(Ordering::SeqCst), 0, "the load ran on a runtime that had shut down");
    }

    /// The process's subscriber, installed by the first test that reads it: tracing caches whether
    /// a callsite is enabled, and a thread-local subscriber would not see a callsite another
    /// test's thread reached first.
    #[cfg(feature = "integration")]
    fn capture() -> &'static Capture {
        static CAPTURE: OnceLock<Capture> = OnceLock::new();
        CAPTURE.get_or_init(|| {
            let capture = Capture::default();
            tracing::subscriber::set_global_default(capture.clone()).expect("no other subscriber is installed");
            capture
        })
    }

    /// Every `warn` and `error` event's message, with the thread it was emitted on.
    #[cfg(feature = "integration")]
    #[derive(Clone, Default)]
    struct Capture {
        lines: Arc<Mutex<Vec<(std::thread::ThreadId, String)>>>,
        next_span: Arc<AtomicU64>,
    }

    #[cfg(feature = "integration")]
    impl Capture {
        fn warnings(&self, on: std::thread::ThreadId) -> Vec<String> {
            lock(&self.lines).iter().filter(|(thread, _)| *thread == on).map(|(_, line)| line.clone()).collect()
        }
    }

    #[cfg(feature = "integration")]
    impl tracing::Subscriber for Capture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.is_span() || *metadata.level() <= tracing::Level::WARN
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            struct Message(String);
            impl tracing::field::Visit for Message {
                fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = Message(String::new());
            event.record(&mut message);
            lock(&self.lines).push((std::thread::current().id(), message.0));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// The link against a NATS server serving TLS from the test certificate, which the suite's own
    /// server does not, the roots the test certificate alone from a loader that blocks twice the
    /// bound, as a trust store slow to read does.
    #[cfg(feature = "integration")]
    mod over_tls {
        use std::net::{Ipv4Addr, SocketAddr};
        use std::sync::atomic::AtomicBool;
        use std::thread::{self, ThreadId};
        use std::time::Instant;

        use futures_util::StreamExt;
        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{ContainerAsync, GenericImage, ImageExt};
        use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
        use ulo_rpc_conformance::{Broker, Budget, Harness, OnTokio, report, startup_failed};

        use super::*;

        const PORT: u16 = 4222;

        /// The bound these tests hold the thread to, the MQTT link's over TLS: half the suite's.
        const STALL: Duration = Duration::from_millis(25);

        /// The test certificate as the store's one root, after blocking twice [`STALL`], as a
        /// trust store slow to read does: the platform's takes 71 ms warm on macOS, past the
        /// bound already. Counted in `SLOT` of [`TLS_LOADS`].
        fn slow_test_roots<const SLOT: usize>() -> CertificateResult {
            TLS_LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(STALL * 2);
            let mut loaded = CertificateResult::default();
            loaded.certs.push(ulo_test_certs::localhost().cert_der().clone());
            loaded
        }

        static TLS_LOADS: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

        /// A NATS server requiring TLS, reached directly by [`Broker::link`] and through `relay` by
        /// [`TlsNats::relayed`]. On a `tls://` url each link has a load of its own from
        /// [`slow_test_roots`]. With `PLAIN_URL` set the links are on `nats://` urls, which
        /// async-nats upgrades on reading the server's `INFO`, and join the process's load, which
        /// `start` makes and holds: a `nats://` client waits for no load, so only a connection made
        /// once one has ended is handed its file.
        struct TlsNats<const SLOT: usize, const PLAIN_URL: bool = false> {
            server: SocketAddr,
            relay: Relay,
            _roots: Option<Arc<Load>>,
            _container: ContainerAsync<GenericImage>,
        }

        impl<const SLOT: usize, const PLAIN_URL: bool> TlsNats<SLOT, PLAIN_URL> {
            fn over(port: u16) -> Nats {
                installed();
                let scheme = if PLAIN_URL { "nats" } else { "tls" };
                let mut link = Nats::url(format!("{scheme}://localhost:{port}"));
                if !PLAIN_URL {
                    link.roots.own = Some(Loader { load: slow_test_roots::<SLOT>, dir: std::env::temp_dir });
                }
                link
            }

            /// A link reaching the server through the relay, which a cut closes.
            fn relayed(&self) -> Nats {
                Self::over(self.relay.addr().port())
            }
        }

        impl<const SLOT: usize, const PLAIN_URL: bool> Broker for TlsNats<SLOT, PLAIN_URL> {
            type Harness = OnTokio;
            type Link = Nats;

            async fn start() -> Self {
                let cert = ulo_test_certs::localhost();
                let (container, addrs) = unshadowed("the TLS NATS container", || async {
                    let container = GenericImage::new("nats", "2.10.14")
                        .with_exposed_port(PORT.tcp())
                        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
                        .with_copy_to("/tls/server.crt", cert.cert_pem().as_bytes().to_vec())
                        .with_copy_to("/tls/server.key", cert.key_pem().as_bytes().to_vec())
                        .with_cmd(["--tls", "--tlscert", "/tls/server.crt", "--tlskey", "/tls/server.key"])
                        .start()
                        .await
                        .unwrap_or_else(|error| startup_failed!("the TLS NATS container did not start: {}", report(&error)));
                    let port = container
                        .get_host_port_ipv4(PORT)
                        .await
                        .unwrap_or_else(|error| startup_failed!("the NATS TLS port is not mapped: {}", report(&error)));
                    (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
                })
                .await;
                reachable(addrs[0], Duration::from_secs(10)).await;
                let roots = if PLAIN_URL {
                    let load = Load::shared(&Tokio::current());
                    assert!(matches!(load.wait().await, Some(Ok(_))), "the process's roots did not load");
                    Some(load)
                } else {
                    None
                };
                TlsNats { server: addrs[0], relay: Relay::start(addrs[0]).await, _roots: roots, _container: container }
            }

            fn link(&self) -> Nats {
                Self::over(self.server.port())
            }

            async fn disrupt(&self) {
                self.relay.cut().await;
            }

            fn budget(&self) -> Budget {
                Budget::default().with_stall(STALL)
            }
        }

        /// F417: connecting over `tls://` does not stall the thread polling the connect, the
        /// suite's `connecting_does_not_stall_the_thread` against [`TlsNats`] with a bound of
        /// [`STALL`], each link's roots blocking twice the bound while they load. Each link loads
        /// them once, the server's and each of the scenario's three clients'.
        #[test]
        fn connecting_over_tls_does_not_stall_the_thread() {
            OnTokio::block_on_one_thread(ulo_rpc_conformance::cases::threads::connecting_does_not_stall_the_thread::<TlsNats<0>>());
            assert_eq!(TLS_LOADS[0].load(Ordering::SeqCst), 4, "the server's link and three clients' each loaded their roots other than once");
        }

        /// F423: connecting on a `nats://` url to a server requiring TLS, which async-nats upgrades,
        /// does not stall the thread either once the process's roots have loaded: the scenario
        /// against [`TlsNats`] on `nats://` urls, every link joining the load the broker made
        /// before the scenario. The four links load once between them, and their connections trust
        /// the test certificate through its file.
        #[test]
        fn connecting_on_a_nats_url_to_a_server_requiring_tls_does_not_stall_the_thread() {
            let _turn = ProcessTurn::blocking(Loader { load: slow_test_roots::<2>, dir: std::env::temp_dir });
            OnTokio::block_on_one_thread(ulo_rpc_conformance::cases::threads::connecting_does_not_stall_the_thread::<TlsNats<2, true>>());
            assert_eq!(TLS_LOADS[2].load(Ordering::SeqCst), 1, "the server's link and three clients' did not share one load");
        }

        /// What [`Ticks::watch`] saw in each phase the test named: the longest gap that ended in
        /// it, and how many gaps did.
        #[derive(Default)]
        struct Ticks {
            phases: Vec<(&'static str, Duration, usize)>,
            threads: Vec<ThreadId>,
        }

        impl Ticks {
            /// A task sleeping a tenth of [`STALL`] between ticks, each gap counted in the phase
            /// `doing` names when the gap ends, until `stop` is raised.
            async fn watch(doing: Arc<Mutex<&'static str>>, stop: Arc<AtomicBool>) -> Ticks {
                let mut ticks = Ticks::default();
                let mut last = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    tokio::time::sleep(STALL / 10).await;
                    let (now, during) = (Instant::now(), *lock(&doing));
                    match ticks.phases.iter_mut().find(|(phase, ..)| *phase == during) {
                        Some((_, longest, count)) => {
                            *longest = (*longest).max(now - last);
                            *count += 1;
                        }
                        None => ticks.phases.push((during, now - last, 1)),
                    }
                    last = now;
                    if !ticks.threads.contains(&thread::current().id()) {
                        ticks.threads.push(thread::current().id());
                    }
                }
                ticks
            }
        }

        const PATTERN: &str = "nats.reconnect";

        fn request(id: u64) -> Frame {
            Frame::Req { id, pattern: PATTERN.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) }
        }

        /// F417: a reconnect over `tls://` does not stall the thread either. On one thread beside a
        /// ticker, a server's link and a client's connect through the relay and a request reaches
        /// the server; the relay then closes both connections, and requests from a client
        /// connection made after the cut are sent until one reaches the server again, which needs
        /// the server's own connection to have reconnected inside async-nats, reading the roots'
        /// file again and resubscribing. Every gap stays under [`STALL`], and some end while the
        /// connections reconnect. Each link loads its roots once, each load blocking twice the
        /// bound, so a reconnect reuses them.
        #[test]
        fn reconnecting_over_tls_does_not_stall_the_thread() {
            OnTokio::block_on_one_thread(async {
                let broker = TlsNats::<1>::start().await;
                let doing = Arc::new(Mutex::new("binding the server"));
                let stop = Arc::new(AtomicBool::new(false));
                let ticker = tokio::spawn(Ticks::watch(Arc::clone(&doing), Arc::clone(&stop)));
                let phase = |what: &'static str| *lock(&doing) = what;

                let app = app().await;
                let mut server = broker.relayed();
                server.prepare(&app.handle()).await.expect("the server's link prepares");
                let mut inbound = server.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens");
                phase("connecting a client");
                let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
                let client = broker.relayed();
                let outbound = client.connect(&runtime).await.expect("the client's link connects");
                (outbound.send)(Pattern::from(PATTERN), request(1), Some(ReplyTo { id: 1 })).await.expect("the request is published");
                tokio::time::timeout(Duration::from_secs(10), inbound.next())
                    .await
                    .expect("the first request did not reach the server within ten seconds")
                    .expect("the server's inbound stream ended");

                phase("reconnecting");
                let severed = broker.relay.cut().await;
                assert!(severed >= 2, "the cut closed {severed} connections, not the server's and the client's");
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut id = 2;
                loop {
                    assert!(Instant::now() < deadline, "no request reached the server within 20 s of the cut");
                    if let Ok(outbound) = client.connect(&runtime).await
                        && (outbound.send)(Pattern::from(PATTERN), request(id), Some(ReplyTo { id })).await.is_ok()
                        && tokio::time::timeout(Duration::from_millis(500), inbound.next()).await.is_ok()
                    {
                        break;
                    }
                    id += 1;
                }
                phase("stopping the ticker");
                stop.store(true, Ordering::Release);
                let ticks = ticker.await.expect("the ticker does not panic");
                let _ = client.close().await;
                let _ = server.close().await;

                assert_eq!(ticks.threads, vec![thread::current().id()], "the ticker was polled on a thread other than the test's");
                for (during, longest, _) in &ticks.phases {
                    assert!(
                        *longest < STALL,
                        "the test's thread went {:.1} ms without polling the ticker while {during}; the bound is {:.1} ms",
                        longest.as_secs_f64() * 1e3,
                        STALL.as_secs_f64() * 1e3
                    );
                }
                let reconnecting = ticks.phases.iter().find(|(during, ..)| *during == "reconnecting").map_or(0, |(.., count)| *count);
                assert!(reconnecting >= 1, "no tick ended while the connections reconnected");
                assert_eq!(TLS_LOADS[1].load(Ordering::SeqCst), 2, "the server's link and the client's each loaded their roots other than once");
            });
        }

        /// How long [`past_boot`] blocks: past the suite's boot budget, inside `roots_timeout`'s
        /// default.
        const PAST_BOOT: Duration = Duration::from_secs(6);

        /// The test certificate as the store's one root after [`PAST_BOOT`], counted in `SLOT` of
        /// [`TLS_LOADS`].
        fn past_boot<const SLOT: usize>() -> CertificateResult {
            TLS_LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(PAST_BOOT);
            let mut loaded = CertificateResult::default();
            loaded.certs.push(ulo_test_certs::localhost().cert_der().clone());
            loaded
        }

        /// A `nats://` server's `listen` waits for its roots, whose load blocks past the boot
        /// budget, and connects with their file to a server requiring TLS, which upgrades the
        /// connection; the relay's cut then forces a reconnect of that connection, which a request
        /// reaching the server shows, and the reconnect uses the file too. The connection made
        /// without the file, or a reconnect reading the platform's store, would trust the
        /// platform's roots, which hold no test certificate, and hold the thread while it read
        /// them: on one thread beside a ticker, every gap stays under [`STALL`] while the server
        /// binds and while it reconnects. The server's link loads once and the client's, on
        /// `tls://`, once.
        #[test]
        fn a_nats_server_waits_for_its_roots_and_reconnects_with_them() {
            let boot = Budget::default().boot;
            assert!(PAST_BOOT > boot && PAST_BOOT < ROOTS_TIMEOUT, "the load does not block past the boot budget and inside the default bound");
            OnTokio::block_on_one_thread(async {
                let broker = TlsNats::<3>::start().await;
                let doing = Arc::new(Mutex::new("binding the server"));
                let stop = Arc::new(AtomicBool::new(false));
                let ticker = tokio::spawn(Ticks::watch(Arc::clone(&doing), Arc::clone(&stop)));
                let phase = |what: &'static str| *lock(&doing) = what;

                let app = app().await;
                installed();
                let mut server = Nats::url(format!("nats://localhost:{}", broker.relay.addr().port()));
                server.roots.own = Some(Loader { load: past_boot::<4>, dir: std::env::temp_dir });
                let began = Instant::now();
                server.prepare(&app.handle()).await.expect("the server's link prepares");
                let mut inbound = server.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens over the upgrade");
                let waited = began.elapsed();
                assert!(waited >= PAST_BOOT, "the server's `listen` returned after {waited:?}, before its roots' load of {PAST_BOOT:?} ended");
                let side = lock(&server.state).server.clone().expect("the server's side is held");
                assert!(side._roots.is_some(), "the server's connection was not handed the file");
                drop(side);

                phase("connecting a client");
                let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
                let client = broker.relayed();
                let outbound = client.connect(&runtime).await.expect("the client's link connects");
                (outbound.send)(Pattern::from(PATTERN), request(1), Some(ReplyTo { id: 1 })).await.expect("the request is published");
                tokio::time::timeout(Duration::from_secs(10), inbound.next())
                    .await
                    .expect("the first request did not reach the server within ten seconds")
                    .expect("the server's inbound stream ended");

                phase("reconnecting");
                let severed = broker.relay.cut().await;
                assert!(severed >= 2, "the cut closed {severed} connections, not the server's and the client's");
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut id = 2;
                loop {
                    assert!(Instant::now() < deadline, "no request reached the server within 20 s of the cut");
                    if let Ok(outbound) = client.connect(&runtime).await
                        && (outbound.send)(Pattern::from(PATTERN), request(id), Some(ReplyTo { id })).await.is_ok()
                        && tokio::time::timeout(Duration::from_millis(500), inbound.next()).await.is_ok()
                    {
                        break;
                    }
                    id += 1;
                }
                phase("stopping the ticker");
                stop.store(true, Ordering::Release);
                let ticks = ticker.await.expect("the ticker does not panic");
                let _ = client.close().await;
                let _ = server.close().await;

                assert_eq!(ticks.threads, vec![thread::current().id()], "the ticker was polled on a thread other than the test's");
                for (during, longest, _) in &ticks.phases {
                    assert!(
                        *longest < STALL,
                        "the test's thread went {:.1} ms without polling the ticker while {during}; the bound is {:.1} ms",
                        longest.as_secs_f64() * 1e3,
                        STALL.as_secs_f64() * 1e3
                    );
                }
                for during in ["binding the server", "reconnecting"] {
                    let ticked = ticks.phases.iter().find(|(phase, ..)| *phase == during).map_or(0, |(.., count)| *count);
                    assert!(ticked >= 1, "no tick ended while {during}");
                }
                assert_eq!(
                    (TLS_LOADS[4].load(Ordering::SeqCst), TLS_LOADS[3].load(Ordering::SeqCst)),
                    (1, 1),
                    "the server's link or the client's loaded its roots other than once"
                );
            });
        }
    }
}
