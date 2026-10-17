//! A server's link outlives neither its closed app nor `serve`: the TCP link, served through an
//! app and closed, drops once `serve` has returned (F425).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ulo::{App, AppHandle, BoundAddr, BoxError, Module, ModuleDef, ModuleIdentity, Runtime, Signal};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Link, Outbound, Pattern};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::Count;

/// Sets its flag when it drops.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// The TCP link, setting a flag when it drops.
struct Watched {
    tcp: Tcp,
    _dropped: DropFlag,
}

impl Link for Watched {
    const NAME: &'static str = "tcp";

    fn capabilities(&self) -> Capabilities {
        self.tcp.capabilities()
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        self.tcp.prepare(app).await
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.tcp.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.tcp.max_inflight(calls);
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        self.tcp.listen(patterns).await
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.tcp.connect(runtime).await
    }

    async fn drain(&self) {
        self.tcp.drain().await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.tcp.close().await
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.tcp.bound()
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

#[tokio::test]
async fn a_closed_app_drops_its_server_s_link() {
    let dropped = Arc::new(AtomicBool::new(false));
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Watched { tcp: Tcp::new("127.0.0.1:0"), _dropped: DropFlag(Arc::clone(&dropped)) }))
        .listen()
        .await
        .expect("the server binds");
    let handle = app.handle();
    app.serve(async { Signal::new("test") }).await.expect("the app shuts down");
    assert!(dropped.load(Ordering::SeqCst), "the server's link outlived the closed app's `serve`");
    drop(handle);
}
