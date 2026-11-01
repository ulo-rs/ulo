//! A server's link outlives neither its closed app nor `serve`: the TCP link, served through an
//! app and closed, drops once `serve` has returned (F425), and bound through an app dropped
//! unserved, drops with the app's value (F426), its address then refusing connections (F428).

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// How long the link's drop has to show at its address.
const PATIENCE: Duration = Duration::from_secs(5);

/// Whether a connection to `addr` is refused. An open listener accepts it; a closed one answers
/// with a reset, which reads as `ConnectionRefused`.
fn refuses(addr: SocketAddr) -> bool {
    matches!(TcpStream::connect_timeout(&addr, Duration::from_millis(200)), Err(error) if error.kind() == ErrorKind::ConnectionRefused)
}

/// A module whose one singleton sets its flag when it drops.
struct Root(Arc<AtomicBool>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(DropFlag(Arc::clone(&self.0)));
    }
}

#[tokio::test]
async fn a_closed_app_drops_its_server_s_link() {
    let dropped = Arc::new(AtomicBool::new(false));
    let singleton = Arc::new(AtomicBool::new(false));
    let app = App::builder(Root(Arc::clone(&singleton)))
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
    assert!(!singleton.load(Ordering::SeqCst), "the app's singletons dropped while a handle was held");
    drop(handle);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived its last handle");
}

#[tokio::test]
async fn a_bound_app_dropped_unserved_drops_its_server_s_link() {
    let dropped = Arc::new(AtomicBool::new(false));
    let singleton = Arc::new(AtomicBool::new(false));
    let app = App::builder(Root(Arc::clone(&singleton)))
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
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    assert!(!refuses(addr), "the bound server's address refuses connections");
    let handle = app.handle();
    drop(app);
    assert!(dropped.load(Ordering::SeqCst), "the server's link outlived the app dropped unserved");
    // The link's drop aborts its accept loop, which ends once this thread's runtime polls it.
    let released = Instant::now();
    while !refuses(addr) {
        assert!(released.elapsed() < PATIENCE, "the address still accepts connections {PATIENCE:?} after the app's drop");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!singleton.load(Ordering::SeqCst), "the app's singletons dropped while a handle was held");
    drop(handle);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived its last handle");
}
