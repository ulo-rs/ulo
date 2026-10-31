//! An execution-scoped instance whose `Drop` panics (section 3.8): the execution still ends, every
//! other instance still drops, and the first panic is logged once at `warn`, redacted.
//!
//! The file is its own test binary because it installs the process's global `tracing` subscriber.

#[macro_use]
mod common;

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use common::*;
use ulo::{ExecOptions, Secret, Signal};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Every `warn` event's fields, by name, as the process's global subscriber received them.
struct Warnings(Mutex<Vec<HashMap<String, String>>>);

impl Warnings {
    fn installed() -> &'static Warnings {
        static WARNINGS: OnceLock<&'static Warnings> = OnceLock::new();
        WARNINGS.get_or_init(|| {
            let warnings: &'static Warnings = Box::leak(Box::new(Warnings(Mutex::new(Vec::new()))));
            tracing::subscriber::set_global_default(Forward(warnings)).expect("this binary installs the only subscriber");
            warnings
        })
    }

    /// The `message` of every warning whose message starts with `prefix`: the tests in this
    /// binary share the subscriber and run in parallel, so each reads its own warnings.
    fn from(&self, prefix: &str) -> Vec<String> {
        let all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.iter().filter_map(|fields| fields.get("message")).filter(|m| m.starts_with(prefix)).cloned().collect()
    }
}

struct Forward(&'static Warnings);

impl Subscriber for Forward {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() == Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields(HashMap::new());
        event.record(&mut fields);
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner).push(fields.0);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

struct Fields(HashMap<String, String>);

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}


const SECRET: &str = "drop-token-5521";

static DROPPED: AtomicUsize = AtomicUsize::new(0);

/// Execution-scoped; the second one's `Drop` panics, carrying the secret.
struct Inst<const N: usize>;

impl<const N: usize> Drop for Inst<N> {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
        if N == 2 {
            panic!("instance 2 failed to drop holding {SECRET}");
        }
    }
}

struct Root;

#[tokio::test]
async fn a_panicking_drop_does_not_stop_the_other_instances_or_the_end_of_the_execution() {
    let warnings = Warnings::installed();
    let root = Mod::<Root>::new(|m| {
        m.secret(&Secret::new(SECRET.to_owned()));
        m.execution(|| async { Inst::<1> });
        m.execution(|| async { Inst::<2> });
        m.execution(|| async { Inst::<3> });
        m.execution(|| async { Inst::<4> });
    });
    let app = connected(root).await;

    // Map order is unspecified, so the panicking instance lands at every position over the runs.
    const RUNS: usize = 12;
    for run in 1..=RUNS {
        app.execute(ExecOptions::new(), async |exec| {
            exec.get::<Inst<1>>().await.unwrap();
            exec.get::<Inst<2>>().await.unwrap();
            exec.get::<Inst<3>>().await.unwrap();
            exec.get::<Inst<4>>().await.unwrap();
        })
        .await
        .expect("an instance's panicking drop must not fail the job");
        assert_eq!(DROPPED.load(Ordering::SeqCst), 4 * run, "run {run}: every instance dropped");
    }

    let logged = warnings.from("instance ");
    assert_eq!(logged.len(), RUNS, "one warning per execution, for its first panic: {logged:?}");
    for message in &logged {
        assert_eq!(message, "instance 2 failed to drop holding [redacted]");
    }

    let shutdown = app.close(Signal::new("done")).await.unwrap();
    assert_eq!(shutdown.abandoned, 0, "an execution whose instance panicked in its drop has ended");
}

static TWINS_DROPPED: AtomicUsize = AtomicUsize::new(0);

/// Execution-scoped; the first and the third one's `Drop` panic.
struct Twin<const N: usize>;

impl<const N: usize> Drop for Twin<N> {
    fn drop(&mut self) {
        TWINS_DROPPED.fetch_add(1, Ordering::SeqCst);
        if N == 1 || N == 3 {
            panic!("twin {N} failed to drop");
        }
    }
}

struct TwinRoot;

#[tokio::test]
async fn two_panicking_drops_in_one_execution_log_one_warning() {
    let warnings = Warnings::installed();
    let root = Mod::<TwinRoot>::new(|m| {
        m.execution(|| async { Twin::<1> });
        m.execution(|| async { Twin::<2> });
        m.execution(|| async { Twin::<3> });
        m.execution(|| async { Twin::<4> });
    });
    let app = connected(root).await;

    const RUNS: usize = 6;
    for run in 1..=RUNS {
        app.execute(ExecOptions::new(), async |exec| {
            exec.get::<Twin<1>>().await.unwrap();
            exec.get::<Twin<2>>().await.unwrap();
            exec.get::<Twin<3>>().await.unwrap();
            exec.get::<Twin<4>>().await.unwrap();
        })
        .await
        .expect("panicking drops must not fail the job");
        assert_eq!(TWINS_DROPPED.load(Ordering::SeqCst), 4 * run, "run {run}: every instance dropped");
    }

    let logged = warnings.from("twin ");
    assert_eq!(logged.len(), RUNS, "one warning per execution, not one per panic: {logged:?}");
    assert!(logged.iter().all(|m| m == "twin 1 failed to drop" || m == "twin 3 failed to drop"), "{logged:?}");
    app.close(Signal::new("done")).await.unwrap();
}
