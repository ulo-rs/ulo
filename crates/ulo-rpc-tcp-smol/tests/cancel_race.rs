//! A caller's `cancel` is the call's reason, `ClientCancelled`, even when the server sends a
//! reply between the link reading the `cancel` and the server taking it. The link writes a call
//! nothing once it reads the call's `cancel`; a send refused in that window would cancel the call
//! `Disconnected` before the `cancel` arrived.
//!
//! [`Gated`] wraps the server's link. It holds each caller's `cancel` on its way from the link to
//! the server, after the link has read it, until the server's send of the handler's next item
//! waits, or has returned and the stream been polled again or dropped. Each step waits on the one
//! before it, so the item is sent inside the window on every run.
//!
//! Once the server has taken the `cancel`, the call holds the link no longer: the draining
//! server's inbound stream ends while the client is still connected.

use std::future::{Future, poll_fn};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::{Either, select};
use futures_util::{Stream, StreamExt, stream};
use ulo::{App, AppHandle, BoundAddr, BoxError, BoxFuture, CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, Timer, controller, handlers};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Delivery, Frame, Link, Outbound, Subscription, ReplyPath, RpcClient};
use ulo_rpc_tcp_smol::Tcp;
use ulo_transport::sync::Watch;
use ulo_transport::{CallError, Count};

const TICKS: &str = "cancel_race.ticks";

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// The server's drain window, far longer than [`PATIENCE`].
const DRAIN: Duration = Duration::from_secs(3600);

/// What the gate, the handler and the test have seen, in the order they see it.
#[derive(Default)]
struct Steps {
    /// The gate holds the caller's `cancel`, which the link has read.
    held: bool,
    /// The handler's stream was polled after the item it yields once `held` is set, so the
    /// server's send of that item has returned, or the stream was dropped.
    replied: bool,
    /// The execution's cancel reason as the stream was dropped.
    reason: Option<Option<CancelReason>>,
    /// A send the server made once `held` was set is waiting.
    waiting: bool,
    /// The link's inbound stream ended: the draining server held no call.
    inbound_ended: bool,
}

#[derive(Clone)]
struct Race(Arc<Watch<Steps>>);

impl Default for Race {
    fn default() -> Self {
        Race(Arc::new(Watch::new(Steps::default())))
    }
}

/// Records the execution's cancel reason when the reply stream is dropped, and lets the gate go.
struct Recorder {
    exec: ExecutionRef,
    race: Race,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let reason = self.exec.cancel_reason();
        self.race.0.modify(|steps| {
            steps.reason = Some(reason);
            steps.replied = true;
        });
    }
}

#[controller]
struct Ticks {
    race: Dep<Race>,
}

#[handlers]
impl Ticks {
    /// Yields `0` at once and `1` once the gate holds the caller's `cancel`, then waits.
    #[ulo_rpc::message("cancel_race.ticks")]
    async fn ticks(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, CallError>> {
        let recorder = Recorder { exec, race: (*self.race).clone() };
        stream::unfold((0u64, recorder), |(n, recorder)| async move {
            match n {
                0 => {}
                1 => recorder.race.0.wait_for(|steps| steps.held).await,
                _ => {
                    recorder.race.0.modify(|steps| steps.replied = true);
                    std::future::pending::<()>().await;
                }
            }
            Some((Ok(n), (n + 1, recorder)))
        })
    }
}

struct ServerRoot {
    race: Race,
}

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.race.clone());
        m.controller::<Ticks>();
    }
}

/// The server's link, holding each `cancel` a caller sent until the race's `replied` or
/// `waiting` is set or [`PATIENCE`] passes.
struct Gated<L> {
    link: L,
    race: Race,
    timer: Arc<dyn Timer>,
}

