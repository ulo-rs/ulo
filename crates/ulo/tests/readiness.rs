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
//! each one as a runtime that has shut down does; the refused backoffs after a timed-out attempt and
//! after the whole-check bound passed run on a `ScriptedTimer` refusing the backoff alone.

#[macro_use]
mod common;

use std::error::Error;
use std::time::Duration;

use common::*;
use ulo_conformance_support::ScriptedTimer;
use ulo::{
    App, ConnectError, Dep, FailureReason, Key, Limit, ModuleDef, Permanent, RUNTIME_SHUT_DOWN, Redacted, Refused, RuntimeShutDown,
    StartupError, is_permanent,
};

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

/// The `RuntimeShutDown` a check ended with, found where the core stores it: inside the
/// `Permanent` its `Errored` reason holds.
fn shut_down(reason: &FailureReason) -> &RuntimeShutDown {
    let FailureReason::Errored(stored) = reason else {
        panic!("the check did not end as `Errored`: {reason:?}");
    };
    let permanent = stored.downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the check's error is not `Permanent`: {reason:?}"));
    permanent
        .source()
        .and_then(|inner| inner.downcast_ref::<RuntimeShutDown>())
        .unwrap_or_else(|| panic!("the `Permanent` does not hold a `RuntimeShutDown`: {reason:?}"))
}

/// N3: a `Timer` that refuses the backoff, its runtime shut down, ends the check after the one
/// refused wait, however many retries are left, and reports the refusal: `runtime_shut_down`,
/// `Permanent`, the last attempt's error as its source. Retried without the wait instead, it would
/// run all 1001 attempts; reported as the last attempt's error alone, it would read as a check out
/// of attempts and as retryable.
#[tokio::test]
async fn a_backoff_the_timer_refuses_ends_the_check_with_the_refusal() {
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
    let error = connect_failing(builder).await;
    assert!(is_permanent(&error), "a check ended on a refused backoff is retryable: {error}");
    let (_, attempts, reason) = readiness(error);
    assert_eq!((attempts, sleeps.count(backoff)), (1, 1), "the attempts and backoffs on a `Timer` refusing the backoff");
    let ended = shut_down(&reason);
    assert_eq!(ended.reason(), RUNTIME_SHUT_DOWN);
    let last = ended.source().and_then(|e| e.downcast_ref::<Redacted>()).and_then(Redacted::downcast_ref::<Boom>);
    assert!(last.is_some(), "the refusal's source is not the last attempt's error: {ended:?}");
}

/// An attempt that timed out before the refused backoff has no error to carry: the check reports
/// the refusal, with the runtime's `Refused` as its source, and not `TimedOut { limit: Attempt }`.
/// The scripted clock refuses the backoff alone, and the test moves it past the attempt bound.
#[tokio::test]
async fn a_refused_backoff_after_a_timed_out_attempt_reports_the_refusal() {
    let backoff = Duration::from_millis(7);
    let bound = Duration::from_secs(5);
    let clock = ScriptedTimer::new();
    clock.refuse(backoff);
    let builder = App::builder(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 })
            .ready(|_s: Dep<S1>| std::future::pending::<Result<(), Boom>>())
            .retries(1000)
            .backoff(backoff)
            .attempt_timeout(bound);
    }))
    .timer(clock.clone());
    watchdog();
    let app = builder.wire().expect("the graph did not wire");
    let error = within("connect", async {
        let mut connecting = std::pin::pin!(app.connect());
        loop {
            tokio::select! {
                ended = &mut connecting => break ended.err().expect("connect succeeded with a check that cannot pass"),
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    if clock.live(bound) > 0 {
                        clock.advance(bound);
                    }
                }
            }
        }
    })
    .await;
    assert!(is_permanent(&error), "a check ended on a refused backoff is retryable: {error}");
    let (_, attempts, reason) = readiness(error);
    assert_eq!((attempts, clock.refused(backoff)), (1, 1), "the attempts and backoffs on a `Timer` refusing the backoff");
    let ended = shut_down(&reason);
    assert_eq!(ended.reason(), RUNTIME_SHUT_DOWN);
    assert!(ended.source().is_some_and(|e| e.is::<Refused>()), "the refusal's source is not the runtime's `Refused`: {ended:?}");
}

