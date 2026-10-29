use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::Semaphore;
use ulo::{BoxFuture, Dep, injectable, module};
use ulo_health::{Check, Health};

/// The notes every transport writes to. It starts closed: the check below reports that until
/// `open` runs, which `main` does once the app listens.
#[injectable]
pub struct Store {
    #[injectable(default)]
    notes: Mutex<Vec<String>>,
    #[injectable(default)]
    open: AtomicBool,
}

impl Store {
    pub fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }

    /// Records `text` as written through `via`, and returns how many notes there are.
    pub fn add(&self, via: &str, text: &str) -> u32 {
        let mut notes = self.notes.lock().unwrap_or_else(PoisonError::into_inner);
        notes.push(format!("{via}: {text}"));
        u32::try_from(notes.len()).unwrap_or(u32::MAX)
    }

    pub fn notes(&self) -> Vec<String> {
        self.notes.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Debug)]
struct Closed;

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the store is closed")
    }
}

impl std::error::Error for Closed {}

#[injectable]
pub struct StoreCheck {
    store: Dep<Store>,
}

impl Check for StoreCheck {
    fn name(&self) -> &str {
        "store"
    }

    fn check(&self) -> BoxFuture<'_, Health> {
        let open = self.store.open.load(Ordering::SeqCst);
        Box::pin(async move { if open { Health::Up } else { Health::down(Closed) } })
    }
}

/// Holds the `hold` handler of each transport until the caller says so; `entered` counts the
/// handlers that have started.
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

impl Gate {
    pub async fn hold(&self) {
        self.entered.add_permits(1);
        self.release.acquire().await.expect("the gate is never closed").forget();
    }
}

#[module(providers = [Store, Gate, into dyn Check: [StoreCheck]], exports = [Store, Gate])]
pub struct StoreModule;
