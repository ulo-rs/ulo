//! The suite's connect scenarios over a store whose `create` connects. No shipped host's options
//! make `Store::create_connects` true, so without this store nothing runs `ulo-db`'s loop around
//! `create`. Its options name the outcome: `"up"` connects unless the host has cut it, `"steady"`
//! connects whatever the cut, `"down"` never connects, `"refused"` is a permanent refusal.
//!
//! The store takes neither `.lazy()` nor migrations, and the stamped host declares both absent:
//! the lazy scenario asserts that `.lazy()` is refused at `wire` on its options, and the suite
//! skips the migration scenarios from the declaration. The same host declaring nothing runs them,
//! and each fails at what the store cannot do.

use std::sync::atomic::{AtomicBool, Ordering};

use ulo::{BoxError, BoxFuture, Permanent};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio};
use ulo_db::{Store, StoreModule};
use ulo_db_conformance::{Capabilities, MigrationProbe};

static OUT: AtomicBool = AtomicBool::new(false);

struct Connecting;

impl Store for Connecting {
    const NAME: &'static str = "connecting";
    type Options = String;
    type Client = ();

    fn create_connects(_: &String) -> bool {
        true
    }

    fn validate(o: &String) -> Result<(), Problems> {
        let mut problems = Problems::new();
        if !matches!(o.as_str(), "up" | "steady" | "down" | "refused") {
            problems.at("", "not one of `up`, `steady`, `down`, `refused`");
        }
        problems.into_result()
    }

    fn create(o: &String) -> BoxFuture<'static, Result<(), BoxError>> {
        let o = o.clone();
        Box::pin(async move {
            match o.as_str() {
                "up" if !OUT.load(Ordering::SeqCst) => Ok(()),
                "steady" => Ok(()),
                "refused" => Err(Permanent::new(std::io::Error::other("credentials refused")).into()),
                _ => Err("no answer".into()),
            }
        })
    }

    fn ping(_: &()) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(_: ()) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

/// The environment over [`Connecting`], declaring its capabilities absent where `DECLARED`.
struct Host<const DECLARED: bool>;

impl<const DECLARED: bool> ulo_db_conformance::Host for Host<DECLARED> {
    type Harness = OnTokio;
    type Store = Connecting;

    async fn start() -> Self {
        Host
    }

    /// `"steady"` on the host declaring nothing, so a stamped scenario cutting the store does not
    /// reach the two tests run on it.
    fn options(&self) -> String {
        if DECLARED { "up" } else { "steady" }.to_owned()
    }

    fn unreachable_options(&self) -> String {
        "down".to_owned()
    }

    fn bad_credentials(&self) -> String {
        "refused".to_owned()
    }

    fn malformed_options(&self, password: &str) -> String {
        format!("{password}?")
    }

    async fn cut(&self) {
        OUT.store(true, Ordering::SeqCst);
    }

    async fn restore(&self) {
        OUT.store(false, Ordering::SeqCst);
    }

    fn lazily(&self, module: StoreModule<Connecting>) -> StoreModule<Connecting> {
        module.lazy()
    }

    fn migrating(&self, module: StoreModule<Connecting>, _run: MigrationProbe) -> StoreModule<Connecting> {
        module
    }

    fn capabilities(&self) -> Capabilities {
        if DECLARED {
            Capabilities::new().without_lazy_start("`create` connects, so `.lazy()` is refused").without_migrations("no migrator")
        } else {
            Capabilities::new()
        }
    }
}

/// A skip counts as a pass, so the gate is held from the other side too: on the host declaring
/// nothing the lazy scenario runs and fails at its check that the options leave `create` lazy.
/// Planted break: the lazy scenario skipping whatever the declaration.
#[test]
#[should_panic(expected = "whose `create` connects for the host's options")]
fn the_lazy_scenario_runs_on_a_host_declaring_nothing() {
    OnTokio::block_on(ulo_db_conformance::cases::lazy::waits_for_the_first_ping::<Host<false>>());
}

/// On the host declaring nothing a migration scenario runs, and fails finding no migration
/// applied, since the store has no migrator. Planted break: the migration scenarios skipping
/// whatever the declaration.
#[test]
#[should_panic(expected = "the requested migrations were not finished")]
fn a_migration_scenario_runs_on_a_host_declaring_nothing() {
    OnTokio::block_on(ulo_db_conformance::cases::migrate::only_on_request::<Host<false>>());
}

ulo_db_conformance::conformance_suite!(Host<true>; not_applicable {
    malformed_url_is_refused: "the options are no URL",
    health_goes_down_in_an_outage_and_recovers: "covered by the stores; this host exists for the loop around `create`",
    pool_closes_after_the_drain: "covered by the stores",
    connecting_does_not_stall_the_thread: "covered by the stores",
    unsupported_runtime_is_refused: "covered by the stores",
    two_keyed_instances_stay_separate: "covered by the stores",
});
