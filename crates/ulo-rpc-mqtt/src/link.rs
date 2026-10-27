use std::collections::{BTreeMap, HashMap, VecDeque};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use rumqttc::v5::mqttbytes::QoS as MqttQoS;
use rumqttc::v5::mqttbytes::v5::{
    Filter, Packet, PubAckReason, PubRecReason, Publish, PublishProperties, SubscribeReasonCode, UnsubAck, UnsubAckReason,
};
use rumqttc::tokio_rustls::rustls::crypto::{CryptoProvider, aws_lc_rs};
use rumqttc::tokio_rustls::rustls::{ClientConfig, RootCertStore};
use rumqttc::v5::{AsyncClient, Event, EventLoop, MqttOptions};
use rumqttc::{Outgoing, TlsConfiguration, Transport};
use rustls_native_certs::CertificateResult;
use tokio::runtime::Handle;
use tokio::sync::{Notify, OnceCell, mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};
use ulo::{AppHandle, BoxError, BoxFuture, Runtime, Timer};
use ulo_runtime_tokio::Tokio;
#[cfg(feature = "conformance-probe")]
use ulo_rpc::__private::ClientProbe;
use ulo_rpc::__private::HoldUntil;
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, ErrorBody, Frame, FrameTooLarge, Link, Misconfigured, NoTimer,
    Ordering as Order, Outbound, Pattern, ReplyPath, ReplyTo,
};
use ulo_transport::{Detail, Details, ErrorKind, SHUTDOWN_MESSAGE, Turns};

/// The user property naming a message's frame kind where the message alone does not tell it:
/// `open` on the request lane, `in`, `in_end` and `cancel` on the control lane, `opened` on the
/// reply lane. A request-lane message without it is a `req` when it carries a Response Topic and
/// an `evt` when it does not.
const KIND: &str = "ulo-t";
const OPEN: &str = "open";
const OPENED: &str = "opened";
const IN: &str = "in";
const IN_END: &str = "in_end";
const CANCEL: &str = "cancel";

/// The topic every server instance subscribes to outside any shared subscription, so each sees
/// every control message and acts on the ones whose Correlation Data names a call it holds.
const CONTROL: &str = "ulo/rpc/control";

/// The `ErrorInfo` domain of the `no_destination` error this link writes for PUBACK 0x10.
const DOMAIN: &str = "ulo.rpc";

/// The largest packet this link accepts from the broker, MQTT's own ceiling; rumqttc's default
/// of 10 KiB would refuse ordinary replies.
const MAX_INCOMING: u32 = 268_435_455;

/// The Receive Maximum this link announces in its CONNECT, MQTT 5's largest: the QoS 1 and 2
/// publishes the broker may have unacknowledged to it at once. rumqttc acknowledges a publish as
/// its event loop reads it, so the window bounds only what is on its way; the server's own
/// in-flight bound, `Server::max_inflight`, refuses what it will not take. MQTT 5 reads an absent
/// Receive Maximum as 65,535, but Mosquitto 2.0 then applies its own `max_inflight_messages`, 20
/// unset, and holds the rest back, and what it holds back is dropped with the session when the
/// instance leaves. The crate doc says why the link refuses rather than holds work at the broker.
const RECEIVE_MAXIMUM: u16 = u16::MAX;

/// The capacity of the server side's rumqttc request channel. A reply finding it full waits for
/// room outside the lock `close` takes; `close`'s own answers that find it full wait once the lock
/// is closed to every other sender. The event loop takes requests while fewer publishes are
/// unacknowledged than the broker's Receive Maximum, so the channel fills only when the broker
/// stops reading or acknowledging, or when replies are queued faster than the broker acknowledges
/// them until this many wait. rumqttc's channel allocates as it fills.
const SERVER_REQUESTS: usize = 65_536;

/// How long a connection's event loop, a server's or a client's, outlives `close`, completed or
/// cut, to read until the broker closes the connection (transports DESIGN §5.2). Dropping the
/// socket with the broker's packets unread resets the connection, and Mosquitto then discards what
/// it received and had not yet read, the DISCONNECT and what was queued ahead of it among them.
const CUT_GRACE: Duration = Duration::from_secs(2);

/// The MQTT v5 link.
///
/// The link's connections and tasks live on the tokio runtime it holds, the one current where it
/// was built or the one [`with_handle`](Self::with_handle) names, so its futures and streams may
/// be polled on any executor or on a plain thread: each connection's event loop runs there, and a
/// publish, an unsubscribe or a disconnect reaches it through rumqttc's request channel. A link
/// built outside a runtime and given none refuses in `usable`, so a client is refused where it
/// takes the link and a server's `listen()` fails, and in `connect`.
///
/// A link dropped without `close`, as a server's is when its app drops before the close step,
/// aborts its event loops, so its connections close, without the DISCONNECT `close` writes.
pub struct Mqtt {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) qos: QoS,
    pub(crate) codec: Codec,
    pub(crate) runtime: Option<Tokio>,
    /// The app's clock, read by `prepare`, which times a server's [`CUT_GRACE`]. A server that
    /// listens without `prepare`, outside an app, times it on `runtime`.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    /// The root module's full type path, written by `prepare` when no `group` is set.
    pub(crate) default_group: Option<String>,
    /// The CONNACK's Maximum Packet Size, zero until a connection reads one.
    pub(crate) max_packet: Arc<AtomicU64>,
    /// The root certificates an `mqtts://` url connects with.
    pub(crate) roots: Roots,
    pub(crate) state: Mutex<State>,
    #[cfg(feature = "conformance-probe")]
    pub(crate) probe: Arc<ClientProbe>,
}

#[derive(Default)]
pub(crate) struct State {
    server: Option<(Arc<ServerSide>, Loop)>,
    client: Option<(Arc<ClientSide>, Loop)>,
}

/// A connection's event loop: its task, the turn `close` gives it before it waits, and the two
/// ends of its linger.
struct Loop {
    task: JoinHandle<()>,
    turns: Turns,
    /// Answered once the loop has written the DISCONNECT `close` queued, and dropped if it ends
    /// first.
    written: oneshot::Receiver<()>,
    /// Dropped as `close` ends, completed or cut, which starts the loop's [`CUT_GRACE`].
    released: oneshot::Sender<()>,
}

impl Loop {
    /// The connection `connection` builds, spawned on `runtime` inside [`linger`] and tracked for
    /// the turn `close` gives it. Both sides' loops are spawned here, so both linger alike.
    fn spawn<F>(runtime: &Tokio, timer: Arc<dyn Timer>, connection: impl FnOnce(Written) -> F) -> Loop
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (written, disconnected) = oneshot::channel();
        let (released, closed) = oneshot::channel();
        let turns = Turns::default();
        let task = runtime.handle().spawn(turns.track(linger(connection(Written(Some(written))), closed, timer)));
        Loop { task, turns, written: disconnected, released }
    }
}

/// Runs `connection` until it ends, or for [`CUT_GRACE`] on `timer` once `close` has ended,
/// completed or cut, whichever comes first. Each side's connection polls past the DISCONNECT and
/// ends when the broker closes the connection, so the socket is not dropped with the broker's
/// packets unread; the grace bounds the wait on a broker that never closes it.
async fn linger(connection: impl Future<Output = ()>, released: oneshot::Receiver<()>, timer: Arc<dyn Timer>) {
    let grace = async {
        let _ = released.await;
        timer.sleep(CUT_GRACE).await;
    };
    tokio::select! {
        biased;
        () = grace => tracing::debug!("the MQTT link dropped its connection `CUT_GRACE` after `close`"),
        () = connection => {}
    }
}

/// A connection's report that it has written the DISCONNECT `close` queued.
struct Written(Option<oneshot::Sender<()>>);

impl Written {
    /// Called on the `Outgoing::Disconnect` that `close` queued. MQTT allows no packet after the
    /// DISCONNECT, so a QoS 1 publish the loop reads past it is left unacknowledged rather than
    /// answered with a PUBACK.
    fn disconnect(&mut self, eventloop: &mut EventLoop) {
        eventloop.state.manual_acks = true;
        if let Some(written) = self.0.take() {
            let _ = written.send(());
        }
    }
}

/// A link dropped without `close` aborts the event loops `close` would have shut, each of which
/// owns its connection: a `JoinHandle` detaches on drop, and a detached loop keeps its connection,
/// its subscriptions and its reconnects until the process ends.
impl Drop for State {
    fn drop(&mut self) {
        if let Some((_, event_loop)) = &self.server {
            event_loop.task.abort();
        }
        if let Some((_, event_loop)) = &self.client {
            event_loop.task.abort();
        }
    }
}

impl Mqtt {
    /// The link on `url`, `mqtt://host:1883` or `mqtts://host:8883`, parsed in `prepare` and
    /// connected lazily.
    pub fn url(url: impl Into<String>) -> Self {
        Mqtt {
            url: url.into(),
            group: None,
            qos: QoS::AtLeastOnce,
            codec: Codec::Json,
            runtime: Tokio::try_current(),
            timer: None,
            default_group: None,
            max_packet: Arc::new(AtomicU64::new(0)),
            roots: Roots::platform(),
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

    /// The shared-subscription group server instances share, in place of the root module's full
    /// type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The QoS every publish and subscription uses: `AtLeastOnce` unset. At `AtMostOnce` a miss
    /// has no signal and is the client's `Timeout`.
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
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

    fn runtime(&self) -> Result<Tokio, BoxError> {
        self.runtime.clone().ok_or_else(|| NO_RUNTIME.into())
    }

    /// The TLS configuration `target` connects with: the [`Roots`] for `mqtts://`, none for
    /// `mqtt://`.
    async fn tls(&self, target: &Target, runtime: &Tokio) -> Result<Option<TlsConfiguration>, BoxError> {
        if target.tls { self.roots.load(runtime).await.map(Some) } else { Ok(None) }
    }
}

const NO_RUNTIME: &str = "the MQTT link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

/// An MQTT quality of service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QoS {
    /// QoS 0: nothing is acknowledged, so a miss has no signal.
    AtMostOnce,
    /// QoS 1, with PUBACK.
    #[default]
    AtLeastOnce,
    /// QoS 2, with PUBREC.
    ExactlyOnce,
}

impl QoS {
    fn wire(self) -> MqttQoS {
        match self {
            QoS::AtMostOnce => MqttQoS::AtMostOnce,
            QoS::AtLeastOnce => MqttQoS::AtLeastOnce,
            QoS::ExactlyOnce => MqttQoS::ExactlyOnce,
        }
    }
}

impl Link for Mqtt {
    const NAME: &'static str = "mqtt";

