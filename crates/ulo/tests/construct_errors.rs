//! How a construction fails during `connect` (section 3.4, 3.9, 10.2).
//!
//! Capabilities pinned: 15 and 45 for the shapes `injection.rs` and `bindings.rs` leave to this
//! file (`Panicked`, `TimedOut`, the redaction of a constructor's error), and the rule of section
//! 9.2 that a failed `connect` runs no hook of what the walk had built. A constructor that returns
//! `Err` is `Errored` and is in `injection.rs` and `bindings.rs`.
//!
//! Timeouts run on `Frozen`, whose every sleep ends at once, so a pending constructor is
//! abandoned in the poll that starts it.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::scope::Singleton;
use ulo::{
    App, BoxError, Bound, ConnectError, Construct, ConstructError, Dependencies, FailureReason, Key, Limit, Resolver, Secret, Signal,
    StartupError,
};

struct A;
struct S1;
struct S2;
struct S3;

fn construction(error: StartupError) -> (Key, String, FailureReason) {
    let StartupError::Connect(ConnectError::Construct { key, module, reason }) = error else {
        panic!("expected a construction failure, got: {error}");
    };
    (key.key(), module.to_string(), reason)
}

async fn failing(builder: ulo::AppBuilder) -> StartupError {
    watchdog();
    within("connect", builder.wire().expect("the graph did not wire").connect()).await.err().expect("connect succeeded")
}

#[tokio::test]
async fn a_panicking_constructor_is_a_panicked_failure_the_core_recognises() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.singleton(|| async {
            if true {
                panic!("the factory panics");
            }
            S1
        });
    }))
    .await;

    let boxed: BoxError = Box::new(error);
    assert!(ulo::is_panic(&boxed), "`is_panic` finds the panic under the startup error");
    let (key, module, reason) = construction(*boxed.downcast::<StartupError>().unwrap());
    assert_eq!((key, module), (Key::of::<S1, ()>(), mod_name::<A>()));
    assert!(matches!(reason, FailureReason::Panicked(_)), "got: {reason}");
}

#[tokio::test]
async fn a_constructor_that_outlives_its_own_bound_is_timed_out_with_the_item_limit() {
    let builder = App::builder(Mod::<A>::new(|m| {
        m.singleton(|| async {
            std::future::pending::<()>().await;
            S1
        })
        .timeout(Duration::from_secs(2));
    }))
    .timer(Frozen::new());

    let (key, _, reason) = construction(failing(builder).await);
    assert_eq!(key, Key::of::<S1, ()>());
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(2), Limit::Item));
}

#[tokio::test]
async fn a_constructor_with_no_bound_of_its_own_takes_the_apps_construct_timeout() {
    let builder = App::builder(Mod::<A>::new(|m| {
        m.singleton(|| async {
            std::future::pending::<()>().await;
            S1
        });
    }))
    .timer(Frozen::new())
    .construct_timeout(Duration::from_secs(8));

    let (_, _, reason) = construction(failing(builder).await);
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(8), Limit::Default));
}

struct Slow;

impl Construct for Slow {
    type Scope = Singleton;
    const CONSTRUCT_TIMEOUT: Bound = Bound::After(Duration::from_secs(6));

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(_r: &Resolver<'_>) -> Result<Self, ConstructError> {
        std::future::pending::<()>().await;
        Ok(Slow)
    }
}

#[tokio::test]
async fn a_construct_types_own_timeout_is_its_bound() {
    let builder = App::builder(Mod::<A>::new(|m| {
        m.provide::<Slow>();
    }))
    .timer(Frozen::new());

    let (key, _, reason) = construction(failing(builder).await);
    assert_eq!(key, Key::of::<Slow, ()>());
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(6), Limit::Item));
}

#[tokio::test]
async fn a_secret_in_a_constructors_error_is_redacted_and_the_original_stays_reachable() {
    const SECRET: &str = "s3cr3t-password";
    let secret = Secret::new(SECRET.to_owned());
    let error = connect_err(Mod::<A>::new(move |m| {
        m.secret(&secret);
        let leaked = secret.expose().clone();
        m.try_singleton(move || {
            let leaked = leaked.clone();
            async move { Err::<S1, _>(Boom::new(format!("connect failed with {leaked}"))) }
        });
    }))
    .await;

    assert!(!error.to_string().contains(SECRET), "the startup report holds the secret");
    let (_, _, reason) = construction(error);
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(!source.to_string().contains(SECRET) && !format!("{source:?}").contains(SECRET));
    assert!(source.downcast_ref::<Boom>().is_some_and(|b| b.0.contains(SECRET)), "the original is handed over by type");
}

