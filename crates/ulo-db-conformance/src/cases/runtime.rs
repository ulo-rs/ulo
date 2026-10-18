use std::time::{Duration, Instant};

use ulo::{App, AppBuilder, BoxFuture, ConnectError, Runtime, Signal, Spawn, TaskHandle, Timer, is_permanent};
use ulo_db::{Store, UnsupportedRuntime};

use crate::cases::{PATIENT, Root, bounded, carried, connect_failure, connecting};
use crate::{Harness, Host, report};

/// The harness's runtime under a type that declares no name of its own, so `Spawn::name` answers
/// its default, `"unknown"`.
struct Unnamed<R>(R);

impl<R: Runtime> Timer for Unnamed<R> {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Timer::sleep(&self.0, d)
    }

    fn now(&self) -> Instant {
        Timer::now(&self.0)
    }
}

impl<R: Runtime> Spawn for Unnamed<R> {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        Spawn::spawn(&self.0, fut)
    }
}

/// The harness's runtime answering `"smol"`, the name of a runtime the core supports and no store
/// does.
struct Smol<R>(R);

impl<R: Runtime> Timer for Smol<R> {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Timer::sleep(&self.0, d)
    }

    fn now(&self) -> Instant {
        Timer::now(&self.0)
    }
}

impl<R: Runtime> Spawn for Smol<R> {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        Spawn::spawn(&self.0, fut)
    }

    fn name(&self) -> &'static str {
        "smol"
    }
}

/// An unsupported runtime is refused at `connect`, `Permanent`, as `UnsupportedRuntime`. Planted
/// break: the name check removed.
///
/// Each app runs on the harness's own runtime, which is tokio for every store, under a wrapper that
/// changes nothing but the name `Spawn::name` answers: the default `"unknown"`, then `"smol"`. The
/// options reach the server and the module retries without counting out under a twenty-second
/// bound, so only the name can refuse the start, and it has to do so before anything is built. Each
/// refusal is the pool's construction failing at once with an `UnsupportedRuntime` inside a
/// `Permanent`, naming the module, `"tokio"` as the runtime it needs and the name it was given.
/// Without the name check the store connects, since the runtime underneath is tokio.
pub async fn unsupported_runtime_is_refused<H: Host>() {
    let host = H::start().await;
    let unnamed = App::builder(Root::new().import(bounded::<H>(host.options(), PATIENT))).runtime(Unnamed(runtime::<H>()));
    refused_on::<H>(unnamed, "unknown").await;
    let smol = App::builder(Root::new().import(bounded::<H>(host.options(), PATIENT))).runtime(Smol(runtime::<H>()));
    refused_on::<H>(smol, "smol").await;
}

fn runtime<H: Host>() -> <H::Harness as Harness>::Runtime {
    <H::Harness as Harness>::runtime()
}

/// Asserts that `builder`'s app, on a runtime answering `given`, is refused as the scenario states.
async fn refused_on<H: Host>(builder: AppBuilder, given: &'static str) {
    let what = format!("an app on a runtime named `{given}`");
    let (outcome, took) = connecting(builder, &what).await;
    let error = match outcome {
        Err(error) => error,
        Ok(app) => {
            let _ = app.close(Signal::new("conformance")).await;
            panic!("{what} connected after {took:?}: `{}` did not check the runtime's name", <H::Store as Store>::NAME);
        }
    };
    let Some((connect, _)) = connect_failure(&error) else {
        panic!("{what} failed outside `connect`: {}", report(&error));
    };
    assert!(
        matches!(connect, ConnectError::Construct { .. }),
        "{what} was refused outside the pool's construction, where the name is checked before anything is built: {connect}"
    );
    assert!(is_permanent(&error), "{what} was refused without a `Permanent`: {}", report(&error));
    let Some(refusal) = carried::<UnsupportedRuntime>(&error) else {
        panic!("{what} was refused without an `UnsupportedRuntime`: {}", report(&error));
    };
    assert_eq!(refusal.required, "tokio", "the refusal names the runtime the store needs: {refusal}");
    assert_eq!(refusal.given, given, "the refusal names the runtime the app was given: {refusal}");
    assert!(!refusal.module.is_empty(), "the refusal names no module: {refusal}");
    assert!(took < PATIENT / 2, "{what} took {took:?} to refuse, retried toward the {PATIENT:?} bound");
}