    fn capabilities(&self) -> Capabilities {
        let max_packet = self.max_packet.load(Ordering::Relaxed);
        Capabilities::new(DeliveryMode::Competing)
            .binary(self.codec.binary())
            .ordering(Order::PerTopic)
            .miss_signal(self.qos != QoS::AtMostOnce)
            .max_frame((max_packet > 0).then_some(max_packet))
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.runtime().map(drop)
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.usable()?;
        let target = Target::parse(&self.url)?;
        match &self.group {
            Some(group) => check_group(group)?,
            None => self.default_group = Some(share_name(&format!("{:#}", app.root().name()))),
        }
        self.timer = app.timer().cloned();
        // A server loads its roots here, so a store that yields none fails `bind` before `listen`.
        self.tls(&target, &self.runtime()?).await?;
        Ok(())
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let group = self
            .group
            .clone()
            .or_else(|| self.default_group.clone())
            .ok_or("the MQTT link's group is unset: `listen` ran before `prepare`")?;
        let timer = self.timer.clone().ok_or(NoTimer { link: "MQTT" })?;
        let runtime = self.runtime()?;
        let target = Target::parse(&self.url)?;
        let tls = self.tls(&target, &runtime).await?;
        let (client, eventloop) = AsyncClient::new(target.options(client_id(), tls), SERVER_REQUESTS);
        let shared: Vec<String> = patterns.iter().map(|pattern| format!("$share/{group}/{pattern}")).collect();
        let (phase, _) = watch::channel(Phase::Serving);
        let (deliveries, inbound) = mpsc::unbounded_channel();
        let side = Arc::new(ServerSide {
            client,
            codec: self.codec,
            qos: self.qos.wire(),
            runtime: runtime.clone(),
            shared,
            calls: Arc::new(Calls::new()),
            replies: Arc::default(),
            phase,
            max_packet: Arc::clone(&self.max_packet),
            deliveries: Mutex::new(Some(deliveries)),
            unsubscribing: Mutex::new(Unsubscribing::default()),
            unconfirmed: watch::channel(0).0,
        });

        let (ready, bound) = oneshot::channel();
        let served = Arc::clone(&side);
        let event_loop = Loop::spawn(&runtime, timer, move |written| serve(served, eventloop, ready, written));
        // Until the broker answers, a `listen` dropped mid-handshake takes the event loop with it.
        let pending = AbortOnDrop::new(event_loop.task.abort_handle());
        // `bind`'s outcome waits for the first CONNACK and the SUBACK of every subscription, so a
        // broker without shared subscriptions, or one refusing a filter, fails `bind`.
        let outcome = bound.await.unwrap_or_else(|_| Err("the MQTT link's event loop ended before the broker answered".into()));
        if let Err(error) = outcome {
            let _ = side.client.try_disconnect();
            return Err(error);
        }
        pending.disarm();
        lock(&self.state).server = Some((side, event_loop));
        Ok(receiver_stream(inbound))
    }

    async fn connect(&self, handed: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let runtime = self.runtime()?;
        let target = Target::parse(&self.url)?;
        let tls = self.tls(&target, &runtime).await?;
        let id = client_id();
        let reply_topic = format!("ulo/rpc/reply/{id}");
        let (client, eventloop) = AsyncClient::new(target.options(id.clone(), tls), 64);
        let side = Arc::new(ClientSide {
            client,
            codec: self.codec,
            qos: self.qos.wire(),
            runtime: runtime.clone(),
            id,
            reply_topic,
            max_packet: Arc::clone(&self.max_packet),
            calls: Mutex::new(HashMap::new()),
            order: tokio::sync::Mutex::new(()),
            outgoing: Mutex::new(None),
            pkids: Mutex::new(HashMap::new()),
            closed: Mutex::new(false),
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
        let (frames, replies) = mpsc::unbounded_channel();
        let (ready, subscribed) = oneshot::channel();
        // The clock of the runtime the client was handed, the app's inside an app.
        let timer = Arc::clone(handed) as Arc<dyn Timer>;
        let looped = Arc::clone(&side);
        let event_loop = Loop::spawn(&runtime, timer, move |written| client_loop(looped, eventloop, frames, ready, written));
        // Until the broker answers, a `connect` dropped mid-handshake, a call's timeout cutting it
        // short, takes the event loop with it.
        let pending = AbortOnDrop::new(event_loop.task.abort_handle());
        // The reply subscription is in place before the first request can be published.
        let outcome = subscribed.await.unwrap_or_else(|_| Err("the MQTT link's event loop ended before the broker answered".into()));
        outcome?;
        pending.disarm();
        lock(&self.state).client = Some((Arc::clone(&side), event_loop));

        Ok(Outbound {
            send: Box::new(move |pattern: Pattern, frame: Frame, _reply_to: Option<ReplyTo>| -> BoxFuture<'static, Result<(), BoxError>> {
                let side = Arc::clone(&side);
                Box::pin(async move { side.send(pattern, frame).await })
            }),
            replies: receiver_stream(replies),
        })
    }

    async fn drain(&self) {
        let server = lock(&self.state).server.as_ref().map(|(side, _)| Arc::clone(side));
        let Some(side) = server else { return };
        side.phase.send_replace(Phase::Draining);
        for filter in &side.shared {
            side.unsubscribe_queued(filter);
            if let Err(error) = side.client.unsubscribe(filter.clone()).await {
                side.unsubscribe_refused();
                tracing::warn!(%error, filter, "the MQTT link could not unsubscribe");
            }
        }
        // The broker routes requests here until it has processed each UNSUBSCRIBE, and writes
        // every PUBLISH it routed before that ahead of the UNSUBACK, which the event loop reads in
        // order: once every UNSUBACK is in, nothing more is routed here and what was routed has
        // been delivered. MQTT 5 §3.10.4 lets a broker deliver after the UNSUBACK a publish it held
        // back for this instance's flow control, which only more than `RECEIVE_MAXIMUM`
        // unacknowledged publishes, or a smaller cap the broker applies, would leave it holding.
        // No bound of its own: the core drops this future at the drain's deadline.
        let unconfirmed = Unconfirmed(Some(Arc::clone(&side)));
        let _ = side.unconfirmed.subscribe().wait_for(|filters| *filters == 0).await;
        unconfirmed.disarm();
        // The side is held weakly: its drop clears the table this waits on, and a watcher holding
        // it would keep it and the table after the link's drop.
        let (mut count, watched) = (side.calls.count.subscribe(), Arc::downgrade(&side));
        side.runtime.handle().spawn(async move {
            while count.wait_for(|held| *held == 0).await.is_ok() {
                let Some(side) = watched.upgrade() else { return };
                if side.end_if_idle() {
                    return;
                }
            }
        });
    }

    async fn close(&self) -> Result<(), BoxError> {
        let (server, client) = {
            let mut state = lock(&self.state);
            (state.server.take(), state.client.take())
        };
        if let Some((side, event_loop)) = server {
            side.phase.send_replace(Phase::Closed);
            // Taken before the table is cleared, so no call is held once it is, and before the
            // replies close, so every call held is one they answer.
            lock(&side.deliveries).take();
            side.calls.clear();
            let unsent = side.replies.close(&side.client, side.codec, side.qos);
            shut(event_loop, &side.runtime, side.queue_unsent(unsent)).await;
        }
        if let Some((side, event_loop)) = client {
            // Under the lock every publish queues under, so none is queued behind the DISCONNECT.
            let queued = {
                let mut closed = lock(&side.closed);
                *closed = true;
                side.client.try_disconnect().is_ok()
            };
            shut(event_loop, &side.runtime, async {
                if !queued {
                    let _ = side.client.disconnect().await;
                }
            })
            .await;
        }
        Ok(())
    }
}

/// Gives the connection's event loop a turn on the calling thread, then runs `rest`, which queues
/// what `close` could not queue at once, and waits for the loop to report the DISCONNECT written,
/// or for the connection to fail after `close`. The loop writes requests in the order they were
/// queued, so every one queued earlier is written ahead of the DISCONNECT: a reply or a refusal on
/// the server side, an event or a `cancel` on the client side. `close` queues what fits before
/// calling this, so the turn writes it in `close`'s first poll, which is all a `close` under a
/// spent shutdown cap gets.
///
/// The loop outlives this future, completed or dropped, by up to [`CUT_GRACE`], reading until the
/// broker closes the connection.
async fn shut(event_loop: Loop, runtime: &Tokio, rest: impl Future<Output = ()>) {
    let Loop { turns, written, released, .. } = event_loop;
    let _released = released;
    {
        // The loop's timers and sockets belong to the link's runtime, whichever thread polls it.
        let _runtime = runtime.handle().enter();
        // Given before `rest` or after it, the turn leaves what the broker reads unchanged.
        turns.give();
    }
    rest.await;
    let _ = written.await;
}

/// Logs, when the drain's future is dropped before the broker confirmed every UNSUBSCRIBE, the
/// filters it had not confirmed. The core drops that future at the drain's deadline; the broker
/// can then still route requests here, and those reaching the event loop after `close` queued its
/// DISCONNECT go unanswered.
struct Unconfirmed(Option<Arc<ServerSide>>);

impl Unconfirmed {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for Unconfirmed {
    fn drop(&mut self) {
        let Some(side) = self.0.take() else { return };
        let filters = side.unconfirmed_filters();
        if !filters.is_empty() {
            tracing::warn!(
                ?filters,
                "the drain's deadline passed before the MQTT broker confirmed the unsubscribe; until it does it can route requests to this instance"
            );
        }
    }
}

/// Aborts a task when dropped, unless disarmed first.
struct AbortOnDrop(Option<AbortHandle>);

impl AbortOnDrop {
    fn new(task: AbortHandle) -> Self {
        AbortOnDrop(Some(task))
    }

    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// Where the link connects, parsed from its url.
struct Target {
    host: String,
    port: u16,
    tls: bool,
    credentials: Option<(String, String)>,
}

impl Target {
    fn parse(url: &str) -> Result<Target, BoxError> {
        let invalid = |why: &str| -> BoxError { format!("the MQTT link's url does not parse: {why}").into() };
        let (scheme, rest) = url.split_once("://").ok_or_else(|| invalid("no scheme, expected `mqtt://` or `mqtts://`"))?;
        let (tls, default_port) = match scheme {
            "mqtt" | "tcp" => (false, 1883),
            "mqtts" | "ssl" => (true, 8883),
            other => return Err(invalid(&format!("the scheme `{other}` is not `mqtt` or `mqtts`"))),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (credentials, address) = match authority.rsplit_once('@') {
            Some((credentials, address)) => {
                let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
                (Some((user.to_owned(), password.to_owned())), address)
            }
            None => (None, authority),
        };
        let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
            let (host, after) = bracketed.split_once(']').ok_or_else(|| invalid("an unclosed `[` in the host"))?;
            let port = match after.strip_prefix(':') {
                Some(port) => port.parse().map_err(|_| invalid("the port is not a number"))?,
                None if after.is_empty() => default_port,
                None => return Err(invalid("text after the host")),
            };
            (host.to_owned(), port)
        } else {
            match address.rsplit_once(':') {
                Some((host, port)) => (host.to_owned(), port.parse().map_err(|_| invalid("the port is not a number"))?),
                None => (address.to_owned(), default_port),
            }
        };
        if host.is_empty() {
            return Err(invalid("no host"));
        }
        Ok(Target { host, port, tls, credentials })
    }

    fn options(&self, client_id: String, tls: Option<TlsConfiguration>) -> MqttOptions {
        let mut options = MqttOptions::new(client_id, self.host.clone(), self.port);
        options.set_keep_alive(Duration::from_secs(10));
        options.set_max_packet_size(Some(MAX_INCOMING));
        options.set_receive_maximum(Some(RECEIVE_MAXIMUM));
        if let Some((user, password)) = &self.credentials {
            options.set_credentials(user.clone(), password.clone());
        }
        if let Some(tls) = tls {
            options.set_transport(Transport::tls_with_config(tls));
        }
        options
    }
}

/// The platform's root certificates, as the TLS configuration an `mqtts://` link connects with,
/// loaded at the link's first need, a server's `prepare` or a client's first connect, and kept for
/// every connection after. The load reads the platform's trust store, 71 ms warm and 2.3 s cold
/// on macOS, so it runs on the link's runtime's blocking pool: on the thread polling the connect it
/// would stall every task scheduled there, and rumqttc's own default configuration reads the store
/// on every connection. A load that fails is kept too, since no retry mends a store that cannot be
/// read or holds no root: a server's `prepare` refuses it, and every client call answers it as
/// [`Misconfigured`].
pub(crate) struct Roots {
    /// `rustls_native_certs::load_native_certs`, which a test replaces.
    load: fn() -> CertificateResult,
    loaded: OnceCell<Result<TlsConfiguration, String>>,
}

impl Roots {
    fn platform() -> Roots {
        Roots { load: rustls_native_certs::load_native_certs, loaded: OnceCell::new() }
    }

