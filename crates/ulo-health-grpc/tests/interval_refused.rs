//! The bridge's re-evaluation is a ticker on the app's runtime. A runtime that has shut down
//! refuses the next tick, and the task ends there: on `sleep`, which ends at once on such a
//! runtime, it would evaluate readiness without pause.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ulo::{App, Bound, BoxFuture, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_conformance_support::ScriptedTimer;
use ulo_health::{Check, Health, HealthModule};
use ulo_health_grpc::HealthGrpc;

/// The bridge's interval, the one length the clock refuses.
const INTERVAL: Duration = Duration::from_secs(7);

/// The ticks after which a ticker still asking is taken to spin.
const TURNS: usize = 3;

const PATIENCE: Duration = Duration::from_secs(10);

/// A readiness check counting its evaluations.
struct Counted(Arc<AtomicUsize>);

impl Check for Counted {
    fn name(&self) -> &str {
        "counted"
    }

    fn check(&self) -> BoxFuture<'_, Health> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Health::Up })
    }
}

struct Root(Arc<AtomicUsize>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(HealthModule::new());
        m.import(HealthGrpc::new().interval(Bound::After(INTERVAL)));
        let check: Arc<dyn Check> = Arc::new(Counted(Arc::clone(&self.0)));
        m.contribute::<dyn Check>().value(check);
    }
}

#[tokio::test]
async fn a_refused_interval_ends_the_re_evaluation() {
    let clock = ScriptedTimer::new();
    clock.refuse(INTERVAL);
    let runs = Arc::new(AtomicUsize::new(0));
    let app = App::builder(Root(Arc::clone(&runs)))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .wire()
        .unwrap_or_else(|error| panic!("the app wires: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app connects: {error}"));
    let began = Instant::now();
    while clock.refused(INTERVAL) == 0 {
        assert!(began.elapsed() < PATIENCE, "the bridge asked for no interval within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let began = Instant::now();
    while clock.refused(INTERVAL) <= TURNS && began.elapsed() < Duration::from_millis(200) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(clock.refused(INTERVAL), 1, "the bridge's ticks on a runtime refusing them: it re-evaluated on rather than ending");
    assert!(runs.load(Ordering::SeqCst) <= TURNS, "readiness was evaluated {} times on a refused interval", runs.load(Ordering::SeqCst));
    let _ = tokio::time::timeout(PATIENCE, app.close(Signal::new("test"))).await.expect("the app closes");
}
