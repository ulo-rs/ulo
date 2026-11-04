//! An RPC link written as a crate outside the `ulo` workspace would write one, against public API
//! alone: `ulo_rpc`'s link SPI and `ulo_transport::sync::ordered`, the queue a link's writer is fed
//! through. CI builds it as its own workspace, runs `scripts/private_paths.py scan` over it, and
//! runs `ulo-rpc-conformance` against it in `tests/conformance.rs`.
//!
//! The link carries frames between servers and clients in one process. A [`Hub`] stands for the
//! broker: a server's link subscribes its patterns there, and a client's `send` routes each
//! request at the call to one subscribed server, round-robin, or fails with `NoDestination` when
//! none is. Every frame one side sends goes through one bounded `ordered` queue towards the other
//! side, so frames arrive in the order their sends were called, and a full queue holds the sender
//! back.

use std::collections::{HashMap, HashSet};
use std::future::{Future, ready};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use futures_util::StreamExt;
use futures_util::stream;
use ulo::{BoxError, BoxFuture, Runtime};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, Capabilities, Delivery, DeliveryMode, Frame, Link, NoDestination, Ordering, Outbound, Pattern, ReplyPath, ReplyTo};
use ulo_transport::sync::ordered;

/// Deliveries queued for one server ahead of the next send's wait.
const INBOUND_QUEUE: usize = 1024;

/// Reply frames queued for one client ahead of the next reply's wait.
const REPLY_QUEUE: usize = 64;

/// The process-local broker the links of one environment share. Cheap to clone, every clone the
/// same hub.
#[derive(Clone, Default)]
pub struct Hub {
    inner: Arc<HubInner>,
}

#[derive(Default)]
struct HubInner {
    state: Mutex<HubState>,
}

#[derive(Default)]
struct HubState {
    subscribers: HashMap<Pattern, Vec<Weak<Instance>>>,
    /// The next subscriber each pattern's requests go to.
    turn: HashMap<Pattern, usize>,
    clients: Vec<Weak<Client>>,
}

impl Hub {
    pub fn new() -> Hub {
        Hub::default()
    }

    /// Severs every client connection: what each had queued is still read, then its reply stream
    /// ends, its sends fail, and each server is sent a `cancel` with no reply path for every call
    /// the connection held, as a closed TCP connection is reported.
    pub fn disrupt(&self) {
        let clients: Vec<Arc<Client>> = lock(&self.inner.state).clients.drain(..).filter_map(|c| c.upgrade()).collect();
        for client in clients {
            client.sever();
        }
    }

    /// The client connections open on the hub.
    pub fn client_connections(&self) -> usize {
        let mut state = lock(&self.inner.state);
        state.clients.retain(|c| c.upgrade().is_some_and(|c| !c.is_severed()));
        state.clients.len()
    }

    /// The next server subscribed to `pattern`, round-robin among those not draining.
    fn pick(&self, pattern: &Pattern) -> Option<Arc<Instance>> {
        let mut state = lock(&self.inner.state);
        let live: Vec<Arc<Instance>> = state
            .subscribers
            .get(pattern)
            .map(|subs| subs.iter().filter_map(Weak::upgrade).filter(|i| !i.draining.load(AtomicOrdering::Acquire)).collect())
            .unwrap_or_default();
        if live.is_empty() {
            return None;
        }
        let turn = state.turn.entry(pattern.clone()).or_insert(0);
        let picked = Arc::clone(&live[*turn % live.len()]);
        *turn = turn.wrapping_add(1);
        Some(picked)
    }

    fn subscribe(&self, instance: &Arc<Instance>, patterns: &[Pattern]) {
        let mut state = lock(&self.inner.state);
        for pattern in patterns {
            state.subscribers.entry(pattern.clone()).or_default().push(Arc::downgrade(instance));
        }
    }

    fn unsubscribe(&self, instance: &Arc<Instance>) {
        let mut state = lock(&self.inner.state);
        for subs in state.subscribers.values_mut() {
            subs.retain(|sub| sub.upgrade().is_some_and(|sub| !Arc::ptr_eq(&sub, instance)));
        }
    }

    fn register(&self, client: &Arc<Client>) {
        lock(&self.inner.state).clients.push(Arc::downgrade(client));
    }
}

/// One server's side of the hub: the queue its deliveries go through, and the calls it holds.
struct Instance {
    inbound: Mutex<Option<ordered::Sender<Delivery>>>,
    /// The ids of the calls started and not yet ended, which keep the inbound stream open through
    /// the drain so their `in`, `in_end` and `cancel` still arrive.
    calls: Mutex<HashSet<u64>>,
    draining: AtomicBool,
    next_id: AtomicU64,
}