    async fn load(&self, runtime: &Tokio) -> Result<TlsConfiguration, BoxError> {
        let load = self.load;
        let loaded = self
            .loaded
            .get_or_try_init(|| async move {
                match runtime.handle().spawn_blocking(move || configure(load())).await {
                    Ok(configured) => Ok(configured),
                    Err(ended) if ended.is_panic() => Ok(Err(format!("loading them panicked: {}", panic_text(&*ended.into_panic())))),
                    Err(ended) => Err(ended),
                }
            })
            .await;
        match loaded {
            Ok(Ok(tls)) => Ok(tls.clone()),
            Ok(Err(cause)) => Err(Misconfigured {
                source: format!("the MQTT link could not load the platform's root certificates for `mqtts://`: {cause}").into(),
            }
            .into()),
            // Not kept: the next connect, on a runtime still running, loads them again.
            Err(_) => Err("the MQTT link's runtime shut down while it loaded the platform's root certificates".into()),
        }
    }
}

/// The TLS configuration trusting what `loaded` yielded, refused when it yielded no root: a store
/// that cannot be read and one that holds none would each fail every handshake. A load yielding
/// roots beside errors keeps the roots and logs the errors at `warn`. The crypto provider is the
/// one the process installed, or `aws-lc-rs`, rumqttc's own, where none is: rustls's builder would
/// panic in a build linking a second provider. Its random source is drawn on once here, which
/// initialises the provider's library off the worker: left to the first handshake, aws-lc-rs's
/// initialisation held the thread polling it for about 20 ms in a debug build.
fn configure(loaded: CertificateResult) -> Result<TlsConfiguration, String> {
    let mut roots = RootCertStore::empty();
    let (added, unparsable) = roots.add_parsable_certificates(loaded.certs);
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
            "the MQTT link loaded some of the platform's root certificates and could not load others"
        );
    }
    let provider = CryptoProvider::get_default().cloned().unwrap_or_else(|| Arc::new(aws_lc_rs::default_provider()));
    provider.secure_random.fill(&mut [0u8; 1]).map_err(|_| "the crypto provider's random source failed".to_owned())?;
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("the crypto provider offers neither TLS 1.2 nor 1.3: {error}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConfiguration::Rustls(Arc::new(config)))
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "a payload that is not text".to_owned())
}

/// The server side of a bound link. The event loop owns the connection; replies are queued on
/// `client` from whichever task answers.
struct ServerSide {
    client: AsyncClient,
    codec: Codec,
    qos: MqttQoS,
    /// The link's runtime, which the drain's watcher is spawned on.
    runtime: Tokio,
    /// The `$share/<group>/<pattern>` filters, unsubscribed by the drain.
    shared: Vec<String>,
    calls: Arc<Calls>,
    replies: Arc<Replies>,
    phase: watch::Sender<Phase>,
    max_packet: Arc<AtomicU64>,
    /// Taken once draining holds no call, or at close, which ends the inbound stream. A call is
    /// held or released, and its frame delivered, under this lock, so the drain cannot end the
    /// stream between the two: a `cancel` releasing the last held call is delivered first.
    deliveries: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
    /// The drain's UNSUBSCRIBEs the broker has not acknowledged.
    unsubscribing: Mutex<Unsubscribing>,
    /// How many filters `unsubscribing` holds, which the drain waits to reach zero.
    unconfirmed: watch::Sender<usize>,
}

/// The drain's UNSUBSCRIBEs, from the request rumqttc queues to the broker's UNSUBACK. The server
/// side unsubscribes nowhere else, so every `Outgoing::Unsubscribe` its event loop reports is the
/// drain's, and rumqttc takes requests in the order they were queued.
#[derive(Default)]
struct Unsubscribing {
    /// Filters queued, oldest first, before the event loop has given them a packet id.
    queued: VecDeque<String>,
    /// Filters by the packet id of the UNSUBSCRIBE carrying them, awaiting its UNSUBACK.
    sent: HashMap<u16, String>,
}

impl Unsubscribing {
    fn len(&self) -> usize {
        self.queued.len() + self.sent.len()
    }
}

/// A held call's reply path holds the table it is held in and the event loop's request channel,
/// so a side dropped with calls held, its link dropped without `close`, would keep them after the
/// link's drop has aborted the event loop (F429).
impl Drop for ServerSide {
    fn drop(&mut self) {
        self.calls.clear();
    }
}

impl ServerSide {
    fn unsubscribing(&self, change: impl FnOnce(&mut Unsubscribing)) {
        let mut unsubscribing = lock(&self.unsubscribing);
        change(&mut unsubscribing);
        self.unconfirmed.send_replace(unsubscribing.len());
    }

    /// Before the drain queues the UNSUBSCRIBE for `filter`, so the event loop's report of it
    /// finds the filter.
    fn unsubscribe_queued(&self, filter: &str) {
        self.unsubscribing(|unsubscribing| unsubscribing.queued.push_back(filter.to_owned()));
    }

    /// The request for the filter queued last never reached the event loop.
    fn unsubscribe_refused(&self) {
        self.unsubscribing(|unsubscribing| {
            unsubscribing.queued.pop_back();
        });
    }

    /// The event loop gave the oldest queued UNSUBSCRIBE packet id `pkid`.
    fn unsubscribe_sent(&self, pkid: u16) {
        self.unsubscribing(|unsubscribing| {
            if let Some(filter) = unsubscribing.queued.pop_front() {
                unsubscribing.sent.insert(pkid, filter);
            }
        });
    }

    /// The broker acknowledged the UNSUBSCRIBE with `unsuback.pkid`. A refusal leaves the broker
    /// routing to this instance, which nothing can change before close; it is logged.
    fn unsubscribe_acknowledged(&self, unsuback: &UnsubAck) {
        self.unsubscribing(|unsubscribing| {
            let Some(filter) = unsubscribing.sent.remove(&unsuback.pkid) else { return };
            if let Some(reason) = unsuback.reasons.iter().find(|reason| !matches!(reason, UnsubAckReason::Success | UnsubAckReason::NoSubscriptionExisted)) {
                tracing::warn!(filter, ?reason, "the MQTT broker refused to unsubscribe the draining server; it can still route requests to this instance");
            }
        });
    }

    /// A CONNACK without a session present: the broker holds no subscription of this client's,
    /// so nothing the drain waits for is routed here any more.
    fn session_ended(&self) {
        self.unsubscribing(|unsubscribing| {
            unsubscribing.queued.clear();
            unsubscribing.sent.clear();
        });
    }

    fn unconfirmed_filters(&self) -> Vec<String> {
        let unsubscribing = lock(&self.unsubscribing);
        let mut filters: Vec<String> = unsubscribing.sent.values().chain(unsubscribing.queued.iter()).cloned().collect();
        filters.sort();
        filters
    }

    /// Ends the inbound stream if no call is held, answering whether it has ended.
    fn end_if_idle(&self) -> bool {
        let mut deliveries = lock(&self.deliveries);
        if !self.calls.is_empty() {
            return false;
        }
        deliveries.take();
        true
    }

    /// Answers a request or streamed request that arrived once the inbound stream ended `err` of
    /// kind `unavailable`, as the server answers one during the drain. The broker routes a request
    /// to this instance until it has processed the drain's UNSUBSCRIBE, which rumqttc queues
    /// behind the replies already waiting, and dropping it would leave the caller to its own
    /// `Timeout`. Queued without waiting: this runs on the event loop, which drains the queue.
    fn refuse(&self, reply: String, correlation: Option<Bytes>, pattern: &str) {
        let bytes = match self.codec.encode_frame(&shutting_down(0)) {
            Ok(bytes) => bytes,
            Err(error) => return tracing::warn!(%error, pattern, "the MQTT link could not refuse a request that arrived after the drain"),
        };
        match self.publish_unowned(reply, bytes, reply_properties(correlation, None)) {
            Ok(()) => {}
            // MQTT allows no packet after the DISCONNECT `close` queued.
            Err(Unowned::Closed) => tracing::debug!(pattern, "the MQTT link did not refuse a request that arrived after `close`"),
            Err(Unowned::Refused(error)) => {
                tracing::warn!(%error, pattern, "the MQTT link could not refuse a request that arrived after the drain");
            }
        }
    }

    /// Queues a reply no held call owns, a refusal or an `opened`, under the lock `close` takes,
    /// so it is queued ahead of the DISCONNECT or not at all. Queued without waiting: this runs on
    /// the event loop, which is what drains the queue.
    fn publish_unowned(&self, topic: String, bytes: Bytes, properties: PublishProperties) -> Result<(), Unowned> {
        let answers = lock(&self.replies.answers);
        if answers.closed {
            return Err(Unowned::Closed);
        }
        self.client.try_publish_with_properties(topic, self.qos, false, bytes, properties).map_err(|error| Unowned::Refused(error.into()))
    }

    fn on_publish(&self, publish: Publish) {
        let topic = String::from_utf8_lossy(&publish.topic).into_owned();
        let properties = publish.properties.unwrap_or_default();
        let kind = kind_of(&properties.user_properties);
        let key = properties
            .correlation_data
            .as_ref()
            .map(|data| String::from_utf8_lossy(data).into_owned())
            .or_else(|| properties.response_topic.clone());
        let deliveries = lock(&self.deliveries);
        if topic == CONTROL {
            let Some(key) = key else { return };
            let Some(sender) = deliveries.as_ref() else { return };
            let frame = match kind.as_deref() {
                Some(IN) => self.calls.get(&key).map(|(id, path)| (Frame::In { id, data: Data::new(publish.payload) }, path)),
                Some(IN_END) => self.calls.get(&key).map(|(id, path)| (Frame::InEnd { id }, path)),
                Some(CANCEL) => self.release(&key).map(|(id, path)| (Frame::Cancel { id }, path)),
                _ => None,
            };
            if let Some((frame, path)) = frame {
                let _ = sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() });
            }
            return;
        }