impl<L: Link> Link for Gated<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.link.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.link.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.link.max_inflight(calls);
    }

    async fn listen(&self, patterns: &[Subscription]) -> Result<Inbound, BoxError> {
        let inbound = self.link.listen(patterns).await?;
        let gate = (inbound, self.race.clone(), Arc::clone(&self.timer));
        Ok(Box::pin(stream::unfold(gate, |(mut inbound, race, timer)| async move {
            let Some(delivery) = inbound.next().await else {
                race.0.modify(|steps| steps.inbound_ended = true);
                return None;
            };
            let delivery = match delivery {
                Delivery { frame: frame @ (Frame::Req { .. } | Frame::Open { .. }), reply: Some(reply), ack } => {
                    Delivery { frame, reply: Some(observed(reply, race.clone())), ack }
                }
                other => other,
            };
            if matches!(delivery.frame, Frame::Cancel { .. }) && delivery.reply.is_some() {
                race.0.modify(|steps| steps.held = true);
                let released = race.0.wait_for(|steps| steps.replied || steps.waiting);
                select(pin!(released), timer.sleep(PATIENCE)).await;
            }
            Some((delivery, (inbound, race, timer)))
        })))
    }

    fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
        self.link.connect(runtime)
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.link.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}

/// `reply` with each send that waits, once the gate holds the caller's `cancel`, setting the
/// race's `waiting`: the send is made after the link read the `cancel` and before the server took
/// it.
fn observed(reply: ReplyPath, race: Race) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let mut sent = reply.send(frame);
        let race = race.clone();
        Box::pin(poll_fn(move |cx| {
            let polled = sent.as_mut().poll(cx);
            if polled.is_pending() && race.0.read(|steps| steps.held) {
                race.0.modify(|steps| steps.waiting = true);
            }
            polled
        }))
    })
}

async fn within<F: IntoFuture>(what: &str, fut: F) -> F::Output {
    match select(pin!(fut.into_future()), OnSmol::runtime().sleep(PATIENCE)).await {
        Either::Left((output, _)) => output,
        Either::Right(_) => panic!("{what} did not happen within {PATIENCE:?}"),
    }
}

#[test]
fn a_reply_sent_after_the_link_read_the_callers_cancel_leaves_the_call_client_cancelled() {
    OnSmol::block_on(run_a_reply_sent_after_the_link_read_the_callers_cancel_leaves_the_call_client_cancelled());
}

async fn run_a_reply_sent_after_the_link_read_the_callers_cancel_leaves_the_call_client_cancelled() {
    let race = Race::default();
    let link = Gated { link: Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))), race: race.clone(), timer: Arc::new(OnSmol::runtime()) };
    let server = App::builder(ServerRoot { race: race.clone() })
        .runtime(OnSmol::runtime())
        .drain_timeout(DRAIN)
        .wire()
        .expect("the server app wires")
        .connect()
        .await
        .expect("the server app connects")
        .bind(ulo_rpc::Server::new(link))
        .listen()
        .await
        .expect("the server listens");
    let addr = match server.addresses().as_slice() {
        [bound] => bound.addr,
        other => panic!("expected the server to report one bound address, got {other:?}"),
    };
    let handle = server.handle();
    let serving = ulo::spawn_with(&OnSmol::runtime(), async move {
        let _ = server.serve(std::future::pending::<Signal>()).await;
    });
    let rpc = RpcClient::new(Tcp::new(addr), Arc::new(OnSmol::runtime())).expect("the client takes the link");

    let mut ticks = within("the stream's opening", rpc.stream::<_, u64>(TICKS, &())).await.expect("the stream opens");
    match within("the first tick", ticks.next()).await {
        Some(Ok(0)) => {}
        other => panic!("expected the first tick, `0`, got {other:?}"),
    }
    drop(ticks);
    within("the reply stream's drop", race.0.wait_for(|steps| steps.reason.is_some())).await;
    let seen = race.0.read(|steps| (steps.reason, steps.waiting));
    assert_eq!(
        seen,
        (Some(Some(CancelReason::ClientCancelled)), true),
        "the call's cancel reason, its `cancel` raced by a reply, and whether that reply's send waited for the server to take the `cancel`"
    );

    // The client stays connected, so the inbound stream ends at the drain only once the link
    // holds no call, the cancelled one included; the drain window is far longer than the wait.
    let closing = ulo::spawn_with(&OnSmol::runtime(), async move { handle.close(Signal::new("test")).await });
    within("the inbound stream's end at the drain", race.0.wait_for(|steps| steps.inbound_ended)).await;
    drop(rpc);
    let _ = within("the server's close", closing).await;
    let _ = serving.await;
}
