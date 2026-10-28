//! `ulo-db`'s loop around a `create` that connects paces its retries on the app's runtime. A
//! runtime that has shut down refuses the backoff, and the loop ends there with the last error:
//! on `sleep`, which ends at once on such a runtime, it would retry without pause until
//! `connect_within`. The store's `create` never connects, and the app's clock refuses the backoff.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Permanent};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio, ScriptedTimer};
use ulo_db::{Store, StoreModule};

const BACKOFF: Duration = Duration::from_secs(7);

/// The attempts after which a loop still retrying is taken to spin: `create` then refuses
/// permanently, which ends any loop.
const TURNS: usize = 3;

static CREATES: AtomicUsize = AtomicUsize::new(0);

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
            Err("no answer".into())
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
fn a_backoff_the_runtime_refuses_ends_the_create_loop() {
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
    assert!(failed.to_string().contains("no answer"), "the connect did not fail with the last attempt's error: {failed}");
}
