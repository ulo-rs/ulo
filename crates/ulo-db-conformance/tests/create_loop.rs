//! The suite's connect scenarios over a store whose `create` connects. No shipped host's options
//! make `Store::create_connects` true, so without this store nothing runs `ulo-db`'s loop around
//! `create`. Its options name the outcome: `"up"` connects unless the host has cut it, `"down"`
//! never connects, `"refused"` is a permanent refusal.

use std::sync::atomic::{AtomicBool, Ordering};

use ulo::{BoxError, BoxFuture, Permanent};
use ulo_config::Problems;
use ulo_conformance_support::OnTokio;
use ulo_db::{LazyStore, Store, StoreModule};
use ulo_db_conformance::MigrationProbe;

static OUT: AtomicBool = AtomicBool::new(false);

struct Connecting;

impl LazyStore for Connecting {}

impl Store for Connecting {
    const NAME: &'static str = "connecting";
    type Options = String;
    type Client = ();

    fn create_connects(_: &String) -> bool {
        true
    }

    fn validate(o: &String) -> Result<(), Problems> {
        let mut problems = Problems::new();
        if !matches!(o.as_str(), "up" | "down" | "refused") {
            problems.at("", "not one of `up`, `down`, `refused`");
        }
        problems.into_result()
    }

    fn create(o: &String) -> BoxFuture<'static, Result<(), BoxError>> {
        let o = o.clone();
        Box::pin(async move {
            match o.as_str() {
                "up" if !OUT.load(Ordering::SeqCst) => Ok(()),
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

struct Host;

impl ulo_db_conformance::Host for Host {
    type Harness = OnTokio;
    type Store = Connecting;

    async fn start() -> Self {
        Host
    }

    fn options(&self) -> String {
        "up".to_owned()
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
}

ulo_db_conformance::conformance_suite!(Host; not_applicable {
    malformed_url_is_refused: "the options are no URL",
    lazy_readiness_waits_for_the_first_ping: "`create` connects, so `.lazy()` is refused",
    health_goes_down_in_an_outage_and_recovers: "covered by the stores; this host exists for the loop around `create`",
    pool_closes_after_the_drain: "covered by the stores",
    connecting_does_not_stall_the_thread: "covered by the stores",
    unsupported_runtime_is_refused: "covered by the stores",
    two_keyed_instances_stay_separate: "covered by the stores",
    migrations_run_only_on_request: "no migrator",
    migrations_run_once_across_replicas: "no migrator",
});