/// `m.value(Secret<String>)` registers the text it binds (§9.3), with no `m.secret(..)` beside it.
#[tokio::test]
async fn a_secret_bound_by_value_is_redacted_in_a_constructors_error() {
    const SECRET: &str = "v4lue-bound-secret";
    let error = connect_err(Mod::<A>::new(|m| {
        m.value(Secret::new(SECRET.to_owned()));
        m.try_singleton(|| async { Err::<S1, _>(Boom::new(format!("connect failed with {SECRET}"))) });
    }))
    .await;

    assert!(!error.to_string().contains(SECRET), "the startup report holds the secret: {error}");
    let (key, _, reason) = construction(error);
    assert_eq!(key, Key::of::<S1, ()>());
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(!source.to_string().contains(SECRET) && !format!("{source:?}").contains(SECRET));
    assert!(source.downcast_ref::<Boom>().is_some_and(|b| b.0.contains(SECRET)), "the original is handed over by type");
}

/// A `Secret<&'static str>` bound by value registers its text as a `Secret<String>` does.
#[tokio::test]
async fn a_str_secret_bound_by_value_is_redacted_in_a_constructors_error() {
    const SECRET: &str = "str-bound-secret";
    let error = connect_err(Mod::<A>::new(|m| {
        m.value(Secret::new(SECRET));
        m.try_singleton(|| async { Err::<S1, _>(Boom::new(format!("connect failed with {SECRET}"))) });
    }))
    .await;

    assert!(!error.to_string().contains(SECRET), "the startup report holds the secret: {error}");
    let (key, _, reason) = construction(error);
    assert_eq!(key, Key::of::<S1, ()>());
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(!source.to_string().contains(SECRET) && !format!("{source:?}").contains(SECRET));
}

/// A `Secret` of a type `value` cannot recognize by downcast.
#[derive(Clone, PartialEq, Eq)]
struct Token(&'static str);

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// `m.value` registers `Secret<String>` and `Secret<&'static str>` alone; any other `Secret<T>`
/// bound by value stays visible in a report until `m.secret(&s)` registers it (section 9.3).
#[tokio::test]
async fn a_secret_of_another_type_is_visible_in_a_report_without_m_secret() {
    const SECRET: &str = "custom-display-secret";
    let error = connect_err(Mod::<A>::new(|m| {
        m.value(Secret::new(Token(SECRET)));
        m.try_singleton(|| async { Err::<S1, _>(Boom::new(format!("connect failed with {SECRET}"))) });
    }))
    .await;

    assert!(error.to_string().contains(SECRET), "the documented gap closed, update section 9.3: {error}");
    let (key, _, reason) = construction(error);
    assert_eq!(key, Key::of::<S1, ()>());
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(source.to_string().contains(SECRET));
}

#[tokio::test]
async fn a_secret_of_another_type_is_redacted_once_m_secret_registers_it() {
    const SECRET: &str = "custom-display-secret";
    let error = connect_err(Mod::<A>::new(|m| {
        let secret = Secret::new(Token(SECRET));
        m.secret(&secret);
        m.value(secret);
        m.try_singleton(|| async { Err::<S1, _>(Boom::new(format!("connect failed with {SECRET}"))) });
    }))
    .await;

    assert!(!error.to_string().contains(SECRET), "the startup report holds the secret: {error}");
    let (key, _, reason) = construction(error);
    assert_eq!(key, Key::of::<S1, ()>());
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(!source.to_string().contains(SECRET) && !format!("{source:?}").contains(SECRET));
    assert!(source.downcast_ref::<Boom>().is_some_and(|b| b.0.contains(SECRET)));
}

#[tokio::test]
async fn a_failed_connect_stops_the_walk_and_runs_no_hook_of_what_it_had_built() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let (built, init, destroy, before, after) = (l.clone(), l.clone(), l.clone(), l.clone(), l.clone());
        m.singleton(move || {
            built.push("s1 built");
            async { S1 }
        })
        .on_init(log_hook(init, "s1 init"))
        .on_destroy(move || {
            let log = destroy.clone();
            async move { log.push("s1 destroy") }
        })
        .before_shutdown(move |_s: Signal| {
            let log = before.clone();
            async move { log.push("s1 before-shutdown") }
        })
        .on_shutdown(move |_s: Signal| {
            let log = after.clone();
            async move { log.push("s1 on-shutdown") }
        });
        m.try_singleton(|| async { Err::<S2, _>(Boom::new("cannot build")) });
        let later = l.clone();
        m.singleton(move || {
            later.push("s3 built");
            async { S3 }
        });
    }))
    .await;

    let (key, _, reason) = construction(error);
    assert_eq!(key, Key::of::<S2, ()>());
    assert!(matches!(reason, FailureReason::Errored(_)));
    let events = log.events();
    assert_eq!(events.first().map(String::as_str), Some("s1 built"), "{events:?}");
    assert!(!events.iter().any(|e| e.contains("init") || e.contains("destroy") || e.contains("shutdown")), "a hook ran: {events:?}");
    assert!(!events.contains(&"s3 built".to_owned()), "the walk went on past the failure: {events:?}");
}

fn log_hook(log: Log, text: &'static str) -> impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Boom>> + Send>> + Send + Sync + 'static {
    move || {
        let log = log.clone();
        Box::pin(async move {
            log.push(text);
            Ok(())
        })
    }
}