        let headers = call_headers(&properties.user_properties);
        let reply = properties.response_topic.clone().map(|topic| (topic, properties.correlation_data.clone()));
        match (reply, kind.as_deref()) {
            (None, None) => {
                let frame = Frame::Evt { pattern: topic, headers, data: Data::new(publish.payload) };
                if let Some(sender) = deliveries.as_ref() {
                    let _ = sender.send(Delivery { frame, reply: None, ack: Ack::none() });
                }
            }
            (Some((reply, correlation)), None) => {
                let key = key.unwrap_or_else(|| reply.clone());
                if let Some(sender) = deliveries.as_ref() {
                    let (id, path) = self.hold(key.clone(), reply.clone(), correlation.clone());
                    let frame = Frame::Req { id, pattern: topic.clone(), headers, data: Data::new(publish.payload) };
                    if sender.send(Delivery { frame, reply: Some(path), ack: Ack::none() }).is_ok() {
                        return;
                    }
                    self.release(&key);
                }
                drop(deliveries);
                self.refuse(reply, correlation, &topic);
            }
            (Some((reply, correlation)), Some(OPEN)) => {
                // Checked before `opened` is sent, so the caller is not told `opened` for a call
                // the server never receives.
                let Some(sender) = deliveries.as_ref() else {
                    drop(deliveries);
                    return self.refuse(reply, correlation, &topic);
                };
                let key = key.unwrap_or_else(|| reply.clone());
                let (id, path) = self.hold(key, reply.clone(), correlation.clone());
                // The caller holds the request's items until this arrives. Queued without waiting:
                // this runs on the event loop, which is what drains the queue. `publish_unowned`'s
                // `closed` check cannot refuse it: this holds the `deliveries` guard, and `close`
                // takes `deliveries` before it sets `closed`, so `deliveries` found `Some` means
                // `closed` is unset. The check is kept for a change to that lock order.
                let opened = reply_properties(correlation, Some(OPENED));
                if let Err(Unowned::Refused(error)) = self.publish_unowned(reply, Bytes::new(), opened) {
                    tracing::warn!(%error, pattern = topic, "the MQTT link could not acknowledge a streamed request");
                }
                let _ = sender.send(Delivery { frame: Frame::Open { id, pattern: topic, headers }, reply: Some(path), ack: Ack::none() });
            }
            (_, Some(kind)) => {
                tracing::warn!(pattern = topic, kind, "the MQTT link dropped a request-lane message of an unknown kind");
            }
        }
    }

    /// Holds a call under `key`, its Correlation Data, the key its control messages carry.
    fn hold(&self, key: String, reply: String, correlation: Option<Bytes>) -> (u64, ReplyPath) {
        let codec = self.codec;
        let to = Addressee { topic: reply, correlation };
        let lane = Lane {
            client: self.client.clone(),
            qos: self.qos,
            calls: Arc::clone(&self.calls),
            replies: Arc::clone(&self.replies),
            key: key.clone(),
            to: to.clone(),
        };
        let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
            // Queued here rather than in the future, so the frame's place against the DISCONNECT
            // is fixed when the send is called, not when its future is first polled.
            match codec.encode_frame(&frame) {
                Ok(bytes) => lane.send(bytes, is_terminal(&frame)),
                Err(error) => Box::pin(std::future::ready(Err(BoxError::from(error)))),
            }
        });
        let id = self.calls.hold(key.clone(), path.clone());
        lock(&self.replies.answers).unanswered.insert(key, Unanswered { id, to });
        (id, path)
    }

    /// Lets go of the call held under `key`, whose caller cancelled it or which the core never
    /// received: the server writes it nothing more, and `close` answers it nothing.
    fn release(&self, key: &str) -> Option<(u64, ReplyPath)> {
        let call = self.calls.release(key);
        lock(&self.replies.answers).unanswered.remove(key);
        call
    }

    /// Queues, in order, what `close` found the channel too full for, each waiting for room. No
    /// other request reaches the channel meanwhile: every reply path, refusal and `opened` finds
    /// the replies closed.
    async fn queue_unsent(&self, unsent: Vec<Unsent>) {
        for request in unsent {
            let queued = match request {
                Unsent::Reply { topic, bytes, properties } => self.client.publish_with_properties(topic, self.qos, false, bytes, properties).await,
                Unsent::Disconnect => self.client.disconnect().await,
            };
            if let Err(error) = queued {
                tracing::debug!(%error, "the MQTT link could not queue what `close` writes");
            }
        }
    }
}

/// Why [`ServerSide::publish_unowned`] queued nothing.
enum Unowned {
    Closed,
    Refused(BoxError),
}

/// A request `close` found the channel full for.
enum Unsent {
    Reply { topic: String, bytes: Bytes, properties: PublishProperties },
    Disconnect,
}

/// Where a held call's replies go: its Response Topic and Correlation Data.
#[derive(Clone)]
struct Addressee {
    topic: String,
    correlation: Option<Bytes>,
}

/// The lock every reply a server sends and its `close` take, so a reply and the DISCONNECT are
/// queued in one order. rumqttc's request channel is the server side's one queue, and MQTT allows
/// no packet after a DISCONNECT: a reply queued behind it is never written. Every queueing under
/// the lock is a `try_` call, so nothing waits while holding it; a reply finding the channel full
/// waits for room after letting go of it.
#[derive(Default)]
struct Replies {
    answers: Mutex<Answers>,
    /// Notified each time the event loop returns from a poll, which is when it may have taken a
    /// request off the channel, and once as it ends. A reply waiting for room waits here rather
    /// than in rumqttc's own `publish`: flume moves a waiting send into the channel at the
    /// receiver's next take, with no further poll of the send, so a reply waiting there when
    /// `close` ran would be queued behind the DISCONNECT.
    room: Notify,
    /// Set once the event loop has ended, after which the channel never has room.
    ended: AtomicBool,
    /// Run once by a reply waiting for room, after a failed attempt and before its wait.
    #[cfg(test)]
    between: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[derive(Default)]
struct Answers {
    /// Set by `close` once it has queued the DISCONNECT: a reply sent after it is dropped.
    closed: bool,
    /// Each held call whose terminal frame has not been queued, by the key it is held under in
    /// [`Calls`].
    unanswered: HashMap<String, Unanswered>,
}

struct Unanswered {
    /// The call's local id, which the `err` `close` writes for it carries.
    id: u64,
    to: Addressee,
}

/// One held call's way to its caller, which its `ReplyPath` sends through.
#[derive(Clone)]
struct Lane {
    client: AsyncClient,
    qos: MqttQoS,
    calls: Arc<Calls>,
    replies: Arc<Replies>,
    /// The key the call is held under in `calls` and in `replies`.
    key: String,
    to: Addressee,
}

impl Lane {
    /// Queues one reply frame, unless `close` has queued the DISCONNECT. A terminal frame queued
    /// ends the call: its control messages find no call, and `close` writes it nothing. A frame
    /// finding the request channel full waits for room in the future answered, the lock let go,
    /// so a streamed reply is paced by the broker rather than cut at the channel's capacity. The
    /// call stays unanswered while the frame waits, so a `close` meanwhile answers it, and the
    /// frame, retried under the lock, then finds the replies closed and is dropped. A channel
    /// whose event loop has ended takes nothing, and the call ends.
    fn send(&self, bytes: Bytes, terminal: bool) -> BoxFuture<'static, Result<(), BoxError>> {
        let properties = reply_properties(self.to.correlation.clone(), None);
        if let Some(outcome) = self.try_queue(&bytes, &properties, terminal) {
            return Box::pin(std::future::ready(outcome));
        }
        let lane = self.clone();
        Box::pin(async move {
            loop {
                // Made before the attempt, so a take between a failed attempt and the wait wakes it.
                // `enable()` is needed only if a `notify_one` is ever added; `notify_waiters`
                // reaches a `Notified` from its creation.
                let mut room = pin!(lane.replies.room.notified());
                room.as_mut().enable();
                if let Some(outcome) = lane.try_queue(&bytes, &properties, terminal) {
                    return outcome;
                }
                #[cfg(test)]
                lane.replies.between_attempt_and_wait();
                room.await;
            }
        })
    }

    /// Queues the frame under the lock, `None` when the channel has no room. rumqttc's
    /// `TryRequest` does not tell a full channel from one whose loop has ended, which `ended` does.
    fn try_queue(&self, bytes: &Bytes, properties: &PublishProperties, terminal: bool) -> Option<Result<(), BoxError>> {
        let mut answers = lock(&self.replies.answers);
        if answers.closed {
            return Some(Err("the MQTT link has closed; the reply was not sent".into()));
        }
        if self.client.try_publish_with_properties(self.to.topic.clone(), self.qos, false, bytes.clone(), properties.clone()).is_ok() {
            if terminal {
                self.answered(&mut answers);
            }
            return Some(Ok(()));
        }
        if self.replies.ended.load(Ordering::Acquire) {
            self.answered(&mut answers);
            return Some(Err("the MQTT link's connection has ended; the reply was not sent".into()));
        }
        None
    }

    /// The call is sent its last frame: its control messages find no call, and `close` writes it
    /// nothing.
    fn answered(&self, answers: &mut Answers) {
        answers.unanswered.remove(&self.key);
        self.calls.release(&self.key);
    }
}

impl Replies {
    #[cfg(test)]
    fn between_attempt_and_wait(&self) {
        let between = lock(&self.between).take();
        if let Some(between) = between {
            between();
        }
    }

    /// Queues, in one hold of the lock, the `err` of kind `unavailable` with the shutdown phrase
    /// for every held call no terminal frame was queued for, a stream's after the items queued
    /// before it, then the DISCONNECT; and marks the replies closed, so a call's own answer
    /// arriving after it is dropped rather than queued behind the DISCONNECT. Nothing here waits:
    /// what the channel has no room for is answered in order for `close` to queue once there is
    /// room.
    fn close(&self, client: &AsyncClient, codec: Codec, qos: MqttQoS) -> Vec<Unsent> {
        let mut answers = lock(&self.answers);
        answers.closed = true;
        let mut unsent = Vec::new();
        for (_, Unanswered { id, to }) in answers.unanswered.drain() {
            let bytes = match codec.encode_frame(&shutting_down(id)) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::debug!(%error, "the MQTT link could not encode the shutdown answer of a call it held at `close`");
                    continue;
                }
            };
            let properties = reply_properties(to.correlation, None);
            // Tried even after an earlier answer found no room: stopping at the first would change
            // only the order across calls, which nothing promises, so that break is unpinned.
            if client.try_publish_with_properties(to.topic.clone(), qos, false, bytes.clone(), properties.clone()).is_ok() {
                continue;
            }
            unsent.push(Unsent::Reply { topic: to.topic, bytes, properties });
        }
        if !unsent.is_empty() || client.try_disconnect().is_err() {
            unsent.push(Unsent::Disconnect);
        }
        unsent
    }
}

/// Marks the replies' event loop ended as it drops, and wakes every reply waiting for room.
struct LoopEnded(Arc<Replies>);

impl Drop for LoopEnded {
    fn drop(&mut self) {
        self.0.ended.store(true, Ordering::Release);
        self.0.room.notify_waiters();
    }
}

