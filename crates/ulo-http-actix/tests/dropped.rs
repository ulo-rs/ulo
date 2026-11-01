//! An embedding dropped without `close`, with the host's server future installed by `run` and
//! never taken by `serve`, `run`'s future dropped unpolled, lets go of that future at the app's
//! drop: the host's listener closes, no task is left on the runtime, and the app's singletons drop
//! with the last handle (F433).

use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo::Signal;
use ulo_http_actix::{Embedded, Handle};
use ulo_runtime_tokio::Tokio;

/// How long the drop has to show at the host's address.
const PATIENCE: Duration = Duration::from_secs(5);

/// Sets its flag when it drops.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
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

/// Whether a connection to `addr` is refused: a listener still bound accepts it into its backlog.
fn refuses(addr: SocketAddr) -> bool {
    matches!(TcpStream::connect_timeout(&addr, Duration::from_millis(200)), Err(error) if error.kind() == ErrorKind::ConnectionRefused)
}

fn alive_tasks() -> usize {
    tokio::runtime::Handle::current().metrics().num_alive_tasks()
}

/// The embedding bound in an app over [`Root`].
async fn bound(server: Embedded, singleton: &Arc<AtomicBool>) -> App<ulo::app::Bound> {
    App::builder(Root(Arc::clone(singleton)))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the embedding binds")
}

/// What the test requires once the app has dropped: the host's listener refusing at `addr`, no
/// task beyond `before`, and the singletons alive until `embedded` drops.
async fn assert_released(addr: SocketAddr, before: usize, singleton: &AtomicBool, embedded: Handle) {
    let released = Instant::now();
    while !refuses(addr) {
        assert!(released.elapsed() < PATIENCE, "the host's listener still accepts {PATIENCE:?} after the app's drop");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(alive_tasks(), before, "a task outlived the app's drop");
    assert!(embedded.host(std::future::ready(Ok::<(), std::io::Error>(()))).is_err(), "a host installed after the drop was taken");
    assert!(!singleton.load(Ordering::SeqCst), "the app's singletons dropped while the embedding's handle was held");
    drop(embedded);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived the embedding's last handle");
}

#[tokio::test]
async fn an_embedding_dropped_with_its_host_installed_and_unserved_releases_the_host() {
    let before = alive_tasks();
    let singleton = Arc::new(AtomicBool::new(false));
    let server = Embedded::new();
    let embedded = server.handle();
    let app = bound(server, &singleton).await;
    let host = {
        let embedded = embedded.clone();
        actix_web::HttpServer::new(move || actix_web::App::new().default_service(ulo_http_actix::scope("", &embedded)))
            .workers(1)
            .bind(("127.0.0.1", 0))
            .expect("the host's listener binds")
    };
    let addr = host.addrs()[0];
    let running = ulo_http_actix::run(app, &embedded, host, async { Signal::new("test") });
    assert!(!refuses(addr), "the host's listener refused a connection before the app's drop");
    drop(running);
    assert_released(addr, before, &singleton, embedded).await;
}