impl Instance {
    fn deliver(&self, delivery: Delivery) -> BoxFuture<'static, Result<(), BoxError>> {
        let Some(inbound) = lock(&self.inbound).clone() else {
            return Box::pin(ready(Err(BoxError::from("the server's link has closed"))));
        };
        let room = inbound.send(delivery);
        Box::pin(async move { room.await.map_err(BoxError::from) })
    }

    /// Forgets call `id`; the inbound stream ends once a draining server holds none.
    fn finish(&self, id: u64) {
        let mut calls = lock(&self.calls);
        calls.remove(&id);
        if calls.is_empty() && self.draining.load(AtomicOrdering::Acquire) {
            lock(&self.inbound).take();
        }
    }

    fn drain(&self) {
        self.draining.store(true, AtomicOrdering::Release);
        if lock(&self.calls).is_empty() {
            lock(&self.inbound).take();
        }
    }
}

/// One client connection: the queue its replies come through, and where each of its calls went.
struct Client {
    replies: Mutex<Option<ordered::Sender<Frame>>>,
    severed: AtomicBool,
    routes: Mutex<HashMap<u64, Route>>,
}

/// Where one call of a client went: the server, the id the server knows it by, and its reply
/// path, which its `in`, `in_end` and `cancel` carry.
struct Route {
    instance: Weak<Instance>,
    id: u64,
    reply: ReplyPath,
}

impl Client {
    fn is_severed(&self) -> bool {
        self.severed.load(AtomicOrdering::Acquire)
    }

    fn sever(&self) {
        self.severed.store(true, AtomicOrdering::Release);
        lock(&self.replies).take();
        let routes: Vec<Route> = lock(&self.routes).drain().map(|(_, route)| route).collect();
        for route in routes {
            if let Some(instance) = route.instance.upgrade() {
                drop(instance.deliver(Delivery { frame: Frame::Cancel { id: route.id }, reply: None, ack: Ack::none() }));
                instance.finish(route.id);
            }
        }
    }

    /// Routes one frame at the call, so the order of the sends is the order of arrival.
    fn send(self: &Arc<Self>, hub: &Hub, pattern: Pattern, frame: Frame) -> BoxFuture<'static, Result<(), BoxError>> {
        if self.is_severed() {
            return Box::pin(ready(Err(BoxError::from("the connection was severed"))));
        }
        match frame {
            Frame::Req { id, .. } | Frame::Open { id, .. } => {
                let Some(instance) = hub.pick(&pattern) else {
                    return Box::pin(ready(Err(BoxError::from(NoDestination { pattern: pattern.as_str().to_owned() }))));
                };
                let server_id = instance.next_id.fetch_add(1, AtomicOrdering::Relaxed);
                let reply = reply_path(Arc::downgrade(self), id, Arc::downgrade(&instance), server_id);
                lock(&instance.calls).insert(server_id);
                lock(&self.routes).insert(id, Route { instance: Arc::downgrade(&instance), id: server_id, reply: reply.clone() });
                instance.deliver(Delivery { frame: with_id(frame, server_id), reply: Some(reply), ack: Ack::none() })
            }
            Frame::Evt { .. } => match hub.pick(&pattern) {
                Some(instance) => instance.deliver(Delivery { frame, reply: None, ack: Ack::none() }),
                None => Box::pin(ready(Err(BoxError::from(NoDestination { pattern: pattern.as_str().to_owned() })))),
            },
            Frame::In { id, .. } | Frame::InEnd { id } | Frame::Cancel { id } => {
                let cancel = matches!(frame, Frame::Cancel { .. });
                let route = {
                    let mut routes = lock(&self.routes);
                    if cancel { routes.remove(&id) } else { routes.get(&id).map(|r| Route { instance: r.instance.clone(), id: r.id, reply: r.reply.clone() }) }
                };
                let Some(route) = route else { return Box::pin(ready(Ok(()))) };
                let Some(instance) = route.instance.upgrade() else { return Box::pin(ready(Ok(()))) };
                let sent = instance.deliver(Delivery { frame: with_id(frame, route.id), reply: Some(route.reply), ack: Ack::none() });
                if cancel {
                    instance.finish(route.id);
                }
                sent
            }
            _ => Box::pin(ready(Ok(()))),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.sever();
    }
}

