//! Readiness checks (section 9.3).
//!
//! Capabilities pinned: 42 (a check runs right after its binding and before anything that reads
//! it, with retries, an attempt bound, a whole-check bound and a backoff; a failed check fails
//! startup as `ConnectError::Readiness` with the attempts made and how the last one ended;
//! credentials are redacted from the error).
//!
//! Bounds run on `Frozen`, whose every sleep ends at once: a check that is still pending when
//! polled is abandoned in that same poll, so no test waits on a clock. The backoff test uses
//! `Recorded`, which also writes down each sleep it is asked for, and `Refusing`, which refuses
//! each one as a runtime that has shut down does.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::{App, ConnectError, Dep, FailureReason, Key, Limit, ModuleDef, Permanent, StartupError};

struct A;
struct S1;
struct S2;
struct S3;

fn readiness(error: StartupError) -> (Key, u32, FailureReason) {
    let StartupError::Connect(ConnectError::Readiness { key, attempts, reason }) = error else {
        panic!("expected a readiness failure, got: {error}");
    };
    (key.key(), attempts, reason)
}

/// The startup error of a builder whose graph wires and whose connect must fail.
async fn connect_failing(builder: ulo::AppBuilder) -> StartupError {
    watchdog();
    let app = builder.wire().expect("the graph did not wire");
    within("connect", app.connect()).await.err().expect("connect succeeded with a check that cannot pass")
}

#[tokio::test]
async fn a_check_runs_right_after_its_binding_and_before_what_reads_it_and_may_pass_on_a_later_attempt() {
    let log = Log::new();
    let tries = Count::new();
    let (l, t) = (log.clone(), tries.clone());
    let app = connects(Mod::<A>::new(move |m: &mut ModuleDef<'_>| {
        // Declared first so only the edge, not declaration order, puts the check before it.
        let built = l.clone();
        m.singleton(move |_s: Dep<S1>| {
            built.push("s2 built");
            async { S2 }
        });
        let built = l.clone();
        let checked = l.clone();
        let t = t.clone();
        m.singleton(move || {
            built.push("s1 built");
            async { S1 }
        })
        .ready(move |_s: Dep<S1>| {
            let n = t.bump();
            let log = checked.clone();
            async move {
                log.push(format!("s1 check {n}"));
                if n < 3 { Err(Boom::new("not yet")) } else { Ok(()) }
            }
        })
        .retries(5);
        let built = l.clone();
        m.singleton(move || {
            built.push("s3 built");
            async { S3 }
        });
    }))
    .await;
    assert_eq!(
        log.events(),
        ["s1 built", "s1 check 1", "s1 check 2", "s1 check 3", "s2 built", "s3 built"],
        "the check follows its binding, retries until it passes, and precedes the binding that reads it"
    );
    assert_eq!(tries.get(), 3, "a passing attempt ends the check");
    close(app).await;
}

#[tokio::test]
async fn a_check_that_never_passes_fails_connect_after_the_retries_plus_one_attempts() {
    for retries in [0u32, 1, 4] {
        let tries = Count::new();
        let t = tries.clone();
        watchdog();
        let error = connect_err(Mod::<A>::new(move |m| {
            let t = t.clone();
            m.singleton(|| async { S1 })
                .ready(move |_s: Dep<S1>| {
                    let n = t.bump();
                    async move { Err::<(), _>(Boom::new(format!("attempt {n}"))) }
                })
                .retries(retries);
        }))
        .await;

        let (key, attempts, reason) = readiness(error);
        assert_eq!(key, Key::of::<S1, ()>());
        assert_eq!(attempts, retries + 1, "retries count the attempts after the first");
        assert_eq!(tries.get() as u32, attempts, "the report counts the attempts the check made");
        let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
        assert_eq!(
            source.downcast_ref::<Boom>().map(|b| b.0.clone()),
            Some(format!("attempt {attempts}")),
            "the reason is how the last attempt ended"
        );
    }
}