/// Polls the server connection until it fails after `close`, which, once the DISCONNECT `close`
/// queued is written, is the broker closing it, or until [`linger`]'s grace drops it. On every
/// CONNACK it subscribes again, rumqttc keeping no subscription across a reconnect: the shared
/// filters while serving, the control topic always.
async fn serve(side: Arc<ServerSide>, mut eventloop: EventLoop, ready: oneshot::Sender<Result<(), BoxError>>, mut written: Written) {
    let _ended = LoopEnded(Arc::clone(&side.replies));
    let mut ready = Some(ready);
    loop {
        let polled = eventloop.poll().await;
        side.replies.room.notify_waiters();
        match polled {
            Ok(Event::Incoming(Packet::ConnAck(connack))) => {
                if !connack.session_present {
                    side.session_ended();
                }
                let properties = connack.properties.as_ref();
                if let Some(max) = properties.and_then(|properties| properties.max_packet_size) {
                    side.max_packet.store(u64::from(max), Ordering::Relaxed);
                }
                if properties.and_then(|properties| properties.shared_subscription_available) == Some(0) {
                    let error = "the MQTT broker's CONNACK announces no shared-subscription support, so the link's \
                                 `Competing` delivery through `$share/<group>/` (the `Mqtt::group` setting) would be false";
                    match ready.take() {
                        Some(ready) => {
                            let _ = ready.send(Err(error.into()));
                            return;
                        }
                        None => tracing::error!("{error}; after a reconnect the link stays subscribed to the control topic alone"),
                    }
                    let _ = side.client.try_subscribe(CONTROL, side.qos);
                    continue;
                }
                let serving = *side.phase.borrow() == Phase::Serving;
                let mut filters: Vec<Filter> = Vec::new();
                if serving {
                    filters.extend(side.shared.iter().map(|filter| Filter::new(filter.clone(), side.qos)));
                }
                filters.push(Filter::new(CONTROL, side.qos));
                if let Err(error) = side.client.try_subscribe_many(filters)
                    && let Some(ready) = ready.take()
                {
                    let _ = ready.send(Err(format!("the MQTT link could not subscribe: {error}").into()));
                    return;
                }
            }
            Ok(Event::Incoming(Packet::SubAck(suback))) => {
                let Some(pending) = ready.take() else { continue };
                let refused = suback.return_codes.iter().enumerate().find_map(|(index, code)| match code {
                    SubscribeReasonCode::Success(_) => None,
                    code => Some((side.shared.get(index).map_or(CONTROL, String::as_str), *code)),
                });
                match refused {
                    None => {
                        let _ = pending.send(Ok(()));
                    }
                    Some((filter, code)) => {
                        let _ = pending.send(Err(format!("the MQTT broker refused the subscription `{filter}`: {code:?}").into()));
                        return;
                    }
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => side.on_publish(publish),
            Ok(Event::Outgoing(Outgoing::Unsubscribe(pkid))) => side.unsubscribe_sent(pkid),
            Ok(Event::Incoming(Packet::UnsubAck(unsuback))) => side.unsubscribe_acknowledged(&unsuback),
            // rumqttc also writes a DISCONNECT of its own on a protocol error, after which the
            // broker closes the connection and the loop reconnects.
            Ok(Event::Outgoing(Outgoing::Disconnect)) if *side.phase.borrow() == Phase::Closed => written.disconnect(&mut eventloop),
            Ok(_) => {}
            Err(error) => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(format!("the MQTT link could not connect: {error}").into()));
                    return;
                }
                if *side.phase.borrow() == Phase::Closed {
                    tracing::debug!(%error, "the MQTT link's connection ended after `close`");
                    return;
                }
                tracing::warn!(%error, "the MQTT link lost its connection; reconnecting");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
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

    fn is_empty(&self) -> bool {
        lock(&self.held).is_empty()
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

/// The client side: one reply topic for every reply, each call's Correlation Data `<id>.<call>`.
struct ClientSide {
    client: AsyncClient,
    codec: Codec,
    qos: MqttQoS,
    /// The link's runtime, which a streamed request's pump is spawned on.
    runtime: Tokio,
    id: String,
    reply_topic: String,
    max_packet: Arc<AtomicU64>,
    calls: Mutex<HashMap<u64, ClientCall>>,
    /// Held across one publish and its `Outgoing::Publish` event, so the event names that
    /// publish's packet id: rumqttc's `publish` returns before the id is assigned.
    order: tokio::sync::Mutex<()>,
    /// The publish waiting for its packet id, which is sent `None` when the connection fails
    /// first.
    outgoing: Mutex<Option<Waiting>>,
    /// The packet id of each request publish awaiting its PUBACK or PUBREC, and its call. The
    /// event loop writes the entry on the publish's `Outgoing::Publish` event: rumqttc has written
    /// the packet by then, so the PUBACK can be read before the publishing task runs again, and
    /// an entry written there would miss it and lose the miss signal.
    pkids: Mutex<HashMap<u16, u64>>,
    /// Set by `close` as it queues the DISCONNECT, under this lock, which every publish queues
    /// under: MQTT allows no packet after the DISCONNECT, and the event loop, polled past it,
    /// would write one queued behind it.
    closed: Mutex<bool>,
    #[cfg(feature = "conformance-probe")]
    probe: Arc<ClientProbe>,
}

/// A publish waiting for its `Outgoing::Publish` event.
struct Waiting {
    /// The request's call, `None` for an event or a control message.
    call: Option<u64>,
    assigned: oneshot::Sender<Option<u16>>,
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
                self.fits(pattern.as_str(), data.len())?;
                lock(&self.calls).insert(id, ClientCall::Unary { pattern: pattern.clone() });
                let properties = self.request_properties(id, &headers, None);
                let sent = self.publish(pattern.to_string(), properties, data.into_bytes(), Some(id)).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::Evt { headers, data, .. } => {
                self.fits(pattern.as_str(), data.len())?;
                let properties = PublishProperties { user_properties: user_properties(&headers, None), ..Default::default() };
                self.publish(pattern.to_string(), properties, data.into_bytes(), None).await
            }
            Frame::Open { id, headers, .. } => {
                let (gate, opened) = oneshot::channel();
                let (queue, queued) = mpsc::unbounded_channel();
                lock(&self.calls).insert(id, ClientCall::Streaming { pattern: pattern.clone(), gate: Some(gate), queue, until: HoldUntil::at_open(&headers) });
                self.runtime.handle().spawn(pump(Arc::clone(self), id, opened, queued));
                let properties = self.request_properties(id, &headers, Some(OPEN));
                let sent = self.publish(pattern.to_string(), properties, Bytes::new(), Some(id)).await;
                if sent.is_err() {
                    lock(&self.calls).remove(&id);
                }
                sent
            }
            Frame::In { id, data } => {
                self.fits(CONTROL, data.len())?;
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
                if unary { self.publish_control(id, CANCEL, Bytes::new()).await } else { Ok(()) }
            }
            other => Err(format!("a client does not send a `{}` frame", other.kind()).into()),
        }
    }

    fn correlation(&self, call: u64) -> Bytes {
        Bytes::from(format!("{}.{call}", self.id))
    }

    fn call_of(&self, correlation: &[u8]) -> Option<u64> {
        let text = std::str::from_utf8(correlation).ok()?;
        text.strip_prefix(self.id.as_str())?.strip_prefix('.')?.parse().ok()
    }

    fn request_properties(&self, call: u64, headers: &CallHeaders, kind: Option<&str>) -> PublishProperties {
        PublishProperties {
            response_topic: Some(self.reply_topic.clone()),
            correlation_data: Some(self.correlation(call)),
            user_properties: user_properties(headers, kind),
            ..Default::default()
        }
    }

    /// A packet's size beside the broker's Maximum Packet Size: the body, the topic and a margin
    /// for the fixed header and properties.
    fn fits(&self, topic: &str, body: usize) -> Result<(), BoxError> {
        let limit = self.max_packet.load(Ordering::Relaxed);
        let size = (body + topic.len() + self.reply_topic.len() + self.id.len() + 64) as u64;
        if limit > 0 && size > limit {
            return Err(Box::new(FrameTooLarge { size: body as u64, limit }));
        }
        Ok(())
    }

    /// Queues the publish without waiting for room: `order` lets the next publish in only once the
    /// event loop has taken this one, so the channel holds at most one, and `try_publish` refuses
    /// only once the loop has ended.
    async fn publish(&self, topic: String, properties: PublishProperties, body: Bytes, call: Option<u64>) -> Result<(), BoxError> {
        let _order = self.order.lock().await;
        let (assigned, pkid) = oneshot::channel();
        *lock(&self.outgoing) = Some(Waiting { call, assigned });
        let queued = {
            // A break that checks `closed` outside this lock shows only as a race, and is unpinned.
            let closed = lock(&self.closed);
            if *closed {
                Err("the MQTT link has closed; the publish was not sent".into())
            } else {
                self.client
                    .try_publish_with_properties(topic, self.qos, false, body, properties)
                    .map_err(|error| BoxError::from(format!("the MQTT link could not queue a publish: {error}")))
            }
        };
        if let Err(error) = queued {
            lock(&self.outgoing).take();
            return Err(error);
        }
        match pkid.await {
            Ok(Some(_)) => Ok(()),
            _ => Err("the MQTT link's connection ended before the publish went out".into()),
        }
    }

    async fn publish_control(&self, call: u64, kind: &str, body: Bytes) -> Result<(), BoxError> {
        let properties = PublishProperties {
            correlation_data: Some(self.correlation(call)),
            user_properties: vec![(KIND.to_owned(), kind.to_owned())],
            ..Default::default()
        };
        self.publish(CONTROL.to_owned(), properties, body, None).await
    }

    fn enqueue(&self, id: u64, frame: Frame) {
        if let Some(ClientCall::Streaming { queue, .. }) = lock(&self.calls).get(&id) {
            let _ = queue.send(frame);
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

    /// Fails the publish waiting for its packet id and forgets the packets awaiting an
    /// acknowledgment, once the connection has ended: no `Outgoing::Publish` and no acknowledgment
    /// will come for them.
    fn ended(&self) {
        if let Some(waiting) = lock(&self.outgoing).take() {
            let _ = waiting.assigned.send(None);
        }
        lock(&self.pkids).clear();
    }

    /// A PUBACK or PUBREC for one of this client's publishes: 0x10 on a request is the miss
    /// signal.
    fn acknowledged(&self, pkid: u16, no_subscribers: bool, frames: &mpsc::UnboundedSender<Frame>) {
        let Some(call) = lock(&self.pkids).remove(&pkid) else { return };
        if no_subscribers && let Some(entry) = self.take(call) {
            let _ = frames.send(no_destination(call, entry.pattern()));
        }
    }
}

/// Polls the client connection until it fails, which, once the DISCONNECT `close` queued is
/// written, is the broker closing it, or until [`linger`]'s grace drops it.
async fn client_loop(
    side: Arc<ClientSide>,
    mut eventloop: EventLoop,
    frames: mpsc::UnboundedSender<Frame>,
    ready: oneshot::Sender<Result<(), BoxError>>,
    mut written: Written,
) {
    let mut ready = Some(ready);
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(connack))) => {
                if let Some(max) = connack.properties.and_then(|properties| properties.max_packet_size) {
                    side.max_packet.store(u64::from(max), Ordering::Relaxed);
                }
                if let Err(error) = side.client.try_subscribe(side.reply_topic.clone(), side.qos)
                    && let Some(ready) = ready.take()
                {
                    let _ = ready.send(Err(format!("the MQTT link could not subscribe to its reply topic: {error}").into()));
                    return;
                }
            }
            Ok(Event::Incoming(Packet::SubAck(suback))) => {
                if let Some(ready) = ready.take() {
                    let outcome = match suback.return_codes.first() {
                        Some(SubscribeReasonCode::Success(_)) => Ok(()),
                        code => Err(format!("the MQTT broker refused the reply topic subscription: {code:?}").into()),
                    };
                    let failed = outcome.is_err();
                    let _ = ready.send(outcome);
                    if failed {
                        return;
                    }
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let properties = publish.properties.unwrap_or_default();
                let Some(call) = properties.correlation_data.as_deref().and_then(|data| side.call_of(data)) else { continue };
                if kind_of(&properties.user_properties).as_deref() == Some(OPENED) {
                    #[cfg(feature = "conformance-probe")]
                    if side.probe.withholds(call) {
                        continue;
                    }
                    side.open_gate(call);
                    continue;
                }
                let frame = match side.codec.decode_frame(&publish.payload) {
                    Ok(frame) => with_id(frame, call),
                    Err(error) => {
                        tracing::warn!(%error, "the MQTT link dropped a reply that does not decode");
                        continue;
                    }
                };
                if is_terminal(&frame) {
                    side.take(call);
                }
                let _ = frames.send(frame);
            }
            Ok(Event::Incoming(Packet::PubAck(puback))) => {
                side.acknowledged(puback.pkid, puback.reason == PubAckReason::NoMatchingSubscribers, &frames);
            }
            Ok(Event::Incoming(Packet::PubRec(pubrec))) => {
                side.acknowledged(pubrec.pkid, pubrec.reason == PubRecReason::NoMatchingSubscribers, &frames);
            }
            Ok(Event::Outgoing(Outgoing::Publish(pkid))) => {
                if let Some(Waiting { call, assigned }) = lock(&side.outgoing).take() {
                    if let Some(call) = call
                        && pkid != 0
                    {
                        lock(&side.pkids).insert(pkid, call);
                    }
                    let _ = assigned.send(Some(pkid));
                }
            }
            // No acknowledgment read past the DISCONNECT is reported: the calls have ended.
            Ok(Event::Outgoing(Outgoing::Disconnect)) if *lock(&side.closed) => {
                side.ended();
                written.disconnect(&mut eventloop);
            }
            Ok(_) => {}
            Err(error) => {
                // A clean session drops whatever was queued, so the waiting publish is lost and no
                // acknowledgment will come for the packets in flight.
                side.ended();
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(format!("the MQTT link could not connect: {error}").into()));
                    return;
                }
                if *lock(&side.closed) {
                    tracing::debug!(%error, "the MQTT link's connection ended after `close`");
                    return;
                }
                // What the broker routed to the reply topic while the connection was down is gone,
                // so the reply lane ends here: `RpcClient` fails the calls waiting on it
                // `Unavailable` and connects again for the next.
                tracing::warn!(%error, "the MQTT link lost its connection; the calls waiting on it fail");
                return;
            }
        }
    }
}

