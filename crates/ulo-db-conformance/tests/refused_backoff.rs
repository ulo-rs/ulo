//! `ulo-db`'s loop around a `create` that connects paces its retries on the app's runtime. A
//! runtime that has shut down refuses the backoff, and the loop ends there, reporting the refusal:
//! a `Permanent` `RuntimeShutDown` whose source is the last attempt's error. On `sleep`, which ends
//! at once on such a runtime, it would retry without pause until `connect_within`. The store's
//! `create` never connects, and the app's clock refuses the backoff.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ulo::{
    App, BoxError, BoxFuture, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Permanent, RUNTIME_SHUT_DOWN,
    RuntimeShutDown, StartupError, is_permanent,
};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio, ScriptedTimer};
use ulo_db::{Store, StoreModule};

const BACKOFF: Duration = Duration::from_secs(7);

/// The attempts after which a loop still retrying is taken to spin: `create` then refuses
/// permanently, which ends any loop.
const TURNS: usize = 3;

static CREATES: AtomicUsize = AtomicUsize::new(0);

/// What every attempt short of the cap fails with.
#[derive(Debug)]
struct NoAnswer;

impl fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no answer")
    }
}

impl Error for NoAnswer {}

struct Unreachable;

impl Store for Unreachable {
    const NAME: &'static str = "unreachable";
    type Options = String;
    type Client = ();

    fn create_connects(_: &String) -> bool {
        true
    }

    fn validate(_: &String) -> Result<(), Problems> {
        Ok(())
    }

    fn create(_: &String) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async {
            if CREATES.fetch_add(1, Ordering::SeqCst) + 1 > TURNS {
                return Err(Permanent::new(std::io::Error::other("spun")).into());
            }
            Err(NoAnswer.into())
        })
    }

    fn ping(_: &()) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(_: ()) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(StoreModule::<Unreachable>::from_options("down".to_owned()).backoff(BACKOFF));
    }
}

#[test]
fn a_backoff_the_runtime_refuses_ends_the_create_loop_with_the_refusal() {
    let clock = ScriptedTimer::new();
    clock.refuse(BACKOFF);
    let timer = clock.clone();
    let failed = <OnTokio as Harness>::block_on(async move {
        let app = App::builder(Root).runtime(<OnTokio as Harness>::runtime()).timer(timer).wire().expect("wires");
        app.connect().await.err().expect("the app connected with a store that never connects")
    });
    assert_eq!(
        (CREATES.load(Ordering::SeqCst), clock.refused(BACKOFF)),
        (1, 1),
        "the creates and backoffs on a runtime refusing the backoff: the loop retried rather than ending ({failed})"
    );
    assert!(is_permanent(&failed), "a loop ended on a refused backoff is retryable: {failed}");
    let StartupError::Connect(ConnectError::Construct { reason: FailureReason::Errored(stored), .. }) = &failed else {
        panic!("the loop did not fail the construction with an error: {failed:?}");
    };
    let ended = stored
        .downcast_ref::<Permanent>()
        .and_then(|permanent| permanent.source())
        .and_then(|inner| inner.downcast_ref::<RuntimeShutDown>())
        .unwrap_or_else(|| panic!("the construction did not fail with a `Permanent` `RuntimeShutDown`: {failed:?}"));
    assert_eq!(ended.reason(), RUNTIME_SHUT_DOWN);
    assert!(ended.source().is_some_and(|last| last.is::<NoAnswer>()), "the refusal's source is not the last attempt's error: {ended:?}");
}
