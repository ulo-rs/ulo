//! Startup failures come back as typed values, one variant per phase (section 10.2).
//!
//! Capabilities pinned: 45 (nothing panics or exits: `wire`, `connect` and `listen` each answer a
//! `StartupError` variant a caller matches, never text) and the listen rows of section 3:
//! `prepare` runs on every server and its failures are collected before any server binds, a
//! `prepare` that panics is one of those failures, and a bind failure closes the servers already
//! bound. `is_permanent` is read here by phase.
//!
//! Panics in a constructor, a hook and a readiness attempt reach the caller as
//! `FailureReason::Panicked` inside `Connect`; those are in `construct_errors.rs`, `hooks.rs` and
//! `readiness.rs`. `RuntimeMissing` is pinned by `ulo-runtime-tokio/tests/app.rs`.

#[macro_use]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ulo::{App, ConnectError, Dep, Permanent, PreparePanicked, Signal, StartupError, TypeName};
use ulo_runtime_tokio::Tokio;

use common::*;

struct A;
struct S1;
struct Missing;

fn builder(root: impl ulo::Module) -> ulo::AppBuilder {
    App::builder(root).runtime(Tokio::current())
}

async fn listen_err(
    builder: ulo::AppBuilder,
    servers: Vec<Queued>,
) -> StartupError {
    match listened(builder, servers).await {
        Err(error) => error,
        Ok(_) => panic!("listen() succeeded; the servers were expected to fail it"),
    }
}

#[tokio::test]
async fn a_wiring_fault_is_a_wiring_startup_error() {
    watchdog();
    let outcome = builder(Mod::<A>::new(|m| {
        m.singleton(|_missing: Dep<Missing>| async { S1 });
    }))
    .wire();
    assert!(matches!(outcome, Err(StartupError::Wiring(_))), "wire() answered something else");
}

#[tokio::test]
async fn a_construction_fault_is_a_connect_startup_error() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.try_singleton(|| async { Err::<S1, _>(Boom::new("down")) });
    }))
    .await;
    assert!(matches!(error, StartupError::Connect(ConnectError::Construct { .. })), "got: {error}");
}

#[tokio::test]
async fn every_prepare_failure_is_collected_before_any_server_binds() {
    let log = Log::new();
    let error = listen_err(
        builder(Empty),
        vec![
            server(TestServer::<TA>::new("a", &log).fail_prepare()),
            server(TestServer::<TB>::new("b", &log).fail_prepare()),
        ],
    )
    .await;

    let StartupError::Configure(errors) = error else { panic!("expected configuration errors, got: {error}") };
    let transports: Vec<TypeName> = errors.iter().map(|e| e.transport).collect();
    assert_eq!(transports, [TypeName::of::<TA>(), TypeName::of::<TB>()], "one entry per failing server, in bind order");
    assert!(errors.iter().all(|e| e.source.downcast_ref::<Boom>().is_some()), "each server's own error is reachable");
    assert_eq!(log.events(), ["prepare a", "prepare b"], "no server bound");
}

#[tokio::test]
async fn a_configuration_error_is_reported_instead_of_a_port_conflict() {
    let log = Log::new();
    let error = listen_err(
        builder(Empty),
        vec![
            server(TestServer::<TA>::new("a", &log).fail_bind()),
            server(TestServer::<TB>::new("b", &log).fail_prepare()),
        ],
    )
    .await;

    let StartupError::Configure(errors) = error else { panic!("expected configuration errors, got: {error}") };
    assert_eq!(errors.len(), 1, "only the server whose `prepare` failed is reported");
    assert!(!log.has("bind a"), "the server that would have failed to bind was never asked to: {:?}", log.events());
}

#[tokio::test]
async fn a_bind_failure_closes_the_servers_already_bound_and_is_a_bind_startup_error() {
    let log = Log::new();
    let error = listen_err(
        builder(Empty),
        vec![
            server(TestServer::<TA>::new("a", &log)),
            server(TestServer::<TB>::new("b", &log).fail_bind()),
            server(TestServer::<ulo_dummy::TC>::new("c", &log)),
        ],
    )
    .await;

    let StartupError::Bind { transport, source } = error else { panic!("expected a bind failure, got: {error}") };
    assert_eq!(transport, TypeName::of::<TB>(), "the error names the transport that failed");
    assert!(source.downcast_ref::<Boom>().is_some(), "the transport's own error is reachable");
    assert_eq!(
        log.events(),
        ["prepare a", "prepare b", "prepare c", "bind a", "bind b", "close a"],
        "every server prepared, binding stopped at the failure, and the one already bound was closed"
    );
}

