//! A transport's `serve` that fails once another trigger has arrived (section 9.5): the outcome
//! reports the trigger alone, as ruled, and the failure is written to the log as a `warn` naming
//! the transport and its error.
//!
//! Capability pinned: 45 (no failure is lost without a trace). The outcome side of these races is
//! `ulo-runtime-tokio/tests/app.rs`'s `two_serves_ending_on_one_poll_record_the_first_bound_once`
//! and `a_serve_failing_after_a_close_is_no_failure`.
//!
//! The file is its own test binary because it installs the process's global `tracing` subscriber:
//! a runtime polls `serve` on whatever thread it likes, which a thread-local one would miss.

#[macro_use]
mod common;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::task::Poll;

use common::*;
use ulo::{AppHandle, BoxError, ServeEnd, ShutdownFailure, Signal, TypeName};
use ulo_runtime_tokio::Tokio;

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

    /// The warnings whose `error` field is `error`.
    fn about(&self, error: &str) -> Vec<HashMap<String, String>> {
        let events = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        events.iter().filter(|fields| fields.get("error").map(String::as_str) == Some(error)).cloned().collect()
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

/// When a [`Failing`] server's `serve` fails.
#[derive(Clone)]
enum When {
    /// At once.
    AtOnce,
    /// Once its `drain` has run: after the shutdown began. `serving` opens as `serve` starts.
    AfterDrain { drained: Arc<Gate>, serving: Arc<Gate> },
    /// Right after it triggered a `close` through the app's handle, so the `close` has won by the
    /// time the failure reaches the trigger.
    AfterClose,
}

/// A server for `T` whose `serve` fails with `error`.
struct Failing<T> {
    error: &'static str,
    when: When,
    app: Option<AppHandle>,
    _t: std::marker::PhantomData<fn() -> T>,
}

fn failing<T>(error: &'static str, when: When) -> Failing<T> {
    Failing { error, when, app: None, _t: std::marker::PhantomData }
}

impl<T: ulo::Transport> ulo::Server for Failing<T> {
    type Transport = T;

    async fn prepare(&mut self, mounted: ulo::Mounted<'_, T>) -> Result<(), BoxError> {
        self.app = Some(mounted.app().clone());
        Ok(())
    }

    async fn bind(&mut self, _mounted: ulo::Mounted<'_, T>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        match &self.when {
            When::AtOnce => {}
            When::AfterDrain { drained, serving } => {
                serving.open();
                drained.pass().await;
            }
            When::AfterClose => {
                let app = self.app.clone().expect("prepared");
                // One poll triggers the shutdown; the close runs on from `serve`'s own `close`.
                let mut close = Box::pin(app.close(Signal::new("closed from inside")));
                std::future::poll_fn(|cx| {
                    let _ = close.as_mut().poll(cx);
                    Poll::Ready(())
                })
                .await;
            }
        }
        Err(Box::new(Boom::new(self.error)))
    }

    async fn drain(&self, _token: ulo::DrainToken) {
        if let When::AfterDrain { drained, .. } = &self.when {
            drained.open();
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

fn the_one(warnings: Vec<HashMap<String, String>>, error: &str) -> HashMap<String, String> {
    assert_eq!(warnings.len(), 1, "one `warn` for the discarded `{error}`: {warnings:?}");
    warnings.into_iter().next().expect("one")
}

#[tokio::test]
async fn a_serve_failing_after_a_handle_closed_the_app_is_warned_about_and_the_outcome_is_the_close() {
    let warnings = Warnings::installed();
    let serving = Gate::new(false);
    let when = When::AfterDrain { drained: Gate::new(false), serving: Arc::clone(&serving) };
    let app = listened(
        ulo::App::builder(Empty).runtime(Tokio::current()).timer(Frozen::new()),
        vec![server(failing::<TB>("lost the listener at the drain", when))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let handle = app.handle();
    let served = tokio::spawn(app.serve(std::future::pending()));
    // `serve` takes the servers when it starts; one started after the close step has none.
    within("serve to start", serving.pass()).await;
    let closed = within("close", handle.close(Signal::new("operator"))).await;
    let served = within("serve", served).await.expect("serve did not panic");

    let shutdown = served.unwrap_or_else(|error| panic!("a serve failing after the close is no failure: {error}"));
    assert_eq!(shutdown.signal.to_string(), "operator");
    assert!(closed.is_ok());
    let warning = the_one(warnings.about("lost the listener at the drain"), "lost the listener at the drain");
    assert_eq!(warning.get("transport"), Some(&TypeName::of::<TB>().to_string()), "{warning:?}");
}

#[tokio::test]
async fn the_second_of_two_serves_failing_on_one_poll_is_warned_about_and_the_first_is_the_outcome() {
    let warnings = Warnings::installed();
    let app = listened(
        ulo::App::builder(Empty).runtime(Tokio::current()).timer(Frozen::new()),
        vec![
            server(failing::<TA>("the first accept loop died", When::AtOnce)),
            server(failing::<TB>("the second accept loop died", When::AtOnce)),
        ],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let error = within("serve", app.serve(std::future::pending())).await.expect_err("a serve that died is a failure");

    let [ShutdownFailure::Serve { transport, end: ServeEnd::Errored(_) }] = &*error.failures else {
        panic!("one `Serve` failure for the first server: {error}");
    };
    assert_eq!(*transport, TypeName::of::<TA>(), "the first bound is the trigger");
    assert!(warnings.about("the first accept loop died").is_empty(), "the trigger is reported, not warned about");
    let warning = the_one(warnings.about("the second accept loop died"), "the second accept loop died");
    assert_eq!(warning.get("transport"), Some(&TypeName::of::<TB>().to_string()), "{warning:?}");
}

#[tokio::test]
async fn a_serve_failing_after_a_close_won_the_trigger_in_the_same_poll_is_warned_about() {
    let warnings = Warnings::installed();
    let app = listened(
        ulo::App::builder(Empty).runtime(Tokio::current()).timer(Frozen::new()),
        vec![server(failing::<TA>("died as the close arrived", When::AfterClose))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let shutdown = within("serve", app.serve(std::future::pending()))
        .await
        .unwrap_or_else(|error| panic!("the close won, so the serve's end is no failure: {error}"));

    assert_eq!(shutdown.signal.to_string(), "closed from inside");
    let warning = the_one(warnings.about("died as the close arrived"), "died as the close arrived");
    assert_eq!(warning.get("transport"), Some(&TypeName::of::<TA>().to_string()), "{warning:?}");
}

#[tokio::test]
async fn a_serve_that_starts_the_shutdown_is_its_outcome_and_no_warning() {
    let warnings = Warnings::installed();
    let app = listened(
        ulo::App::builder(Empty).runtime(Tokio::current()).timer(Frozen::new()),
        vec![server(failing::<TA>("the only accept loop died", When::AtOnce))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let error = within("serve", app.serve(std::future::pending())).await.expect_err("a serve that died is a failure");
    assert!(matches!(&*error.failures, [ShutdownFailure::Serve { .. }]), "{error}");
    assert!(warnings.about("the only accept loop died").is_empty(), "a trigger is never also a warning");
}