#[tokio::test]
async fn an_attempt_that_outlives_its_bound_is_timed_out_and_retried() {
    let tries = Count::new();
    let t = tries.clone();
    let builder = App::builder(Mod::<A>::new(move |m| {
        let t = t.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                t.bump();
                async {
                    std::future::pending::<()>().await;
                    Ok::<(), Boom>(())
                }
            })
            .retries(2)
            .attempt_timeout(Duration::from_secs(5));
    }))
    .timer(Frozen::new());

    let (key, attempts, reason) = readiness(connect_failing(builder).await);
    assert_eq!((key, attempts), (Key::of::<S1, ()>(), 3), "a timed-out attempt is retried like an error");
    assert_eq!(tries.get(), 3);
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(5), Limit::Attempt));
}

#[tokio::test]
async fn an_attempt_with_no_bound_of_its_own_takes_the_construct_timeout() {
    let builder = App::builder(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 }).ready(|_s: Dep<S1>| async {
            std::future::pending::<()>().await;
            Ok::<(), Boom>(())
        });
    }))
    .timer(Frozen::new())
    .construct_timeout(Duration::from_secs(9));

    let (_, attempts, reason) = readiness(connect_failing(builder).await);
    assert_eq!(attempts, 1);
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(9), Limit::Default));
}

#[tokio::test]
async fn the_whole_check_bound_ends_the_check_whatever_the_attempt_was_doing() {
    let builder = App::builder(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 })
            .ready(|_s: Dep<S1>| async {
                std::future::pending::<()>().await;
                Ok::<(), Boom>(())
            })
            .retries(5)
            .timeout(Duration::from_secs(4));
    }))
    .timer(Frozen::new());

    let (_, attempts, reason) = readiness(connect_failing(builder).await);
    assert_eq!(attempts, 1, "the bound fired inside the first attempt, which writing `.timeout` alone left unbounded");
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(4), Limit::Item));
}

#[tokio::test]
async fn a_panicking_attempt_ends_the_check_at_once() {
    let tries = Count::new();
    let t = tries.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let t = t.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                t.bump();
                async {
                    if true {
                        panic!("the check panics");
                    }
                    Ok::<(), Boom>(())
                }
            })
            .retries(5);
    }))
    .await;

    let (_, attempts, reason) = readiness(error);
    assert_eq!((attempts, tries.get()), (1, 1), "a panic is not retried");
    assert!(matches!(reason, FailureReason::Panicked(_)), "got: {reason}");
}

#[tokio::test]
async fn a_permanent_error_ends_the_check_at_once_and_a_plain_one_does_not() {
    let tries = Count::new();
    let t = tries.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let t = t.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                t.bump();
                async { Err::<(), _>(Permanent::new(Boom::new("bad credentials"))) }
            })
            .retries(5);
    }))
    .await;
    assert!(ulo::is_permanent(&error), "a restart cannot mend a `Permanent` failure");
    let (_, attempts, reason) = readiness(error);
    assert_eq!((attempts, tries.get()), (1, 1), "no further attempt follows a permanent error");
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(source.downcast_ref::<Permanent>().is_some(), "the marker stays reachable");

    let plain = connect_err(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 })
            .ready(|_s: Dep<S1>| async { Err::<(), _>(Boom::new("connection refused")) })
            .retries(2);
    }))
    .await;
    assert!(!ulo::is_permanent(&plain), "an error that carries no `Permanent` may succeed on a restart");
    assert_eq!(readiness(plain).1, 3);
}

