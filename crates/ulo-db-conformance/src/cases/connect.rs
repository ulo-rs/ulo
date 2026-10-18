use std::time::{Duration, Instant};

use futures_util::future::join;
use ulo::{ConnectError, FailureReason, StartupError, is_permanent};
use ulo_db::Store;

use crate::cases::{PATIENT, Root, bounded, builder, closed, connect_failure, connecting, refused, sleep};
use crate::{Host, report};

/// The connect bound the unreachable scenario waits out.
const WITHIN: Duration = Duration::from_secs(3);

/// What a timed-out attempt's future and the app's refusal may add past the bound they waited out.
const OVERRUN: Duration = Duration::from_secs(2);

/// How long the create-loop scenario keeps the store out before restoring it.
const OUTAGE: Duration = Duration::from_millis(1_500);

/// The scheduling slack allowed below a bound a scenario expects to be waited out.
const EARLY: Duration = Duration::from_millis(100);

/// An unreachable database fails `connect` at its bound, not `Permanent`. Planted break:
/// `Unreachable` treated as permanent.
///
/// The module retries without counting out (a thousand retries a tenth of a second apart) under a
/// three-second `connect_within`, so only the bound ends the loop. The refusal is a connect failure
/// whose reason is `TimedOut`, `is_permanent` reads it as retryable, and it arrives no earlier than
/// the bound. A store whose `create` connects nothing fails as `ConnectError::Readiness`, the
/// `.ready(ping)` check timing out as a whole. A store whose `create` connects fails as
/// `ConnectError::Construct`, its loop around `create` reaching the construction bound; or, where
/// `create` succeeds without reaching the server (MongoDB on a plain `mongodb://` URL), as
/// `ConnectError::Readiness` from the one confirming ping, under the attempt bound. The app sets
/// that attempt bound, `construct_timeout`, to the connect bound, so either path waits at least the
/// bound and at most the bound plus one attempt.
pub async fn unreachable_fails_at_the_bound<H: Host>() {
    let host = H::start().await;
    let module = bounded::<H>(host.unreachable_options(), WITHIN);
    let (error, took) = refused(
        builder::<H>(Root::new().import(module)).construct_timeout(WITHIN),
        "an app over unreachable options",
        "nothing answers at the unreachable options",
    )
    .await;

    let Some((connect, reason)) = connect_failure(&error) else {
        panic!("an unreachable database failed outside `connect`: {}", report(&error));
    };
    assert!(!is_permanent(&error), "an unreachable database is the one retryable start failure, and `is_permanent` read it as permanent after {took:?}: {}", report(&error));
    assert!(
        matches!(reason, FailureReason::TimedOut { .. }),
        "an unreachable database ended its connect with `{reason}` after {took:?}, not at the bound"
    );
    let construct = matches!(connect, ConnectError::Construct { .. });
    let readiness = matches!(connect, ConnectError::Readiness { .. });
    if <H::Store as Store>::CREATE_CONNECTS {
        assert!(construct || readiness, "a store whose `create` connects failed its connect outside the loop and the confirming ping: {connect}");
    } else {
        assert!(readiness, "a store whose `create` connects nothing failed its connect outside the readiness check: {connect}");
    }
    assert!(took + EARLY >= WITHIN, "an unreachable database was refused after {took:?}, before the {WITHIN:?} bound: {connect}");
    let most = WITHIN + WITHIN + OVERRUN;
    assert!(took <= most, "an unreachable database was refused after {took:?}, past the bound and one attempt ({most:?}): {connect}");
}

/// Bad credentials fail `connect` at once, `Permanent`. Planted break: a permanent error retried to
/// the bound.
///
/// The module retries without counting out under a twenty-second bound. The refusal is permanent,
/// its reason the store's own error (`Errored`), it arrives in under half the bound, and it does not
/// print [`PASSWORD`](crate::PASSWORD), the password the host's refused credentials carry. A store
/// whose `create` connects nothing fails as `ConnectError::Readiness` after exactly one attempt; one
/// whose `create` connects fails in its loop as `ConnectError::Construct`, or in the confirming
/// ping as a one-attempt `ConnectError::Readiness`.
pub async fn bad_credentials_are_permanent<H: Host>() {
    let host = H::start().await;
    let module = bounded::<H>(host.bad_credentials(), PATIENT);
    let (error, took) =
        refused(builder::<H>(Root::new().import(module)), "an app with bad credentials", "the server accepted the bad credentials").await;
    assert_permanent_at_once::<H>(&error, took, "bad credentials");
}