/// Publishes one streamed request's control frames once the server has acknowledged its `open`.
async fn pump(side: Arc<ClientSide>, call: u64, opened: oneshot::Receiver<()>, mut queued: mpsc::UnboundedReceiver<Frame>) {
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
        if let Err(error) = side.publish_control(call, kind, body).await {
            tracing::debug!(%error, kind, "the MQTT link could not publish a control frame");
        }
    }
}

fn reply_properties(correlation: Option<Bytes>, kind: Option<&str>) -> PublishProperties {
    PublishProperties {
        correlation_data: correlation,
        user_properties: kind.map(|kind| vec![(KIND.to_owned(), kind.to_owned())]).unwrap_or_default(),
        ..Default::default()
    }
}

fn user_properties(headers: &CallHeaders, kind: Option<&str>) -> Vec<(String, String)> {
    let mut properties: Vec<(String, String)> = headers.iter().map(|(name, value)| (name.to_owned(), value.to_owned())).collect();
    if let Some(kind) = kind {
        properties.push((KIND.to_owned(), kind.to_owned()));
    }
    properties
}

fn kind_of(properties: &[(String, String)]) -> Option<String> {
    properties.iter().find(|(name, _)| name == KIND).map(|(_, value)| value.clone())
}

fn call_headers(properties: &[(String, String)]) -> CallHeaders {
    let mut headers = CallHeaders::new();
    for (name, value) in properties {
        if name != KIND {
            headers.insert(name.as_str(), value.as_str());
        }
    }
    headers
}

/// What a caller receives for a miss this link signals: `Unavailable` with
/// `reason: "no_destination"`, as `RpcClient` maps a `NoDestination` from `send`.
fn no_destination(id: u64, pattern: &Pattern) -> Frame {
    let mut details = Details::new();
    details.push(Detail::ErrorInfo { reason: "no_destination".to_owned(), domain: DOMAIN.to_owned(), metadata: BTreeMap::new() });
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, format!("nothing listens on pattern `{pattern}`"), details) }
}

/// The reply frame with the caller's own `id`, read from the Correlation Data: the server wrote
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

/// The `unavailable` with the shutdown phrase and no detail, which the core writes for a call the
/// drain's end abandons and for one it refuses once the drain began.
fn shutting_down(id: u64) -> Frame {
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE, Details::new()) }
}

fn is_terminal(frame: &Frame) -> bool {
    matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. })
}