/// A server's replies to call `server_id`, carried back to the client under its own id
/// `client_id`. The call is forgotten on both sides at its last frame.
fn reply_path(client: Weak<Client>, client_id: u64, instance: Weak<Instance>, server_id: u64) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let Some(replies) = client.upgrade().and_then(|c| lock(&c.replies).clone()) else {
            return Box::pin(ready(Err(BoxError::from("the caller has gone"))));
        };
        let last = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
        let room = replies.send(with_id(frame, client_id));
        if last {
            if let Some(client) = client.upgrade() {
                let mut routes = lock(&client.routes);
                if routes.get(&client_id).is_some_and(|route| route.id == server_id) {
                    routes.remove(&client_id);
                }
            }
            if let Some(instance) = instance.upgrade() {
                instance.finish(server_id);
            }
        }
        Box::pin(async move { room.await.map_err(BoxError::from) })
    })
}

/// `frame` under the id `id`; a frame carrying none is answered as it stands.
fn with_id(frame: Frame, id: u64) -> Frame {
    match frame {
        Frame::Req { pattern, headers, data, .. } => Frame::Req { id, pattern, headers, data },
        Frame::Res { data, .. } => Frame::Res { id, data },
        Frame::Err { error, .. } => Frame::Err { id, error },
        Frame::Item { data, .. } => Frame::Item { id, data },
        Frame::End { .. } => Frame::End { id },
        Frame::Open { pattern, headers, .. } => Frame::Open { id, pattern, headers },
        Frame::In { data, .. } => Frame::In { id, data },
        Frame::InEnd { .. } => Frame::InEnd { id },
        Frame::Cancel { .. } => Frame::Cancel { id },
        Frame::Credit { n, .. } => Frame::Credit { id, n },
        other => other,
    }
}

/// The link over a [`Hub`]: as a server, it subscribes its patterns there; as a client, each
/// `connect` opens a connection to it.
pub struct Loopback {
    hub: Hub,
    server: Mutex<Option<Arc<Instance>>>,
    clients: Mutex<Vec<Weak<Client>>>,
}

impl Loopback {
    pub fn on(hub: &Hub) -> Loopback {
        Loopback { hub: hub.clone(), server: Mutex::new(None), clients: Mutex::new(Vec::new()) }
    }
}

impl Link for Loopback {
    const NAME: &'static str = "loopback";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Competing).binary(true).ordering(Ordering::PerConnection).miss_signal(true).ordered_control(true)
    }

    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        let (sender, mut receiver) = ordered::channel(INBOUND_QUEUE);
        let instance = Arc::new(Instance {
            inbound: Mutex::new(Some(sender)),
            calls: Mutex::new(HashSet::new()),
            draining: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        });
        self.hub.subscribe(&instance, patterns);
        *lock(&self.server) = Some(instance);
        let inbound: Inbound = stream::poll_fn(move |cx| receiver.poll_recv(cx)).boxed();
        ready(Ok(inbound))
    }

    fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
        let _ = runtime;
        let (sender, mut receiver) = ordered::channel(REPLY_QUEUE);
        let client = Arc::new(Client { replies: Mutex::new(Some(sender)), severed: AtomicBool::new(false), routes: Mutex::new(HashMap::new()) });
        self.hub.register(&client);
        lock(&self.clients).push(Arc::downgrade(&client));
        let hub = self.hub.clone();
        let send = Box::new(move |pattern: Pattern, frame: Frame, _reply: Option<ReplyTo>| client.send(&hub, pattern, frame));
        let replies = stream::poll_fn(move |cx| receiver.poll_recv(cx)).boxed();
        ready(Ok(Outbound { send, replies }))
    }

    /// No new call reaches the server once this returns; the calls it holds still receive their
    /// `in`, `in_end` and `cancel`, and the inbound stream ends with the last of them.
    fn drain(&self) -> impl Future<Output = ()> + Send {
        if let Some(instance) = lock(&self.server).clone() {
            self.hub.unsubscribe(&instance);
            instance.drain();
        }
        ready(())
    }

    /// Ends the server's inbound stream and severs every client connection this link opened.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        if let Some(instance) = lock(&self.server).take() {
            self.hub.unsubscribe(&instance);
            instance.draining.store(true, AtomicOrdering::Release);
            lock(&instance.inbound).take();
        }
        let clients: Vec<Arc<Client>> = lock(&self.clients).drain(..).filter_map(|c| c.upgrade()).collect();
        for client in clients {
            client.sever();
        }
        ready(Ok(()))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