/// A backoff refused once the whole-check bound has passed reports the bound's expiry, retryable,
/// and not the refusal: only a bound that did not pass is reported as refused. The attempt fails
/// in the poll that moves the clock past the bound, so its refused backoff and the bound's expiry
/// answer together.
#[tokio::test]
async fn a_backoff_refused_after_the_whole_check_bound_passed_reports_the_expiry() {
    let backoff = Duration::from_millis(7);
    let whole = Duration::from_secs(5);
    let late = whole + Duration::from_secs(1);
    let clock = ScriptedTimer::new();
    clock.refuse(backoff);
    let attempt_clock = clock.clone();
    let builder = App::builder(Mod::<A>::new(move |m| {
        let attempt_clock = attempt_clock.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                let slept = ulo::Timer::sleep(&attempt_clock, late);
                async move {
                    slept.await;
                    Err::<(), _>(Boom::new("down"))
                }
            })
            .retries(1000)
            .backoff(backoff)
            .timeout(whole);
    }))
    .timer(clock.clone());
    watchdog();
    let app = builder.wire().expect("the graph did not wire");
    let error = within("connect", async {
        let mut connecting = std::pin::pin!(app.connect());
        loop {
            tokio::select! {
                ended = &mut connecting => break ended.err().expect("connect succeeded with a check that cannot pass"),
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    if clock.live(whole) == 1 && clock.live(late) == 1 {
                        clock.advance(late);
                    }
                }
            }
        }
    })
    .await;
    assert!(!is_permanent(&error), "a check whose bound passed is permanent: {error}");
    let (_, attempts, reason) = readiness(error);
    assert_eq!(attempts, 1);
    assert!(
        matches!(reason, FailureReason::TimedOut { after, limit: Limit::Item } if after == whole),
        "a backoff refused after the whole-check bound passed reported {reason:?}"
    );
    assert_eq!(clock.refused(backoff), 1, "the backoffs asked of a `Timer` refusing them");
}

/// A check under a five-second whole-check bound whose one attempt fails once the scripted clock
/// reads `failed_at`, its backoff then refused: what `connect` fails with.
async fn backoff_refused_at(failed_at: Duration) -> StartupError {
    let backoff = Duration::from_millis(7);
    let whole = Duration::from_secs(5);
    let clock = ScriptedTimer::new();
    clock.refuse(backoff);
    let attempt_clock = clock.clone();
    let builder = App::builder(Mod::<A>::new(move |m| {
        let attempt_clock = attempt_clock.clone();
        m.singleton(|| async { S1 })
            .ready(move |_s: Dep<S1>| {
                let slept = ulo::Timer::sleep(&attempt_clock, failed_at);
                async move {
                    slept.await;
                    Err::<(), _>(Boom::new("down"))
                }
            })
            .retries(1000)
            .backoff(backoff)
            .timeout(whole);
    }))
    .timer(clock.clone());
    watchdog();
    let app = builder.wire().expect("the graph did not wire");
    within("connect", async {
        let mut connecting = std::pin::pin!(app.connect());
        loop {
            tokio::select! {
                ended = &mut connecting => break ended.err().expect("connect succeeded with a check that cannot pass"),
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    let armed = if failed_at == whole { clock.live(whole) == 2 } else { clock.live(whole) == 1 && clock.live(failed_at) == 1 };
                    if armed {
                        clock.advance(failed_at);
                    }
                }
            }
        }
    })
    .await
}

/// A backoff refused before the whole-check bound passes reports the refusal, `Permanent`, the
/// last attempt's error as its source: a bound that has not passed is not reported as passing.
#[tokio::test]
async fn a_backoff_refused_before_the_whole_check_bound_passed_reports_the_refusal() {
    let error = backoff_refused_at(Duration::from_secs(1)).await;
    assert!(is_permanent(&error), "a check ended on a refused backoff is retryable: {error}");
    let (_, attempts, reason) = readiness(error);
    assert_eq!(attempts, 1);
    let ended = shut_down(&reason);
    let last = ended.source().and_then(|e| e.downcast_ref::<Redacted>()).and_then(Redacted::downcast_ref::<Boom>);
    assert!(last.is_some(), "the refusal's source is not the last attempt's error: {ended:?}");
}

/// A backoff refused with the clock at the whole-check bound's end reports the expiry: a bound
/// has passed once the clock reaches it, as `Expiry` reads it.
#[tokio::test]
async fn a_backoff_refused_at_the_whole_check_bound_reports_the_expiry() {
    let error = backoff_refused_at(Duration::from_secs(5)).await;
    assert!(!is_permanent(&error), "a check whose bound passed is permanent: {error}");
    let (_, _, reason) = readiness(error);
    assert!(
        matches!(reason, FailureReason::TimedOut { after, limit: Limit::Item } if after == Duration::from_secs(5)),
        "a backoff refused at the whole-check bound reported {reason:?}"
    );
}

/// The control: on a clock that keeps time, a check whose retries run out on errors ends with the
/// last attempt's error, retryable, and no `RuntimeShutDown`.
#[tokio::test]
async fn retries_that_run_out_on_a_running_clock_end_with_the_last_error() {
    let sleeps = SleepLog::new();
    let backoff = Duration::from_millis(7);
    let builder = App::builder(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 })
            .ready(|_s: Dep<S1>| async { Err::<(), _>(Boom::new("down")) })
            .retries(1)
            .backoff(backoff);
    }))
    .timer(Recorded::new(&sleeps));
    let error = connect_failing(builder).await;
    assert!(!is_permanent(&error), "a check out of retries is permanent: {error}");
    let (_, attempts, reason) = readiness(error);
    assert_eq!(attempts, 2);
    let FailureReason::Errored(last) = &reason else {
        panic!("the check did not end with its last attempt's error: {reason:?}");
    };
    assert!(last.downcast_ref::<Boom>().is_some(), "the check's error is not the last attempt's: {reason:?}");
}