#[tokio::test]
async fn a_secret_in_a_check_error_is_redacted_from_the_report_and_the_original_stays_reachable() {
    const SECRET: &str = "hunter2-token";
    let secret = ulo::Secret::new(SECRET.to_owned());
    let error = connect_err(Mod::<A>::new(move |m| {
        m.secret(&secret);
        let leaked = secret.expose().clone();
        m.singleton(|| async { S1 }).ready(move |_s: Dep<S1>| {
            let leaked = leaked.clone();
            async move { Err::<(), _>(Boom::new(format!("login failed with {leaked}"))) }
        });
    }))
    .await;

    assert!(!error.to_string().contains(SECRET), "the startup report holds the secret");
    let (_, _, reason) = readiness(error);
    let FailureReason::Errored(source) = reason else { panic!("expected `Errored`, got: {reason}") };
    assert!(!source.to_string().contains(SECRET), "`Display` writes the secret");
    assert!(!format!("{source:?}").contains(SECRET), "`Debug` writes the secret");
    assert!(
        source.downcast_ref::<Boom>().is_some_and(|b| b.0.contains(SECRET)),
        "the original is handed over by type, secret and all"
    );
}

#[tokio::test]
async fn a_failed_check_stops_the_walk_so_nothing_after_it_is_built() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let built = l.clone();
        m.singleton(move || {
            built.push("s1 built");
            async { S1 }
        })
        .ready(|_s: Dep<S1>| async { Err::<(), _>(Boom::new("down")) });
        let built = l.clone();
        m.singleton(move |_s: Dep<S1>| {
            built.push("s2 built");
            async { S2 }
        });
        let built = l.clone();
        m.singleton(move || {
            built.push("s3 built");
            async { S3 }
        });
    }))
    .await;

    assert!(matches!(error, StartupError::Connect(ConnectError::Readiness { .. })), "got: {error}");
    assert_eq!(log.events(), ["s1 built"], "neither the reader of `S1` nor the unrelated binding after it was built");
}

#[tokio::test]
async fn a_backoff_waits_between_attempts_on_the_apps_timer_and_not_after_the_last() {
    let sleeps = SleepLog::new();
    let backoff = Duration::from_millis(7);
    let tries = Count::new();
    let t = tries.clone();
    let builder = App::builder(Mod::<A>::new(move |m| {
        let t = t.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                t.bump();
                async { Err::<(), _>(Boom::new("down")) }
            })
            .retries(3)
            .backoff(backoff);
    }))
    .timer(Recorded::new(&sleeps));
    let (_, attempts, _) = readiness(connect_failing(builder).await);
    assert_eq!(attempts, 4);
    assert_eq!(sleeps.count(backoff), 3, "one backoff between each pair of attempts and none after the last");

    let sleeps = SleepLog::new();
    let tries = Count::new();
    let t = tries.clone();
    let app = App::builder(Mod::<A>::new(move |m| {
        let t = t.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                let n = t.bump();
                async move { if n < 2 { Err(Boom::new("down")) } else { Ok(()) } }
            })
            .retries(3)
            .backoff(backoff);
    }))
    .timer(Recorded::new(&sleeps))
    .wire()
    .unwrap()
    .connect()
    .await
    .expect("the second attempt passes");
    assert_eq!(sleeps.count(backoff), 1, "a passing attempt ends the waiting");
    close(app).await;
}

/// N3: a `Timer` that refuses the backoff, its runtime shut down, ends the check after the one
/// refused wait, with the last attempt's reason, however many retries are left. Retried without
/// the wait instead, it would run all 1001 attempts.
#[tokio::test]
async fn a_backoff_the_timer_refuses_ends_the_check() {
    let sleeps = SleepLog::new();
    let backoff = Duration::from_millis(7);
    let tries = Count::new();
    let t = tries.clone();
    let builder = App::builder(Mod::<A>::new(move |m| {
        let t = t.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                t.bump();
                async { Err::<(), _>(Boom::new("down")) }
            })
            .retries(1000)
            .backoff(backoff);
    }))
    .timer(Refusing::new(&sleeps));
    let (_, attempts, reason) = readiness(connect_failing(builder).await);
    assert!(matches!(reason, FailureReason::Errored(_)), "the check did not end with its last attempt's error: {reason:?}");
    assert_eq!((attempts, sleeps.count(backoff)), (1, 1), "the attempts and backoffs on a `Timer` refusing the backoff");
}
