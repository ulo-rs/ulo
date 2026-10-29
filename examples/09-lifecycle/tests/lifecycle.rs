use ex_09_lifecycle::{BACKOFF, Journal, JournalModule, builder};
use ulo::{App, BoxError, ConnectError, Dep, FailureReason, OnModuleInit, Signal, StartupError, injectable, module};
use tokio::time::Instant;
use ulo_runtime_tokio::Tokio;

/// The app's clock is tokio's, so a paused test clock moves it: a backoff passes when the runtime
/// has nothing else to do, and no real time is spent waiting for it.
#[tokio::test(start_paused = true)]
async fn hooks_run_in_dependency_order_and_in_reverse_at_close() {
    let started = Instant::now();
    let app = builder(2).runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();
    let journal = app.get::<Journal>().await.unwrap();

    let shutdown = app.close(Signal::new("done")).await.unwrap();
    assert_eq!(shutdown.signal.name(), "done");

    assert_eq!(
        journal.lines(),
        [
            "Link ready: Err(\"link refused attempt 1\")",
            "Link ready: Err(\"link refused attempt 2\")",
            "Link ready: Ok(3)",
            "Cache built",
            "LinkModule init",
            "Cache init",
            "Cache bootstrap",
            "Cache before shutdown (done)",
            "Cache destroy",
            "Link destroy",
            "Cache shutdown (done)",
        ]
    );
    assert_eq!(started.elapsed(), 2 * BACKOFF, "two refused attempts waited one backoff each");
}

#[tokio::test(start_paused = true)]
async fn a_readiness_check_that_never_passes_fails_connect_with_the_key_attempts_and_reason() {
    let started = Instant::now();
    let Err(error) = builder(99).runtime(Tokio::current()).wire().unwrap().connect().await else {
        panic!("connect should fail");
    };
    let StartupError::Connect(ConnectError::Readiness { key, attempts, reason }) = &error else {
        panic!("expected a readiness failure, got {error}");
    };
    assert_eq!(key.to_string(), "Link");
    assert_eq!(*attempts, 4, "the first attempt and three retries");
    let FailureReason::Errored(last) = reason else { panic!("expected the last attempt's error, got {reason}") };
    assert_eq!(last.to_string(), "link refused attempt 4");
    assert_eq!(started.elapsed(), 3 * BACKOFF);
}

/// Registered dependent-first, so only dependency order puts `Store` before `Report`.
struct Report {
    journal: Dep<Journal>,
}

#[injectable]
impl Report {
    fn new(_store: Dep<Store>, journal: Dep<Journal>) -> Self {
        Report { journal }
    }
}

impl OnModuleInit for Report {
    async fn on_module_init(&self) -> Result<(), BoxError> {
        self.journal.push("Report init");
        Ok(())
    }
}

#[injectable]
struct Store {
    journal: Dep<Journal>,
}

impl OnModuleInit for Store {
    async fn on_module_init(&self) -> Result<(), BoxError> {
        self.journal.push("Store init");
        Ok(())
    }
}

#[module(imports = [JournalModule], providers = [Report, Store])]
struct WrittenBackwards;

#[tokio::test]
async fn init_hooks_follow_dependencies_not_the_order_written() {
    let app = App::builder(WrittenBackwards).runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();
    let journal = app.get::<Journal>().await.unwrap();
    assert_eq!(journal.lines(), ["Store init", "Report init"]);
    app.close(Signal::new("done")).await.unwrap();
}
