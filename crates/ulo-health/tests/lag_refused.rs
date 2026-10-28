//! The before-shutdown lag holds the shutdown open on the app's `Timer`. A `Timer` whose runtime
//! has shut down refuses it, and the hook ends at once, logged at `debug`: on `sleep`, which ends
//! at once there too, the shutdown would go on with nothing saying the lag was skipped. The app
//! binds no transport and runs on its clock alone, driven from a plain thread.

use std::fmt;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_conformance_support::ScriptedTimer;
use ulo_health::HealthModule;

/// The lag, the one length the clock refuses.
const LAG: Duration = Duration::from_secs(7);

/// How long the test waits for the app to connect and close before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the hook logs when the clock refuses the lag.
const REFUSED: &str = "refused the health module's lag";

/// The test binary's global `tracing` subscriber, keeping each event's fields as text.
struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    fn installed() -> &'static Recorder {
        static RECORDER: OnceLock<&'static Recorder> = OnceLock::new();
        RECORDER.get_or_init(|| {
            let recorder: &'static Recorder = Box::leak(Box::new(Recorder(Mutex::new(Vec::new()))));
            tracing::subscriber::set_global_default(Forward(recorder)).expect("the test binary installs no other subscriber");
            recorder
        })
    }

    fn containing(&self, needle: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|text| text.contains(needle)).count()
    }
}

struct Forward(&'static Recorder);

impl Subscriber for Forward {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut text = Fields(format!("{} ", event.metadata().level()));
        event.record(&mut text);
        self.0.0.lock().unwrap().push(text.0);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(HealthModule::new().lag(LAG));
    }
}

#[test]
fn a_refused_lag_ends_the_hook_at_once_and_is_logged() {
    let recorder = Recorder::installed();
    let clock = ScriptedTimer::new();
    clock.refuse(LAG);
    let timer = clock.clone();
    let (closed, close) = mpsc::channel();
    thread::spawn(move || {
        let outcome = futures_executor::block_on(async move {
            let app = App::builder(Root).timer(timer).wire().unwrap_or_else(|error| panic!("the app wires: {error}"));
            let app = app.connect().await.unwrap_or_else(|error| panic!("the app connects: {error}"));
            app.close(Signal::new("test")).await
        });
        let _ = closed.send(outcome.map(|shutdown| format!("{shutdown:?}")).map_err(|error| error.to_string()));
    });
    let outcome = close.recv_timeout(PATIENCE).expect("the app neither connected nor closed on a clock refusing the lag");
    let outcome = outcome.unwrap_or_else(|error| panic!("the app's close failed on a clock refusing the lag: {error}"));
    assert_eq!(clock.refused(LAG), 1, "the lags the hook asked of a clock refusing them ({outcome})");
    assert_eq!(recorder.containing(REFUSED), 1, "the hook did not log the refused lag at `debug` ({outcome})");
}
