//! A caller's `cancel` is the call's reason, `ClientCancelled`, even when the server answers
//! between the link reading the `cancel` and the server taking it. The link writes a call nothing
//! once it reads the call's `cancel`; a send refused in that window would cancel the call
//! `Disconnected` before the `cancel` arrived.
//!
//! [`Gated`] wraps the server's link. It holds each caller's `cancel` on its way from the link to
//! the server, after the link has read it, until the server's send of the call's answer waits or
//! the call's execution has ended. The handler answers once the gate holds the `cancel`, so the
//! answer is sent inside the window on every run, and the execution's reason is read as it ends.
//!
//! Once the server has taken the `cancel`, the call holds the link no longer: the draining
//! server's inbound stream ends while the client is still connected.

use std::future::{Future, poll_fn};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use futures_util::future::{Either, select};
use ulo::{
    App, AppHandle, BoundAddr, BoxError, BoxFuture, CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, Timer,
    controller, handlers, injectable,
};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Delivery, Frame, Link, Outbound, Subscription, ReplyPath, RpcClient};
use ulo_rpc_udp::Udp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::sync::Watch;
use ulo_transport::{CallError, Count};

const ANSWER: &str = "cancel_race.answer";

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// The server's drain window, far longer than [`PATIENCE`].
const DRAIN: Duration = Duration::from_secs(3600);

/// What the gate, the handler and the test have seen, in the order they see it.
#[derive(Default)]
struct Steps {
    /// The call's execution, set as its handler starts.
    exec: Option<ExecutionRef>,
    /// The gate holds the caller's `cancel`, which the link has read.
    held: bool,
    /// The execution's cancel reason as it ended, the answer sent.
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

/// Built for each call's execution and dropped as the execution ends, once the server has sent
/// the call's answer: records the execution's cancel reason then.
#[injectable(execution)]
struct Ended {
    race: Dep<Race>,
}

impl Drop for Ended {
    fn drop(&mut self) {
        self.race.0.modify(|steps| steps.reason = Some(steps.exec.as_ref().and_then(ExecutionRef::cancel_reason)));
    }
}

#[controller]
struct Answers {
    race: Dep<Race>,
}

#[handlers]
impl Answers {
    /// Answers `1` once the gate holds the caller's `cancel`.
    #[ulo_rpc::message("cancel_race.answer")]
    async fn answer(&self, exec: ExecutionRef, _ended: Dep<Ended>) -> Result<u64, CallError> {
        self.race.0.modify(|steps| steps.exec = Some(exec));
        self.race.0.wait_for(|steps| steps.held).await;
        Ok(1)
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
        m.provide::<Ended>();
        m.controller::<Answers>();
    }
}

/// The server's link, holding each `cancel` a caller sent until the server's send of the call's
/// answer waits, the call's execution has ended, or [`PATIENCE`] passes.
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
                let released = race.0.wait_for(|steps| steps.reason.is_some() || steps.waiting);
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
    match select(pin!(fut.into_future()), pin!(tokio::time::sleep(PATIENCE))).await {
        Either::Left((output, _)) => output,
        Either::Right(_) => panic!("{what} did not happen within {PATIENCE:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_sent_after_the_link_read_the_callers_cancel_leaves_the_call_client_cancelled() {
    let race = Race::default();
    let link = Gated { link: Udp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))), race: race.clone(), timer: Arc::new(Tokio::current()) };
    let server = App::builder(ServerRoot { race: race.clone() })
        .runtime(Tokio::current())
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
    let serving = tokio::spawn(async move {
        let _ = server.serve(std::future::pending::<Signal>()).await;
    });
    let rpc = RpcClient::new(Udp::new(addr), Arc::new(Tokio::current())).expect("the client takes the link");

    {
        let call = rpc.request::<_, u64>(ANSWER, &()).into_future();
        let started = race.0.wait_for(|steps| steps.exec.is_some());
        match within("the handler's start", select(pin!(call), pin!(started))).await {
            Either::Left((answered, _)) => panic!("the call answered before its handler started: {answered:?}"),
            // The call is dropped here, which sends its `cancel`.
            Either::Right(_) => {}
        }
    }
    within("the execution's end", race.0.wait_for(|steps| steps.reason.is_some())).await;
    let reason = race.0.read(|steps| steps.reason);
    assert_eq!(reason, Some(Some(CancelReason::ClientCancelled)), "the call's cancel reason as it ended, its answer sent after the link read its `cancel`");

    // The client stays connected, so the inbound stream ends at the drain only once the link
    // holds no call, the cancelled one included; the drain window is far longer than the wait.
    let closing = tokio::spawn(async move { handle.close(Signal::new("test")).await });
    within("the inbound stream's end at the drain", race.0.wait_for(|steps| steps.inbound_ended)).await;
    drop(rpc);
    let _ = within("the server's close", closing).await;
    let _ = serving.await;
}