/// A client id unique to one connection, inside the 23 characters every MQTT broker accepts.
fn client_id() -> String {
    format!("ulo-{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

/// A ShareName from the root module's path: MQTT refuses `/`, `+` and `#` in one, and whitespace
/// is replaced to keep the filter readable in a broker's logs.
fn share_name(text: &str) -> String {
    text.chars().map(|c| if c.is_whitespace() || matches!(c, '/' | '+' | '#') { '_' } else { c }).collect()
}

fn check_group(group: &str) -> Result<(), BoxError> {
    if group.is_empty() || group.chars().any(|c| matches!(c, '/' | '+' | '#')) {
        return Err(format!("the MQTT link's group `{group}` is not a ShareName: it must be non-empty, with no `/`, `+` or `#`").into());
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
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    use futures_util::FutureExt;
    use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError};
    use ulo_rpc::{RpcClient, Server};
    use ulo_transport::ErrorKind;

    use super::*;

    /// Root certificates loaded per test, each test's loader counting in a slot of its own.
    static LOADS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

    /// A store that cannot be read: rustls-native-certs reading a file that is not there, as it
    /// reads one `SSL_CERT_FILE` names.
    fn unreadable<const SLOT: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        rustls_native_certs::load_certs_from_paths(Some(Path::new("/nonexistent/ulo-rpc-mqtt-roots.pem")), None)
    }

    /// A store holding no root, read without an error.
    fn empty<const SLOT: usize>() -> CertificateResult {
        LOADS[SLOT].fetch_add(1, Ordering::SeqCst);
        CertificateResult::default()
    }

    /// An `mqtts://` link whose roots come from `load`. Nothing listens at the url: a link that
    /// reached a socket would fail `Unavailable`, not with the load's refusal.
    fn link(load: fn() -> CertificateResult) -> Mqtt {
        let mut link = Mqtt::url("mqtts://localhost:1");
        link.roots.load = load;
        link
    }

    struct Root;

    impl Module for Root {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, _m: &mut ModuleDef<'_>) {}
    }

    async fn refused_at_bind(link: Mqtt) -> String {
        let bound = App::builder(Root)
            .runtime(Tokio::current())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(Server::new(link))
            .listen()
            .await;
        match bound {
            Err(error @ StartupError::Configure(_)) => error.to_string(),
            Err(other) => panic!("the bind failed other than as a `Configure` refusal: {other:?}"),
            Ok(_) => panic!("a server whose root certificates did not load listened"),
        }
    }

    async fn refused_calls(link: Mqtt) -> Vec<(ErrorKind, Option<String>, String)> {
        let client = RpcClient::new(link, Arc::new(Tokio::current())).expect("the link is usable");
        let mut refused = Vec::new();
        for _ in 0..2 {
            let failed = client.request::<_, u32>("mqtt.roots", &()).await.expect_err("a call over a link whose roots did not load");
            refused.push((failed.kind(), failed.reason().map(str::to_owned), failed.message().to_owned()));
        }
        refused
    }

    /// F414: a store that cannot be read refuses a server's bind in `prepare`, naming the store and
    /// the cause, where the link before the fix panicked in `listen`, and the store is read once.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_refuses_the_server_s_bind() {
        let refused = refused_at_bind(link(unreadable::<0>)).await;
        assert!(
            refused.contains("could not load the platform's root certificates") && refused.contains("/nonexistent/ulo-rpc-mqtt-roots.pem"),
            "the refusal does not name the store and its cause: {refused}"
        );
        assert_eq!(LOADS[0].load(Ordering::SeqCst), 1, "the server read the store other than once");
    }

    /// F414: a store that cannot be read fails every client call `Internal` with
    /// `reason: "misconfigured_link"`, naming the cause, where the link before the fix panicked in
    /// each call, and the store is read once for both calls.
    #[tokio::test]
    async fn a_store_that_cannot_be_read_fails_every_call_misconfigured() {
        for (kind, reason, message) in refused_calls(link(unreadable::<1>)).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains("/nonexistent/ulo-rpc-mqtt-roots.pem"), "the call does not name the cause: {message}");
        }
        assert_eq!(LOADS[1].load(Ordering::SeqCst), 1, "the client read the store other than once for two calls");
    }

    /// F414: a store holding no root is refused as one that cannot be read, on both sides, where
    /// the link before the fix connected with an empty trust store and failed every handshake
    /// `Unavailable`.
    #[tokio::test]
    async fn a_store_holding_no_root_is_refused_as_misconfigured() {
        let refused = refused_at_bind(link(empty::<2>)).await;
        assert!(refused.contains("the platform's store holds none"), "the server's refusal does not say the store is empty: {refused}");
        for (kind, reason, message) in refused_calls(link(empty::<3>)).await {
            assert_eq!((kind, reason.as_deref()), (ErrorKind::Internal, Some("misconfigured_link")), "{message}");
            assert!(message.contains("the platform's store holds none"), "the call does not say the store is empty: {message}");
        }
        assert_eq!((LOADS[2].load(Ordering::SeqCst), LOADS[3].load(Ordering::SeqCst)), (1, 1), "a side read the store other than once");
    }

    /// F429: a server's link dropped holding a call lets go of the table the call is held in,
    /// undrained and drained. The held call's reply path holds the table and the table holds the
    /// path; the link's drop aborts the event loop, so the connection closes whatever the table
    /// holds, and the conformance suite's test counts it, but the table and the request channel
    /// its paths hold stay without the side's drop clearing the table and without the drain's
    /// watcher holding the side weakly. No broker is reached: the side's event loop is never
    /// polled, a task that never ends standing in for it, and a server with no patterns drains
    /// without waiting for an UNSUBACK.
    #[tokio::test]
    async fn a_server_link_dropped_holding_a_call_lets_go_of_its_table() {
        for drained in [false, true] {
            let link = Mqtt::url("mqtt://localhost:1");
            let runtime = Tokio::current();
            let (client, _eventloop) = AsyncClient::new(MqttOptions::new("held", "localhost", 1), 8);
            let (deliveries, inbound) = mpsc::unbounded_channel();
            let side = server_side(client, deliveries);
            let (_, path) = side.hold("held".to_owned(), "ulo/rpc/reply/held".to_owned(), None);
            drop(path);
            let table = Arc::downgrade(&side.calls);
            let event_loop = never_ending(&runtime);
            lock(&link.state).server = Some((side, event_loop));
            if drained {
                link.drain().await;
            }
            drop(inbound);
            drop(link);
            let dropped = std::time::Instant::now();
            while table.upgrade().is_some() {
                assert!(
                    dropped.elapsed() < Duration::from_secs(5),
                    "the {}server's link dropped holding a call kept its table 5s after the drop",
                    if drained { "drained " } else { "" }
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }

    /// An event loop that never ends and never reports its DISCONNECT written, timed on `runtime`.
    fn never_ending(runtime: &Tokio) -> Loop {
        Loop::spawn(runtime, Arc::new(runtime.clone()), |written| async move {
            let _written = written;
            std::future::pending::<()>().await;
        })
    }

    /// A server side built by hand on `client`, on the test's runtime, with no patterns.
    fn server_side(client: AsyncClient, deliveries: mpsc::UnboundedSender<Delivery>) -> Arc<ServerSide> {
        Arc::new(ServerSide {
            client,
            codec: Codec::Json,
            qos: MqttQoS::AtLeastOnce,
            runtime: Tokio::current(),
            shared: Vec::new(),
            calls: Arc::new(Calls::new()),
            replies: Arc::default(),
            phase: watch::channel(Phase::Serving).0,
            max_packet: Arc::new(AtomicU64::new(0)),
            deliveries: Mutex::new(Some(deliveries)),
            unsubscribing: Mutex::new(Unsubscribing::default()),
            unconfirmed: watch::channel(0).0,
        })
    }

    /// A server's link whose side is built by hand, with a request channel of `capacity` and an
    /// event loop that is never polled, a pending task standing in for it: what the side queues
    /// stays in the channel for [`queued`] to read in order. No broker is reached.
    fn unpolled_server(capacity: usize) -> (Mqtt, Arc<ServerSide>, EventLoop) {
        let link = Mqtt::url("mqtt://localhost:1");
        let (client, eventloop) = AsyncClient::new(MqttOptions::new("unpolled", "localhost", 1), capacity);
        let side = server_side(client, mpsc::unbounded_channel().0);
        let event_loop = never_ending(&Tokio::current());
        lock(&link.state).server = Some((Arc::clone(&side), event_loop));
        (link, side, eventloop)
    }

    /// A request the side queued, as the broker would read it.
    #[derive(Debug, PartialEq)]
    enum Queued {
        Reply { topic: String, correlation: Option<Bytes>, frame: Frame },
        Disconnect,
        Other(String),
    }

    /// Every request queued and not yet taken, oldest first.
    fn queued(eventloop: &mut EventLoop) -> Vec<Queued> {
        eventloop.clean();
        eventloop
            .pending
            .drain(..)
            .map(|request| match request {
                rumqttc::v5::Request::Publish(publish) => Queued::Reply {
                    topic: String::from_utf8_lossy(&publish.topic).into_owned(),
                    correlation: publish.properties.and_then(|properties| properties.correlation_data),
                    frame: Codec::Json.decode_frame(&publish.payload).expect("a reply decodes"),
                },
                rumqttc::v5::Request::Disconnect => Queued::Disconnect,
                other => Queued::Other(format!("{other:?}")),
            })
            .collect()
    }

    /// What the caller under `correlation` reads for frame `frame` on reply topic `topic`.
    fn reply(topic: &str, correlation: &'static str, frame: Frame) -> Queued {
        Queued::Reply { topic: topic.to_owned(), correlation: Some(Bytes::from_static(correlation.as_bytes())), frame }
    }

    /// The `err` the core writes for a call the drain's end abandons, which `close` writes for a
    /// call no terminal frame was queued for: kind `unavailable`, the shutdown phrase, no detail
    /// and so no reason.
    fn abandoned(id: u64) -> Frame {
        Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE, Details::new()) }
    }

    fn res(id: u64, data: &'static [u8]) -> Frame {
        Frame::Res { id, data: Data::new(Bytes::from_static(data)) }
    }

    fn item(id: u64, data: &'static [u8]) -> Frame {
        Frame::Item { id, data: Data::new(Bytes::from_static(data)) }
    }

    fn hold(side: &ServerSide, correlation: &'static str) -> (u64, ReplyPath) {
        side.hold(correlation.to_owned(), format!("ulo/rpc/reply/{correlation}"), Some(Bytes::from_static(correlation.as_bytes())))
    }

    /// `close`'s first poll, all a `close` under a spent shutdown cap gets, then its drop.
    fn close_once(link: &Mqtt) {
        assert!(link.close().now_or_never().is_none(), "the close ended though its event loop never ran");
    }

    /// H1: a server's `close`, in its first poll, queues the `unavailable` with the shutdown
    /// phrase for every held call no terminal frame was queued for, then the DISCONNECT, and
    /// drops what a call sends after it. An answered call gets nothing more, and a stream gets its
    /// items, then the `unavailable`. The answers of different calls come in no promised order.
    #[tokio::test]
    async fn close_answers_each_call_still_held_unavailable_ahead_of_the_disconnect() {
        let (link, side, mut eventloop) = unpolled_server(16);
        let (unanswered, unanswered_path) = hold(&side, "a.1");
        let (answered, answered_path) = hold(&side, "a.2");
        let (streaming, streaming_path) = hold(&side, "a.3");
        answered_path.send(res(answered, b"7")).await.expect("the answered call's reply is queued");
        streaming_path.send(item(streaming, b"1")).await.expect("the stream's item is queued");

        close_once(&link);
        let late = unanswered_path.send(res(unanswered, b"8")).await;
        let late_end = streaming_path.send(Frame::End { id: streaming }).await;

        let mut taken = queued(&mut eventloop);
        assert_eq!(taken.pop(), Some(Queued::Disconnect), "the close's last request is not the DISCONNECT");
        assert_eq!(
            taken[..2],
            [reply("ulo/rpc/reply/a.2", "a.2", res(answered, b"7")), reply("ulo/rpc/reply/a.3", "a.3", item(streaming, b"1"))],
            "what was queued before the close, in order"
        );
        let mut answers: Vec<String> = taken[2..].iter().map(|queued| format!("{queued:?}")).collect();
        answers.sort();
        let mut expected = [reply("ulo/rpc/reply/a.1", "a.1", abandoned(unanswered)), reply("ulo/rpc/reply/a.3", "a.3", abandoned(streaming))]
            .map(|queued| format!("{queued:?}"));
        expected.sort();
        assert_eq!(answers, expected, "the close's answers, one per call still held");
        let late = late.expect_err("a reply sent after the close was taken");
        assert_eq!(late.to_string(), "the MQTT link has closed; the reply was not sent");
        assert!(late_end.is_err(), "a stream's end sent after the close was taken");
    }

    /// H1: a reply's place against the DISCONNECT is fixed when its send is called. Its future,
    /// first polled after `close` queued the DISCONNECT, answers the reply sent, and `close`
    /// writes the call nothing more.
    #[tokio::test]
    async fn a_reply_sent_before_close_keeps_its_place_though_its_future_is_polled_after() {
        let (link, side, mut eventloop) = unpolled_server(16);
        let (id, path) = hold(&side, "b.1");
        let sending = path.send(res(id, b"9"));

        close_once(&link);
        sending.await.expect("the reply sent before the close is queued");

        assert_eq!(
            queued(&mut eventloop),
            vec![reply("ulo/rpc/reply/b.1", "b.1", res(id, b"9")), Queued::Disconnect],
            "what the close queued, in order"
        );
    }

    /// What the event loop takes, then the wake its return from a poll gives a reply waiting for
    /// room.
    fn take(eventloop: &mut EventLoop, side: &ServerSide) -> Vec<Queued> {
        let taken = queued(eventloop);
        side.replies.room.notify_waiters();
        taken
    }

    /// H1, I1: a reply finding the request channel full waits for room in its future, the lock
    /// let go, and is queued once the event loop takes what is ahead of it, in order; a stream
    /// longer than the channel is paced rather than cut. The call it belongs to stays held while
    /// its frame waits.
    #[tokio::test]
    async fn a_reply_finding_the_channel_full_waits_for_room_outside_the_lock() {
        let (_link, side, mut eventloop) = unpolled_server(3);
        let (paced, paced_path) = hold(&side, "c.1");
        paced_path.send(item(paced, b"1")).await.expect("the first item is queued");
        paced_path.send(item(paced, b"2")).await.expect("the second item is queued");
        paced_path.send(item(paced, b"3")).await.expect("the third item is queued");

        let mut waiting = paced_path.send(item(paced, b"4"));
        assert!((&mut waiting).now_or_never().is_none(), "a send to a full channel answered without waiting");
        assert!(side.calls.get("c.1").is_some(), "the call whose frame waits is not held");

        let mut taken = take(&mut eventloop, &side);
        tokio::time::timeout(Duration::from_secs(5), waiting).await.expect("the waiting item was never queued").expect("the waiting item is queued once the channel has room");
        taken.extend(queued(&mut eventloop));
        assert_eq!(
            taken,
            (1..=4).map(|n| reply("ulo/rpc/reply/c.1", "c.1", item(paced, n.to_string().leak().as_bytes()))).collect::<Vec<_>>(),
            "what the channel took, in order"
        );
        assert!(side.calls.get("c.1").is_some(), "a stream whose items were all queued is no longer held");
    }

    /// I1: a reply waiting for room when `close` runs never reaches the channel: retried under the
    /// lock once there is room, it finds the replies closed and is dropped. `close`'s own answer
    /// for the call, which the full channel refused in `close`'s first poll, waits for room and is
    /// queued ahead of the DISCONNECT, so the caller reads exactly one terminal frame.
    #[tokio::test]
    async fn a_reply_waiting_for_room_when_close_runs_is_dropped_and_close_answers_its_call() {
        let (link, side, mut eventloop) = unpolled_server(3);
        let (paced, paced_path) = hold(&side, "e.1");
        for n in [b"1", b"2", b"3"] {
            paced_path.send(item(paced, n)).await.expect("an item is queued");
        }
        let mut waiting = paced_path.send(item(paced, b"4"));
        assert!((&mut waiting).now_or_never().is_none(), "a send to a full channel answered without waiting");

        let mut closing = pin!(link.close());
        assert!(closing.as_mut().now_or_never().is_none(), "the close ended with its answer refused by the full channel");
        let mut taken = Vec::new();
        let mut late = None;
        for _ in 0..8 {
            taken.extend(take(&mut eventloop, &side));
            if late.is_none() {
                late = (&mut waiting).now_or_never();
            }
            tokio::task::yield_now().await;
            let _ = closing.as_mut().now_or_never();
        }
        let late = late.expect("the waiting item was not woken by the room the take made");
        assert_eq!(late.expect_err("the item waiting when `close` ran was queued").to_string(), "the MQTT link has closed; the reply was not sent");
        let mut expected: Vec<Queued> = [b"1", b"2", b"3"].into_iter().map(|n| reply("ulo/rpc/reply/e.1", "e.1", item(paced, n))).collect();
        expected.extend([reply("ulo/rpc/reply/e.1", "e.1", abandoned(paced)), Queued::Disconnect]);
        assert_eq!(taken, expected, "what the channel took: the items queued before the close, its one answer, the DISCONNECT");
    }

    /// I1: a terminal frame that waited for room ends its call once queued, so `close` writes the
    /// call nothing more.
    #[tokio::test]
    async fn a_terminal_frame_queued_after_waiting_ends_its_call() {
        let (link, side, mut eventloop) = unpolled_server(1);
        let (id, path) = hold(&side, "f.1");
        path.send(item(id, b"1")).await.expect("the item is queued");
        let mut ending = path.send(Frame::End { id });
        assert!((&mut ending).now_or_never().is_none(), "a send to a full channel answered without waiting");

        let mut taken = take(&mut eventloop, &side);
        tokio::time::timeout(Duration::from_secs(5), ending).await.expect("the waiting end was never queued").expect("the end is queued");
        assert!(side.calls.get("f.1").is_none(), "the call whose end was queued is still held");
        taken.extend(queued(&mut eventloop));
        close_once(&link);
        taken.extend(queued(&mut eventloop));
        assert_eq!(
            taken,
            vec![reply("ulo/rpc/reply/f.1", "f.1", item(id, b"1")), reply("ulo/rpc/reply/f.1", "f.1", Frame::End { id }), Queued::Disconnect],
            "what the channel took: the stream and its end, then the DISCONNECT alone"
        );
    }

    /// A reply waiting for room when the event loop ends fails its call rather than waiting on a
    /// channel nobody takes from.
    #[tokio::test]
    async fn a_reply_waiting_for_room_fails_once_the_event_loop_ends() {
        let (_link, side, _eventloop) = unpolled_server(1);
        let (id, path) = hold(&side, "g.1");
        path.send(item(id, b"1")).await.expect("the item is queued");
        let mut waiting = path.send(item(id, b"2"));
        assert!((&mut waiting).now_or_never().is_none(), "a send to a full channel answered without waiting");
        drop(LoopEnded(Arc::clone(&side.replies)));
        let failed = tokio::time::timeout(Duration::from_secs(5), waiting).await.expect("the waiting item did not wake as the loop ended");
        assert_eq!(failed.expect_err("an item was queued with no loop to take it").to_string(), "the MQTT link's connection has ended; the reply was not sent");
        assert!(side.calls.get("g.1").is_none(), "the call whose reply failed is still held");
    }

    /// I4: a refusal of a request that arrived after the inbound stream ended is queued under the
    /// lock `close` takes: one issued before `close` is written ahead of the DISCONNECT, and one
    /// issued after it is not queued at all, MQTT allowing no packet after the DISCONNECT.
    #[tokio::test]
    async fn a_refusal_is_queued_ahead_of_the_disconnect_or_not_at_all() {
        let (link, side, mut eventloop) = unpolled_server(16);
        side.refuse("ulo/rpc/reply/r.1".to_owned(), Some(Bytes::from_static(b"r.1")), "mqtt.refused");
        close_once(&link);
        side.refuse("ulo/rpc/reply/r.2".to_owned(), Some(Bytes::from_static(b"r.2")), "mqtt.refused");
        assert_eq!(
            queued(&mut eventloop),
            vec![reply("ulo/rpc/reply/r.1", "r.1", abandoned(0)), Queued::Disconnect],
            "what the channel took: the refusal issued before the close, then the DISCONNECT"
        );
    }

    /// J7: a reply waiting for room makes and enables its wait before each attempt, so room made
    /// between a failed attempt and the wait wakes it. A gate run there takes what the channel
    /// holds and wakes the waiters, once; a wait made after the attempt misses that wake, and the
    /// send waits for one that never comes.
    #[tokio::test]
    async fn room_made_between_a_failed_attempt_and_the_wait_wakes_the_reply() {
        let (_link, side, eventloop) = unpolled_server(1);
        let (id, path) = hold(&side, "j.1");
        path.send(item(id, b"1")).await.expect("the first item is queued");
        let eventloop = Arc::new(Mutex::new(eventloop));
        let taken: Arc<Mutex<Vec<Queued>>> = Arc::default();
        let (taker, recorded, replies) = (Arc::clone(&eventloop), Arc::clone(&taken), Arc::downgrade(&side.replies));
        *lock(&side.replies.between) = Some(Box::new(move || {
            lock(&recorded).extend(queued(&mut lock(&taker)));
            if let Some(replies) = replies.upgrade() {
                replies.room.notify_waiters();
            }
        }));

        tokio::time::timeout(Duration::from_secs(5), path.send(item(id, b"2")))
            .await
            .expect("the reply missed the room made between its attempt and its wait")
            .expect("the second item is queued");
        assert!(lock(&side.replies.between).is_none(), "the gate did not run: no attempt failed before the wait");
        let mut taken = std::mem::take(&mut *lock(&taken));
        taken.extend(queued(&mut lock(&eventloop)));
        assert_eq!(taken, vec![reply("ulo/rpc/reply/j.1", "j.1", item(id, b"1")), reply("ulo/rpc/reply/j.1", "j.1", item(id, b"2"))], "what the channel took, in order");
    }

    /// A clock whose sleeps end only when the test fires it, recording each duration asked for.
    #[derive(Default)]
    struct Scripted {
        asked: Mutex<Vec<Duration>>,
        fired: Arc<Notify>,
    }

    impl Timer for Scripted {
        fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
            lock(&self.asked).push(d);
            let fired = Arc::clone(&self.fired);
            Box::pin(async move { fired.notified().await })
        }

        fn now(&self) -> std::time::Instant {
            std::time::Instant::now()
        }
    }

    /// A connection outlives `close` by [`CUT_GRACE`] on the clock its loop was given, starting as
    /// `close` ends: a loop whose connection never ends asks that clock for the grace once
    /// released, and ends when the clock says it has passed.
    #[tokio::test]
    async fn the_linger_runs_on_the_clock_the_loop_was_given() {
        let clock = Arc::new(Scripted::default());
        let Loop { task, released, .. } = Loop::spawn(&Tokio::current(), Arc::clone(&clock) as Arc<dyn Timer>, |written| async move {
            let _written = written;
            std::future::pending::<()>().await;
        });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(*lock(&clock.asked), Vec::<Duration>::new(), "the grace started before `close` ended");
        drop(released);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(*lock(&clock.asked), vec![CUT_GRACE], "what the loop asked its clock for once released");
        assert!(!task.is_finished(), "the loop ended before its clock said the grace had passed");
        clock.fired.notify_waiters();
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("the loop outlived its grace").expect("the loop's task");
    }

    /// A client side built by hand on `client`, on the test's runtime.
    fn client_side(client: AsyncClient) -> Arc<ClientSide> {
        Arc::new(ClientSide {
            client,
            codec: Codec::Json,
            qos: MqttQoS::AtLeastOnce,
            runtime: Tokio::current(),
            id: "ulo-closing".to_owned(),
            reply_topic: "ulo/rpc/reply/ulo-closing".to_owned(),
            max_packet: Arc::new(AtomicU64::new(0)),
            calls: Mutex::new(HashMap::new()),
            order: tokio::sync::Mutex::new(()),
            outgoing: Mutex::new(None),
            pkids: Mutex::new(HashMap::new()),
            closed: Mutex::new(false),
            #[cfg(feature = "conformance-probe")]
            probe: Arc::default(),
        })
    }

    /// A client's publish sent after its `close` queued the DISCONNECT is refused rather than
    /// queued behind it: the event loop, polled past the DISCONNECT, would write it, and MQTT
    /// allows no packet after the DISCONNECT.
    #[tokio::test]
    async fn a_publish_after_the_client_s_close_is_refused_not_queued_behind_the_disconnect() {
        let link = Mqtt::url("mqtt://localhost:1");
        let (client, mut eventloop) = AsyncClient::new(MqttOptions::new("closing", "localhost", 1), 8);
        let side = client_side(client);
        lock(&link.state).client = Some((Arc::clone(&side), never_ending(&Tokio::current())));
        close_once(&link);
        let event = Frame::Evt { pattern: "link.late".to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"1")) };
        let refused = tokio::time::timeout(Duration::from_secs(5), side.send(Pattern::from("link.late"), event))
            .await
            .expect("a publish sent after `close` waited rather than being refused")
            .expect_err("a publish sent after `close` was queued");
        assert_eq!(refused.to_string(), "the MQTT link has closed; the publish was not sent");
        assert_eq!(queued(&mut eventloop), vec![Queued::Disconnect], "what the client's close queued");
    }

    /// The RPC suite's stall scenario over `mqtts://`, the one url on which the link loads roots,
    /// against a Mosquitto serving TLS, which the suite's own broker does not.
    #[cfg(feature = "integration")]
    mod over_tls {
        use std::net::{Ipv4Addr, SocketAddr};

        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;
        use testcontainers::{ContainerAsync, GenericImage, ImageExt};
        use ulo_rpc_conformance::relay::{reachable, unshadowed};
        use ulo_rpc_conformance::{Broker, Budget, Harness, OnTokio, report, startup_failed};

        use super::*;

        const TLS_PORT: u16 = 8883;

        /// The bound this test holds the thread to, half the suite's: the provider's one-time
        /// initialisation, left to the first handshake, held the thread 35 to 43 ms of the suite's 50
        /// in a debug build, and a handshake after it 9 to 13 ms.
        const STALL: Duration = Duration::from_millis(25);

        const CONFIG: &str = "listener 8883\nallow_anonymous true\ncafile /mosquitto/config/server.crt\ncertfile /mosquitto/config/server.crt\nkeyfile /mosquitto/config/server.key\n";

        /// Roots loaded by [`slow_test_roots`], the server's link and each client's once.
        static SLOW_LOADS: AtomicUsize = AtomicUsize::new(0);

        /// The test certificate as the store's one root, after blocking twice the bound, as a trust
        /// store slow to read does: 71 ms warm on macOS, past the bound already.
        fn slow_test_roots() -> CertificateResult {
            SLOW_LOADS.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(STALL * 2);
            let mut loaded = CertificateResult::default();
            loaded.certs.push(ulo_test_certs::localhost().cert_der().clone());
            loaded
        }

        struct TlsMosquitto {
            server: SocketAddr,
            _container: ContainerAsync<GenericImage>,
        }

        impl Broker for TlsMosquitto {
            type Harness = OnTokio;
            type Link = Mqtt;

            async fn start() -> Self {
                let cert = ulo_test_certs::localhost();
                let (container, addrs) = unshadowed("the TLS Mosquitto container", || async {
                    let container = GenericImage::new("eclipse-mosquitto", "2.0.18")
                        .with_exposed_port(TLS_PORT.tcp())
                        .with_wait_for(WaitFor::message_on_stderr(" running"))
                        .with_copy_to("/mosquitto/config/tls.conf", CONFIG.as_bytes().to_vec())
                        .with_copy_to("/mosquitto/config/server.crt", cert.cert_pem().as_bytes().to_vec())
                        .with_copy_to("/mosquitto/config/server.key", cert.key_pem().as_bytes().to_vec())
                        .with_cmd(["mosquitto", "-c", "/mosquitto/config/tls.conf"])
                        .start()
                        .await
                        .unwrap_or_else(|error| startup_failed!("the TLS Mosquitto container did not start: {}", report(&error)));
                    let port = container
                        .get_host_port_ipv4(TLS_PORT)
                        .await
                        .unwrap_or_else(|error| startup_failed!("the MQTT TLS port is not mapped: {}", report(&error)));
                    (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
                })
                .await;
                reachable(addrs[0], Duration::from_secs(10)).await;
                TlsMosquitto { server: addrs[0], _container: container }
            }

            fn link(&self) -> Mqtt {
                let mut link = Mqtt::url(format!("mqtts://localhost:{}", self.server.port()));
                link.roots.load = slow_test_roots;
                link
            }

            async fn disrupt(&self) {}

            fn budget(&self) -> Budget {
                Budget::default().with_stall(STALL)
            }
        }

        /// F414: connecting over `mqtts://` does not stall the thread polling the connect, the
        /// suite's `connecting_does_not_stall_the_thread` against [`TlsMosquitto`] with a bound of
        /// [`STALL`], each link's roots blocking twice the bound while they load. Each link loads
        /// them once, the server's and each of the scenario's three clients'.
        #[test]
        fn connecting_over_tls_does_not_stall_the_thread() {
            OnTokio::block_on_one_thread(ulo_rpc_conformance::cases::threads::connecting_does_not_stall_the_thread::<TlsMosquitto>());
            assert_eq!(SLOW_LOADS.load(Ordering::SeqCst), 4, "the server's link and three clients' each loaded their roots other than once");
        }
    }
}
