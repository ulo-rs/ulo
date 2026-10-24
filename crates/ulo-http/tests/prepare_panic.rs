//! An upgrade handler whose `prepare` panics inside the HTTP server's `prepare` (core §9.5,
//! transports §3.5): `listen()` reports it as the server's `Configure` entry, and every handler
//! whose `prepare` ran is released as the server drops, so a handler holding the app's handle, as
//! the WebSocket hand-off does, lets go of the app and its singletons are freed.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ulo::{App, AppHandle, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, PreparePanicked, StartupError, TypeName};
use ulo_http::{AppService, Backend, BackendLimits, Http, HttpConfig, Request, Response, UpgradeHandler};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;

const PATIENCE: Duration = Duration::from_secs(10);

/// Binds nothing and serves nothing.
struct Idle;

impl Backend for Idle {
    const NAME: &'static str = "idle";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(&mut self, _listeners: Vec<BoundListener>, _tls: Option<Arc<ServerConfig>>, _svc: AppService, _cfg: &HttpConfig) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// What a hand-off keeps between `prepare` and its last `release`: how many servers hold it, and
/// the app's handle, which holds the app from inside the app's own graph until released.
#[derive(Default)]
struct Slot {
    holders: usize,
    app: Option<AppHandle>,
}

/// An upgrade handler kept the way the WebSocket hand-off is kept.
struct Holder(Arc<Mutex<Slot>>);

impl UpgradeHandler for Holder {
    fn paths(&self, _app: &AppHandle) -> Vec<Cow<'static, str>> {
        Vec::new()
    }

    fn upgrade(&self, _req: Request) -> BoxFuture<'static, Response> {
        Box::pin(async { unreachable!("no path is taken") })
    }

    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        slot.holders += 1;
        slot.app = Some(app.clone());
        Ok(())
    }

    fn release(&self) {
        let app = {
            let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            slot.holders = slot.holders.saturating_sub(1);
            if slot.holders == 0 { slot.app.take() } else { None }
        };
        drop(app);
    }
}

/// An upgrade handler whose `prepare` panics.
struct Panics;

impl UpgradeHandler for Panics {
    fn paths(&self, _app: &AppHandle) -> Vec<Cow<'static, str>> {
        Vec::new()
    }

    fn upgrade(&self, _req: Request) -> BoxFuture<'static, Response> {
        Box::pin(async { unreachable!("no path is taken") })
    }

    fn prepare(&self, _app: &AppHandle) -> Result<(), BoxError> {
        panic!("the gateway table exploded");
    }
}

/// Sets its flag as it drops: the app's singleton store dropping with the app.
struct Stored(Arc<AtomicBool>);

impl Drop for Stored {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Root {
    slot: Arc<Mutex<Slot>>,
    dropped: Arc<AtomicBool>,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let dropped = Arc::clone(&self.dropped);
        m.singleton(move || {
            let dropped = Arc::clone(&dropped);
            async move { Stored(dropped) }
        });
        // The holder first, so its `prepare` has run when the next one panics.
        m.meta::<ulo_http::Upgrades>().register(Holder(Arc::clone(&self.slot))).register(Panics);
    }
}

#[tokio::test]
async fn a_panicking_upgrade_prepare_is_a_configure_entry_and_the_handlers_already_prepared_are_released() {
    let slot = Arc::new(Mutex::new(Slot::default()));
    let dropped = Arc::new(AtomicBool::new(false));
    let app = App::builder(Root { slot: Arc::clone(&slot), dropped: Arc::clone(&dropped) })
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http::Server::with_backend("127.0.0.1:0", Idle));
    let error = match tokio::time::timeout(PATIENCE, app.listen()).await.expect("listen finished in time") {
        Err(error) => error,
        Ok(_) => panic!("listen() accepted an app whose upgrade handler panicked"),
    };

    let StartupError::Configure(errors) = &error else { panic!("expected configuration errors, got: {error}") };
    assert_eq!(errors.len(), 1, "{errors}");
    let entry = errors.iter().next().expect("one entry");
    assert_eq!(entry.transport, TypeName::of::<Http>(), "the HTTP server's entry");
    let panicked = entry.source.downcast_ref::<PreparePanicked>().expect("the entry carries the panic");
    assert_eq!(panicked.message.to_string(), "the gateway table exploded");
    drop(error);

    let slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(slot.holders, 0, "the server's drop released the handler whose `prepare` ran");
    assert!(slot.app.is_none(), "the handler let go of the app's handle");
    drop(slot);
    assert!(dropped.load(Ordering::SeqCst), "the singleton store dropped with the app once `listen` failed");
}