mod ulo_dummy {
    pub struct TC;

    impl ulo::Transport for TC {
        const KEY: &'static str = "c";
        type Cx = ();
        type Reply = ();
    }
}

#[tokio::test]
async fn permanence_follows_the_error_and_the_phase() {
    // Wiring is always permanent: a restart reads the same graph.
    watchdog();
    let wiring = builder(Mod::<A>::new(|m| {
        m.singleton(|_missing: Dep<Missing>| async { S1 });
    }))
    .wire()
    .err()
    .expect("the graph wired");
    assert!(ulo::is_permanent(&wiring));

    // Connect: by what the failing item carries.
    let plain = connect_err(Mod::<A>::new(|m| {
        m.try_singleton(|| async { Err::<S1, _>(Boom::new("down")) });
    }))
    .await;
    assert!(!ulo::is_permanent(&plain), "a constructor's plain error is retryable");
    let permanent = connect_err(Mod::<A>::new(|m| {
        m.try_singleton(|| async { Err::<S1, _>(Permanent::new(Boom::new("bad credentials"))) });
    }))
    .await;
    assert!(ulo::is_permanent(&permanent), "a constructor's `Permanent` is not");
    let panicked = connect_err(Mod::<A>::new(|m| {
        m.singleton(|| async {
            if true {
                panic!("the factory panics");
            }
            S1
        });
    }))
    .await;
    assert!(!ulo::is_permanent(&panicked), "a panic is retryable");

    // Configure and Bind: by the server's error.
    let log = Log::new();
    for (permanent, expected) in [(false, false), (true, true)] {
        let mut prepared = TestServer::<TA>::new("a", &log).fail_prepare();
        let mut bound = TestServer::<TA>::new("a", &log).fail_bind();
        if permanent {
            prepared = prepared.permanent();
            bound = bound.permanent();
        }
        let configure = listen_err(builder(Empty), vec![server(prepared)]).await;
        assert!(matches!(configure, StartupError::Configure(_)));
        assert_eq!(ulo::is_permanent(&configure), expected, "a server's `prepare` failure, permanent: {permanent}");
        let bind = listen_err(builder(Empty), vec![server(bound)]).await;
        assert!(matches!(bind, StartupError::Bind { .. }));
        assert_eq!(ulo::is_permanent(&bind), expected, "a server's `bind` failure, permanent: {permanent}");
    }

    // A transport on an app with no runtime is a configuration the app cannot mend by restarting.
    let runtimeless = listen_err(App::builder(Empty), vec![server(TestServer::<TA>::new("a", &log))]).await;
    assert!(matches!(runtimeless, StartupError::Bind { .. }));
    assert!(ulo::is_permanent(&runtimeless), "`RuntimeMissing` is permanent");
}

#[tokio::test]
async fn an_app_that_listened_closes_cleanly() {
    let log = Log::new();
    let app = listened(builder(Empty), vec![server(TestServer::<TA>::new("a", &log))])
        .await
        .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let report = within("serve", app.serve(async { Signal::new("done") })).await.expect("shutdown reported a failure");
    assert_eq!(report.signal.name(), "done");
    assert_eq!(log.events(), ["prepare a", "bind a", "drain a", "close a"]);
}

/// Sets its flag when it drops: the app's singleton store dropping with the app's shared state.
struct Stored(Arc<AtomicBool>);

impl Drop for Stored {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A server whose `prepare` keeps the app's handle, as every shipped transport's does, and then
/// panics.
struct PanicsInPrepare {
    app: Option<ulo::AppHandle>,
    log: Log,
}

impl ulo::Server for PanicsInPrepare {
    type Transport = TA;

    async fn prepare(&mut self, mounted: ulo::Mounted<'_, TA>) -> Result<(), ulo::BoxError> {
        self.log.push("prepare panics");
        self.app = Some(mounted.app().clone());
        panic!("the route table exploded");
    }

