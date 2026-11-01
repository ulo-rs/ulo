//! A WebSocket message's execution ends when the connection's task drops it, once its answer is
//! written (DESIGN §3.8): a streamed answer keeps its execution and its lookups until its last
//! item, and an execution-scoped service holding its own `ExecutionRef` drops as each answer
//! finishes.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::{Stream, StreamExt, stream};
use serde_json::json;
use tokio::sync::Notify;
use ulo::{Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_transport::CallError;
use ulo_ws::WsModule;

use support::{Running, hang_up, next_json, send_json, within};

#[derive(Default)]
struct Probe {
    dropped: AtomicUsize,
    waiting: Notify,
    gate: Notify,
}

#[derive(Clone, Default)]
struct Shared(Arc<Probe>);

impl Shared {
    fn dropped(&self) -> usize {
        self.0.dropped.load(Ordering::SeqCst)
    }
}

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
#[ulo_ws::gateway(path = "/audited")]
impl Audited {
    #[ulo_ws::message("once")]
    fn once(&self) -> u32 {
        u32::from(!self.audit.exec.is_cancelled())
    }

    /// 1, then, once the test opens the gate, 2 when a lookup through the instance's own handle
    /// answers that instance, 0 when the execution ended first.
    #[ulo_ws::message("streamed")]
    fn streamed(&self) -> impl Stream<Item = Result<u32, CallError>> + use<> {
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
        m.import(WsModule::for_root());
        m.value(self.0.clone());
        m.provide::<Audit>();
        m.controller::<Audited>();
    }
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    within(what, async {
        while !ready() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streamed_answer_keeps_its_execution_until_its_last_item() {
    let probe = Shared::default();
    let app = Running::start(Root(probe.clone()), ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/audited", &[]).await;

    for id in 1..=3 {
        send_json(&mut socket, &json!({ "event": "once", "id": id, "data": null })).await;
        assert_eq!(next_json(&mut socket).await, json!({ "id": id, "data": 1 }));
        until("the message's `Audit` dropping as its answer finished", || probe.dropped() == id).await;
    }

    send_json(&mut socket, &json!({ "event": "streamed", "id": 4, "data": null })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 4, "data": 1 }));
    within("the stream reaching its gate", probe.0.waiting.notified()).await;
    assert_eq!(probe.dropped(), 3, "the streamed message's instance lives while its answer runs");
    probe.0.gate.notify_one();
    assert_eq!(next_json(&mut socket).await, json!({ "id": 4, "data": 2 }), "the stream's lookup answered its own instance");
    assert_eq!(next_json(&mut socket).await, json!({ "id": 4, "complete": true }));
    until("the streamed message's `Audit` dropping with its answer", || probe.dropped() == 4).await;

    hang_up(socket).await;
    app.stop().await;
}
