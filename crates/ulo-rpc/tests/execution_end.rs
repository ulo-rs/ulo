//! An RPC call's execution ends when its task drops it, once the reply is written (DESIGN §3.8): a
//! streamed reply keeps its execution and its lookups until its last item, and an
//! execution-scoped service holding its own `ExecutionRef` drops as the reply finishes, so the
//! drain has nothing to wait for. The drain window is longer than any wait here, so a close that
//! waited it out fails the test.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{Notify, mpsc};
use ulo::{App, BoxError, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, injectable, routes};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, Frame, Link, Outbound, Pattern, ReplyPath};
use ulo_transport::CallError;

const PATIENCE: Duration = Duration::from_secs(5);
const DRAIN: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Probe {
    dropped: AtomicUsize,
    waiting: Notify,
    gate: Notify,
}

#[derive(Clone, Default)]
struct Shared(Arc<Probe>);

#[injectable(execution)]
struct Audit {
    exec: ExecutionRef,
    probe: Dep<Shared>,
}

impl Drop for Audit {
    fn drop(&mut self) {
        self.probe.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[injectable]
struct Audited {
    audit: Dep<Audit>,
}

#[routes]
impl Audited {
    #[ulo_rpc::message("end.unary")]
    async fn unary(&self) -> u32 {
        u32::from(!self.audit.exec.is_cancelled())
    }

    /// 1, then, once the test opens the gate, 2 when a lookup through the instance's own handle
    /// answers that instance, 0 when the execution ended first.
    #[ulo_rpc::message("end.streamed")]
    async fn streamed(&self) -> impl Stream<Item = Result<u32, CallError>> + use<> {
        let audit = self.audit.clone();
        stream::iter(1..=2u32).then(move |n| {
            let audit = audit.clone();
            async move {
                if n == 1 {
                    return Ok(1);
                }
                audit.probe.0.waiting.notify_one();
                audit.probe.0.gate.notified().await;
                match audit.exec.get::<Audit>().await {
                    Ok(again) if Arc::ptr_eq(again.arc(), audit.arc()) => Ok(2),
                    _ => Ok(0),
                }
            }
        })
    }
}

struct Root(Shared);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.provide::<Audit>();
        m.controller::<Audited>();
    }
}

/// A link whose inbound stream the test feeds.
struct Scripted {
    inbound: Mutex<Option<Inbound>>,
}

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).miss_signal(true)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        self.inbound.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// Sends a call to `pattern` with `id`, and answers the channel its reply frames arrive on.
fn call(deliveries: &mpsc::UnboundedSender<Delivery>, id: u64, pattern: &str) -> mpsc::UnboundedReceiver<Frame> {
    let (frames, replies) = mpsc::unbounded_channel();
    let reply = ReplyPath::new(move |frame: Frame| {
        let sent = frames.send(frame).map_err(BoxError::from);
        Box::pin(async move { sent })
    });
    let frame = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    deliveries.send(Delivery { frame, reply: Some(reply), ack: Ack::none() }).expect("the server reads its link");
    replies
}

async fn frame(replies: &mut mpsc::UnboundedReceiver<Frame>) -> Frame {
    tokio::time::timeout(PATIENCE, replies.recv()).await.expect("a frame in time").expect("the reply path is open")
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let waited = async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(PATIENCE, waited).await.unwrap_or_else(|_| panic!("{what} within {PATIENCE:?}"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_streamed_reply_keeps_its_execution_until_its_last_item_and_finished_calls_leave_the_drain_nothing() {
    let probe = Shared::default();
    let (deliveries, mut receiver) = mpsc::unbounded_channel::<Delivery>();
    let inbound: Inbound = Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx)));
    let app = App::builder(Root(probe.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .drain_timeout(DRAIN)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Scripted { inbound: Mutex::new(Some(inbound)) }))
        .listen()
        .await
        .expect("the server binds");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });

    for id in 1..=3u64 {
        let mut replies = call(&deliveries, id, "end.unary");
        assert_eq!(frame(&mut replies).await, Frame::Res { id, data: Data::new(b"1".as_slice()) });
        until("the unary call's `Audit` dropping as its reply finished", || probe.0.dropped.load(Ordering::SeqCst) == id as usize).await;
    }

    let mut replies = call(&deliveries, 4, "end.streamed");
    assert_eq!(frame(&mut replies).await, Frame::Item { id: 4, data: Data::new(b"1".as_slice()) });
    tokio::time::timeout(PATIENCE, probe.0.waiting.notified()).await.expect("the stream reached its gate");
    assert_eq!(probe.0.dropped.load(Ordering::SeqCst), 3, "the streamed call's instance lives while its reply runs");
    probe.0.gate.notify_one();
    assert_eq!(
        frame(&mut replies).await,
        Frame::Item { id: 4, data: Data::new(b"2".as_slice()) },
        "the stream's lookup after the handler returned answered its own instance"
    );
    assert_eq!(frame(&mut replies).await, Frame::End { id: 4 });
    until("the streamed call's `Audit` dropping with its reply", || probe.0.dropped.load(Ordering::SeqCst) == 4).await;

    drop(deliveries);
    let shutdown = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test")))
        .await
        .expect("close returned without waiting out the drain window")
        .expect("a clean shutdown");
    assert_eq!((shutdown.abandoned, shutdown.terminal_skipped), (0, 0));
    let _ = serving.await;
}
