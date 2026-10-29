use std::time::Duration;

use tokio::sync::Semaphore;
use ulo::{App, AppBuilder, Dep, ExecutionRef, injectable, module, routes};

/// How long in-flight executions get after the app stops accepting.
pub const DRAIN: Duration = Duration::from_secs(5);
/// The cap on the whole close sequence, counted from its trigger.
pub const SHUTDOWN_CAP: Duration = Duration::from_secs(25);

/// Lets `/work` be held until the caller says so; `entered` counts the handlers that have started.
pub struct Gate {
    pub entered: Semaphore,
    pub release: Semaphore,
}

#[injectable]
impl Gate {
    fn new() -> Self {
        Gate { entered: Semaphore::new(0), release: Semaphore::new(0) }
    }
}

#[injectable]
pub struct WorkController {
    gate: Dep<Gate>,
}

#[routes]
impl WorkController {
    /// Answers when released. A request still here when the drain window ends is abandoned.
    #[ulo_http::get("/work")]
    async fn work(&self) -> &'static str {
        self.gate.entered.add_permits(1);
        self.gate.release.acquire().await.expect("the gate is never closed").forget();
        "finished"
    }

    /// A handler that watches for the drain and ends its own work early.
    #[ulo_http::get("/watch")]
    async fn watch(&self, exec: ExecutionRef) -> &'static str {
        self.gate.entered.add_permits(1);
        exec.draining().await;
        "stopped early"
    }
}

#[module(providers = [Gate], controllers = [WorkController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule).drain_timeout(DRAIN).shutdown_timeout(SHUTDOWN_CAP)
}