    async fn bind(&mut self, _mounted: ulo::Mounted<'_, TA>) -> Result<(), ulo::BoxError> {
        self.log.push("bind panics");
        Ok(())
    }

    async fn serve(&self) -> Result<(), ulo::BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: ulo::DrainToken) {}

    async fn close(&self) -> Result<(), ulo::BoxError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_prepare_that_panics_is_a_configure_entry_naming_its_transport_and_the_app_is_freed() {
    let log = Log::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let root = Mod::<A>::new({
        let dropped = Arc::clone(&dropped);
        move |m| {
            let dropped = Arc::clone(&dropped);
            m.secret(&ulo::Secret::new("exploded".to_owned()));
            m.singleton(move || {
                let dropped = Arc::clone(&dropped);
                async move { Stored(dropped) }
            });
        }
    });
    let error = within(
        "listen",
        listen_err(
            builder(root),
            // The panicking server second, so an entry naming the first server is told apart.
            vec![
                server(TestServer::<TB>::new("b", &log)),
                server(PanicsInPrepare { app: None, log: log.clone() }),
            ],
        ),
    )
    .await;

    let StartupError::Configure(errors) = &error else { panic!("expected configuration errors, got: {error}") };
    assert_eq!(errors.len(), 1, "{errors}");
    let entry = errors.iter().next().expect("one entry");
    assert_eq!(entry.transport, TypeName::of::<TA>(), "the entry names the panicking server's transport");
    let panicked = entry.source.downcast_ref::<PreparePanicked>().expect("the entry carries the panic");
    assert_eq!(panicked.message.to_string(), "the route table [redacted]", "the payload is redacted");
    assert_eq!(log.events(), ["prepare b", "prepare panics"], "every server prepared, and none bound");
    drop(error);
    assert!(dropped.load(Ordering::SeqCst), "the singleton store dropped with the app once `listen` failed");
}

/// `is_panic` finds a panicking `prepare` in the report `listen()` answers, whether it is handed the
/// `StartupError` or the `ConfigureErrors` inside it, and finds it beside another server's plain
/// failure; a report of plain failures alone is not a panic.
#[tokio::test]
async fn is_panic_finds_a_panicking_prepare_in_the_configure_report() {
    let log = Log::new();
    let panicked = || PanicsInPrepare { app: None, log: log.clone() };

    let error = within("listen", listen_err(builder(Empty), vec![server(panicked())])).await;
    assert!(matches!(&error, StartupError::Configure(errors) if errors.len() == 1), "got: {error}");
    assert!(ulo::is_panic(&ulo::BoxError::from(error)), "the `StartupError` of a panicking `prepare`");

    let mixed = within(
        "listen",
        listen_err(builder(Empty), vec![server(TestServer::<TB>::new("b", &log).fail_prepare()), server(panicked())]),
    )
    .await;
    let StartupError::Configure(errors) = mixed else { panic!("expected configuration errors, got: {mixed}") };
    let transports: Vec<TypeName> = errors.iter().map(|entry| entry.transport).collect();
    assert_eq!(transports, [TypeName::of::<TB>(), TypeName::of::<TA>()], "the plain failure first, then the panic");
    let entries: Vec<bool> = errors
        .iter()
        .map(|entry| entry.source.downcast_ref::<PreparePanicked>().is_some())
        .collect();
    assert_eq!(entries, [false, true], "only the second entry carries the panic");
    assert!(ulo::is_panic(&ulo::BoxError::from(errors)), "the `ConfigureErrors` holding one panic among its entries");

    let plain = within(
        "listen",
        listen_err(
            builder(Empty),
            vec![
                server(TestServer::<TA>::new("a", &log).fail_prepare()),
                server(TestServer::<TB>::new("b", &log).fail_prepare()),
            ],
        ),
    )
    .await;
    let StartupError::Configure(errors) = &plain else { panic!("expected configuration errors, got: {plain}") };
    assert!(errors.iter().all(|entry| entry.source.downcast_ref::<Boom>().is_some()), "each entry is the server's own error");
    assert!(!ulo::is_panic(&ulo::BoxError::from(plain)), "a report of plain `prepare` failures is no panic");
}