/// A store whose `create` connects retries `create` under the connect bound and stops at once on
/// `Permanent`; a store whose `create` connects nothing declares it not applicable. Planted break:
/// a `create` run once outside the loop.
///
/// With the host's options cut, the app connects while the outage lasts and the store is restored
/// a second and a half in: the connect succeeds, no earlier than the restore, the loop having
/// carried `create` across the outage. Then bad credentials end the same loop at once, as
/// [`bad_credentials_are_permanent`] asserts. A `create` run once fails at the first refused
/// connection, with the store still out.
pub async fn create_loop<H: Host>() {
    assert!(
        <H::Store as Store>::CREATE_CONNECTS,
        "`{}`'s `create` connects nothing, so its one retry loop is `.ready`'s: declare this scenario not applicable",
        <H::Store as Store>::NAME
    );
    let host = H::start().await;
    let budget = host.budget();
    let within = OUTAGE + budget.recovery;

    host.cut().await;
    let module = bounded::<H>(host.options(), within);
    let (((outcome, took), connected_at), restoring) = join(
        async move {
            let connected = connecting(builder::<H>(Root::new().import(module)), "an app across an outage").await;
            (connected, Instant::now())
        },
        async {
            sleep::<H>(OUTAGE).await;
            let restoring = Instant::now();
            host.restore().await;
            restoring
        },
    )
    .await;
    match outcome {
        Ok(app) => {
            assert!(
                connected_at + EARLY >= restoring,
                "the app connected after {took:?}, before the store was restored {OUTAGE:?} into the outage: the host's `cut` let a connection through"
            );
            closed(app, "the app connected across the outage").await;
        }
        Err(error) => panic!(
            "a store whose `create` connects did not carry `create` across a {OUTAGE:?} outage within its {within:?} bound; it was refused after {took:?}: {}",
            report(&error)
        ),
    }

    let module = bounded::<H>(host.bad_credentials(), PATIENT);
    let (error, took) = refused(
        builder::<H>(Root::new().import(module)),
        "an app with bad credentials",
        "the server accepted the bad credentials",
    )
    .await;
    assert_permanent_at_once::<H>(&error, took, "bad credentials in the loop around `create`");
}

/// A refusal of `what` that ended the store's one retry loop at its first permanent error: permanent,
/// `Errored`, one attempt where a readiness check failed, well inside [`PATIENT`], the password
/// redacted.
fn assert_permanent_at_once<H: Host>(error: &StartupError, took: Duration, what: &str) {
    let Some((connect, reason)) = connect_failure(error) else {
        panic!("{what} failed outside `connect`: {}", report(error));
    };
    assert!(is_permanent(error), "{what} carried no `Permanent` after {took:?}: {}", report(error));
    assert!(
        matches!(reason, FailureReason::Errored(_)),
        "{what} ended the connect with `{reason}` after {took:?}, not with the store's own error"
    );
    match connect {
        ConnectError::Readiness { attempts, .. } => {
            assert_eq!(*attempts, 1, "{what} were retried: the readiness check made {attempts} attempts in {took:?}");
        }
        ConnectError::Construct { .. } => assert!(
            <H::Store as Store>::CREATE_CONNECTS,
            "{what} failed the construction of a store whose `create` connects nothing: {connect}"
        ),
        other => panic!("{what} failed outside the construction and the readiness check: {other}"),
    }
    let at_once = PATIENT / 2;
    assert!(took < at_once, "{what} took {took:?} to refuse, retried toward the {PATIENT:?} bound: {connect}");
    crate::cases::assert_redacted(error, what);
}
