use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ulo::{
    App, AppBuilder, BeforeApplicationShutdown, BoxError, Dep, Module, ModuleDef, ModuleIdentity, OnApplicationBootstrap,
    OnApplicationShutdown, OnModuleDestroy, OnModuleInit, Signal, injectable, module,
};

/// One line per lifecycle event, in the order they happen.
#[derive(Default)]
pub struct Journal(Mutex<Vec<String>>);

impl Journal {
    pub fn push(&self, line: impl Into<String>) {
        self.0.lock().unwrap().push(line.into());
    }

    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

#[module(global, providers = [Journal::default()], exports = [Journal])]
pub struct JournalModule;

/// A dependency that refuses its first `fail_first` pings.
pub struct Link {
    fail_first: u32,
    attempts: AtomicU32,
}

impl Link {
    fn ping(&self, journal: &Journal) -> Result<(), String> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let pinged = if attempt > self.fail_first { Ok(attempt) } else { Err(format!("link refused attempt {attempt}")) };
        journal.push(format!("Link ready: {pinged:?}"));
        pinged.map(drop)
    }
}

pub const BACKOFF: Duration = Duration::from_millis(500);

/// Closure hooks and the readiness check hang off `ModuleDef`; `#[module]` has no spelling for them.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LinkModule {
    pub fail_first: u32,
}

impl Module for LinkModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let fail_first = self.fail_first;
        m.singleton(move || async move { Link { fail_first, attempts: AtomicU32::new(0) } })
            .ready(|link: Dep<Link>, journal: Dep<Journal>| async move { link.ping(&journal) })
            .retries(3)
            .backoff(BACKOFF)
            .on_destroy(|journal: Dep<Journal>| async move { journal.push("Link destroy") });
        m.on_init(|journal: Dep<Journal>| async move {
            journal.push("LinkModule init");
            Ok::<(), BoxError>(())
        });
        m.export::<Link>();
    }
}

/// Implements every hook trait; `#[injectable]` registers whichever are implemented.
pub struct Cache {
    journal: Dep<Journal>,
}

#[injectable]
impl Cache {
    fn new(_link: Dep<Link>, journal: Dep<Journal>) -> Self {
        journal.push("Cache built");
        Cache { journal }
    }
}

impl OnModuleInit for Cache {
    async fn on_module_init(&self) -> Result<(), BoxError> {
        self.journal.push("Cache init");
        Ok(())
    }
}

impl OnApplicationBootstrap for Cache {
    async fn on_application_bootstrap(&self) -> Result<(), BoxError> {
        self.journal.push("Cache bootstrap");
        Ok(())
    }
}

impl BeforeApplicationShutdown for Cache {
    async fn before_application_shutdown(&self, signal: &Signal) {
        self.journal.push(format!("Cache before shutdown ({signal})"));
    }
}

impl OnModuleDestroy for Cache {
    async fn on_module_destroy(&self) {
        self.journal.push("Cache destroy");
    }
}

impl OnApplicationShutdown for Cache {
    async fn on_application_shutdown(&self, signal: &Signal) {
        self.journal.push(format!("Cache shutdown ({signal})"));
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[module(imports = [JournalModule, LinkModule { fail_first: self.fail_first }], providers = [Cache])]
pub struct AppModule {
    pub fail_first: u32,
}

pub fn builder(fail_first: u32) -> AppBuilder {
    App::builder(AppModule { fail_first })
}
