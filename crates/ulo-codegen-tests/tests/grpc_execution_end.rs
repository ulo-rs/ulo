//! A gRPC call's execution ends when the server drops it with the reply body (DESIGN §3.8): a
//! streaming reply keeps its execution, its instances and its lookups, until the stream ends, and
//! the drain waits for it; an execution-scoped service holding its own `ExecutionRef` drops as the
//! reply finishes.
//!
//! The app's clock holds every sleep as long as the drain window, which the drain asks for as the
//! window's end less the runtime's clock, so a close that waited out the window would never return.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::Notify;
use ulo::{App, BoxFuture, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, Timer, injectable, routes};
use ulo_codegen_tests::probe::ends_client::EndsClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::Message;
use ulo_transport::CallError;

use support::{PlainHttp2, within};

const DRAIN: Duration = Duration::from_secs(10);

/// The runtime's clock, with every sleep within a second of [`DRAIN`] held for ever.
#[derive(Clone)]
struct Clock {
    runtime: ulo_runtime_tokio::Timer,
    windows: Arc<AtomicUsize>,
}

impl Timer for Clock {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        if d + Duration::from_secs(1) < DRAIN || d > DRAIN {
            return self.runtime.sleep(d);
        }
        self.windows.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }

    fn now(&self) -> Instant {
        self.runtime.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        let sleep = self.sleep(d);
        Box::pin(async move {
            sleep.await;
            Ok(())
        })
    }
}

#[derive(Default)]
struct Probe {
    dropped: AtomicUsize,
    exec: Mutex<Option<ExecutionRef>>,
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
struct Streams {
    audit: Dep<Audit>,
}

#[routes]
impl Streams {
    /// Tick 1, then, once the test opens the gate, tick 2 when a lookup through the instance's own
    /// handle answers that instance, 0 when the execution ended first.
    #[ulo_grpc::method(probe::ends::Written)]
    fn written(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> + use<> {
        let audit = self.audit.clone();
        *audit.probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner) = Some(audit.exec.clone());
        stream::iter(1..=2u32).then(move |n| {
            let audit = audit.clone();
            async move {
                if n == 1 {
                    return Ok(Tick { n });
                }
                audit.probe.0.waiting.notify_one();
                audit.probe.0.gate.notified().await;
                match audit.exec.get::<Audit>().await {
                    Ok(again) if Arc::ptr_eq(again.arc(), audit.arc()) => Ok(Tick { n }),
                    _ => Ok(Tick { n: 0 }),
                }
            }
        })
    }
}


#[injectable]
struct Leaving {
    audit: Dep<Audit>,
}

#[routes]
impl Leaving {
    /// Tick 1, then a wait for the cancellation the caller's leaving fires.
    #[ulo_grpc::method(probe::ends::Abandoned)]
    fn abandoned(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> + use<> {
        let audit = self.audit.clone();
        *audit.probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner) = Some(audit.exec.clone());
        stream::iter(1..=2u32).then(move |n| {
            let audit = audit.clone();
            async move {
                if n == 2 {
                    audit.probe.0.waiting.notify_one();
                    audit.exec.cancelled().await;
                }
                Ok(Tick { n })
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
        m.controller::<Streams>();
        m.controller::<Leaving>();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streaming_reply_keeps_its_execution_until_it_ends_and_the_drain_waits_for_it() {
    let probe = Shared::default();
    let clock = Clock { runtime: ulo_runtime_tokio::Timer::current(), windows: Arc::default() };
    let app = App::builder(Root(probe.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .drain_timeout(DRAIN)
        .wire()
        .unwrap_or_else(|error| panic!("the gRPC app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the gRPC app did not connect: {error}"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the gRPC app did not listen: {error}"));
    let addr = app.addresses().first().expect("one address").addr;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let http2: PlainHttp2 =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).http2_only(true).build_http::<tonic::body::Body>();
    let mut client = EndsClient::with_origin(http2, format!("http://{addr}").parse().expect("an origin"));

    let mut items = within("the call", client.written(Ticks::default())).await.expect("the call is answered").into_inner();
    let first = within("the first item", items.next()).await.expect("an item").expect("no error");
    assert_eq!(first.n, 1);
    within("the stream waiting at its gate", probe.0.waiting.notified()).await;
    assert_eq!(probe.0.dropped.load(Ordering::SeqCst), 0, "the instance lives while the reply runs");

    let closing = tokio::spawn({
        let handle = handle.clone();
        async move { handle.close(Signal::new("test")).await }
    });
    within("the drain waiting on its window", async {
        while clock.windows.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    assert!(!closing.is_finished(), "the drain waits for the open reply's execution");

    probe.0.gate.notify_one();
    let second = within("the second item", items.next()).await.expect("an item").expect("no error");
    assert_eq!(second.n, 2, "the reply's lookup after the handler returned answered its own instance");
    assert!(within("the reply's end", items.next()).await.is_none(), "the reply ended with an OK status");
    drop(items);

    let shutdown = within("the close", closing).await.expect("the close task").expect("a clean shutdown");
    assert_eq!(shutdown.abandoned, 0, "the reply's execution ended with the reply, inside the window");
    let _ = within("serve ending", serving).await;
    assert_eq!(probe.0.dropped.load(Ordering::SeqCst), 1, "the instance dropped with the reply");
    let exec = probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
    assert!(matches!(exec.get::<Audit>().await, Err(ulo::LookupError::ExecutionEnded { .. })), "the call's handle outlived it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_resetting_a_streaming_reply_ends_its_execution() {
    let probe = Shared::default();
    let clock = Clock { runtime: ulo_runtime_tokio::Timer::current(), windows: Arc::default() };
    let app = App::builder(Root(probe.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .drain_timeout(DRAIN)
        .wire()
        .unwrap_or_else(|error| panic!("the gRPC app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the gRPC app did not connect: {error}"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the gRPC app did not listen: {error}"));
    let addr = app.addresses().first().expect("one address").addr;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let http2: PlainHttp2 =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).http2_only(true).build_http::<tonic::body::Body>();
    let mut client = EndsClient::with_origin(http2, format!("http://{addr}").parse().expect("an origin"));

    let mut items = within("the call", client.abandoned(Ticks::default())).await.expect("the call is answered").into_inner();
    let first = within("the first item", items.next()).await.expect("an item").expect("no error");
    assert_eq!(first.n, 1);
    within("the stream waiting on its cancellation", probe.0.waiting.notified()).await;
    drop(items);
    within("the reply's `Audit` dropping once the caller reset the stream", async {
        while probe.0.dropped.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    let exec = probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
    assert_eq!(exec.cancel_reason(), Some(ulo::CancelReason::ClientCancelled));
    assert!(matches!(exec.get::<Audit>().await, Err(ulo::LookupError::ExecutionEnded { .. })));

    // The window's sleep never ends, so a close that returns waited for nothing.
    let shutdown = within("the close", handle.close(Signal::new("test"))).await.expect("a clean shutdown");
    assert_eq!(shutdown.abandoned, 0);
    let _ = within("serve ending", serving).await;
}
