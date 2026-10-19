//! An embedding dropped without `close`, with the host's server future installed through
//! `Handle::host` and never taken by `serve`, lets go of that future at the app's drop: no task is
//! left on the runtime, and the app's singletons drop with the last handle (F433).
//!
//! rocket binds its listener when `launch` is first polled, so an ignited rocket installed and
//! never served holds no socket: the test reads the singletons and the runtime's tasks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo::{BoxError, BoxFuture};
use ulo_http_rocket::{Embedded, Handle};
use ulo_runtime_tokio::Tokio;

/// How long the drop has to show on the runtime.
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

/// What the test requires once the app has dropped: no task beyond `before`, and the singletons
/// alive until `embedded` drops.
async fn assert_released(before: usize, singleton: &AtomicBool, embedded: Handle) {
    let released = Instant::now();
    while alive_tasks() != before {
        assert!(released.elapsed() < PATIENCE, "a task outlived the app's drop by {PATIENCE:?}");
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
    let figment = rocket::Config::figment().merge(("port", 0)).merge(("shutdown.ctrlc", false)).merge(("log_level", "off"));
    let ignited = rocket::custom(figment).mount("/", ulo_http_rocket::routes(&embedded)).ignite().await.expect("rocket ignites");
    let host: BoxFuture<'static, Result<(), BoxError>> =
        Box::pin(async move { ignited.launch().await.map(drop).map_err(|error| BoxError::from(error.to_string())) });
    embedded.host(host).expect("the host's future is installed");
    drop(app);
    assert_released(before, &singleton, embedded).await;
}
