//! The conformance suite every `ulo-db` store runs (integrations DESIGN §4.9): one scenario list,
//! stamped per store by [`conformance_suite!`] from a [`Host`] the store crate implements in its
//! `tests/`, against a real server in a container, or a file for SQLite.
//!
//! ```ignore
//! // crates/ulo-db-sqlx/tests/conformance.rs
//! struct PostgresHost { /* a fresh container */ }
//!
//! impl ulo_db_conformance::Host for PostgresHost {
//!     type Harness = ulo_db_conformance::OnTokio;
//!     type Store = ulo_db_sqlx::Postgres;
//!     async fn start() -> Self { /* a fresh server */ }
//!     fn options(&self) -> PgOptions { /* reaching it */ }
//!     /* ... */
//! }
//!
//! ulo_db_conformance::conformance_suite!(PostgresHost);
//! ```
//!
//! A store that lacks a capability, [`Capabilities::lazy`] or [`Capabilities::migrations`], declares
//! the scenarios that need it not applicable with its reason, stamped `#[ignore]` so the report
//! counts them as ignored, and run on a store they do not apply to they fail. Every scenario names
//! the planted break it is shown failing against.
//!
//! The suite names no runtime: a host's [`Harness`] is its runtime half, as the RPC suite's is. It
//! shares that suite's pieces through `ulo-conformance-support`: [`Budget`] with its `stall`,
//! `block_on_one_thread`, [`startup_failed!`], [`report`], [`relay`] and the `not_applicable`
//! stamping rules. A host short of memory bounds how many scenarios hold a server at once with
//! [`PARALLEL_VAR`].

use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ulo_db::{Store, StoreModule};

pub mod cases;

#[cfg(feature = "tokio")]
pub use ulo_conformance_support::OnTokio;
pub use ulo_conformance_support::{Budget, Harness, PARALLEL_VAR, failures_dir, relay, report, startup_failed};
// The macro expands to `$crate::__private::Slots`.
#[doc(hidden)]
pub use ulo_conformance_support::__private;

/// The password the suite asks a host to put in the URLs it plants, so a scenario can assert that no
/// refusal or error prints it.
pub const PASSWORD: &str = "hunter2";

/// What a store offers, named positively and true by default, so a store lacking one declares
/// `not_applicable` with its reason.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The store offers `.lazy()`: `CREATE_CONNECTS` is false.
    pub lazy: bool,
    /// The store offers `.migrate(..)`.
    pub migrations: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities { lazy: true, migrations: true }
    }
}

impl Capabilities {
    pub const fn lazy(self, on: bool) -> Self {
        Capabilities { lazy: on, ..self }
    }

    pub const fn migrations(self, on: bool) -> Self {
        Capabilities { migrations: on, ..self }
    }
}

/// Counts how often the migrations a host supplies ran, so the lock scenario can assert that two
/// replicas racing against one server apply them once.
#[derive(Clone, Debug, Default)]
pub struct MigrationProbe {
    runs: Arc<AtomicUsize>,
}

impl MigrationProbe {
    pub fn new() -> Self {
        MigrationProbe::default()
    }

    /// Called by the host's migration body each time it runs.
    pub fn record(&self) {
        self.runs.fetch_add(1, Ordering::SeqCst);
    }

    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

/// One store's environment for the suite: a server in a container, or for SQLite a file.
pub trait Host: Sized + Send + Sync + 'static {
    /// The runtime the scenarios run on: `OnTokio`, behind the `tokio` feature.
    type Harness: Harness;

    type Store: Store;

    /// How many scenarios hold an environment at once; `None`, the default, leaves the scenarios to
    /// libtest's parallelism. [`PARALLEL_VAR`] bounds it further on one host.
    const PARALLEL: Option<NonZeroUsize> = None;

    /// Starts the environment for one scenario, so no state leaks between scenarios: a fresh
    /// container, or a fresh file. A container reached at a published port starts through
    /// [`relay::unshadowed`], and a start that fails does so through [`startup_failed!`].
    fn start() -> impl Future<Output = Self> + Send;

    /// Options that reach the server, through a relay the host cuts for the outage scenarios.
    fn options(&self) -> <Self::Store as Store>::Options;

    /// Options naming a host nothing answers at, whose connection attempts fail as unreachable and
    /// not as refused credentials.
    fn unreachable_options(&self) -> <Self::Store as Store>::Options;

    /// Options that reach the server with credentials it refuses.
    fn bad_credentials(&self) -> <Self::Store as Store>::Options;

    /// Options the store's `validate` refuses, with `password` as the URL's password.
    fn malformed_options(&self, password: &str) -> <Self::Store as Store>::Options;

    /// Begins the outage: connections through [`options`](Self::options) fail until
    /// [`restore`](Self::restore).
    fn cut(&self) -> impl Future<Output = ()> + Send;

    fn restore(&self) -> impl Future<Output = ()> + Send;

    /// `module.lazy()` for a store that offers it; a store that does not returns `module` unchanged
    /// and declares the lazy scenarios not applicable. Written as a method because `.lazy()` exists
    /// only where `CREATE_CONNECTS` is false.
    fn lazily(&self, module: StoreModule<Self::Store>) -> StoreModule<Self::Store>;

    /// `module.migrate(..)` with the migrations the lock scenario runs, whose body calls
    /// `run.record()`; a store without a migrator returns `module` unchanged.
    fn migrating(&self, module: StoreModule<Self::Store>, run: MigrationProbe) -> StoreModule<Self::Store>;

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    /// How long this environment takes, which a slow server raises.
    fn budget(&self) -> Budget {
        Budget::default()
    }
}

/// Stamps every scenario as a `#[test]` for the host type `$host`, each running its scenario through
/// the host's [`Harness::block_on`].
///
/// A scenario that cannot apply to a store is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`:
///
/// ```ignore
/// ulo_db_conformance::conformance_suite!(RedisHost; not_applicable {
///     migrations_run_only_on_request: "Redis has no migrator",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile.
#[macro_export]
macro_rules! conformance_suite {
    ($host:ty $(,)?) => {
        $crate::conformance_suite!($host; not_applicable {});
    };
    ($host:ty; not_applicable { $($skip:ident : $why:literal),* $(,)? } $(,)?) => {
        $crate::conformance_suite!(@stamper [$] $host; $($skip : $why),*);
    };
    (@stamper [$d:tt] $host:ty; $($skip:ident : $why:literal),*) => {
        macro_rules! __ulo_db_conformance_stamp {
            $(
                ($skip, $d($d test:tt)*) => {
                    #[test]
                    #[ignore = concat!("not applicable: ", $why)]
                    $d($d test)*
                };
            )*
            ($d other:ident, $d($d test:tt)*) => {
                #[test]
                $d($d test)*
            };
        }
        // A declared name that is no scenario names no function here.
        $(const _: fn() = $skip;)*
        $crate::conformance_suite!(@cases $host;
            malformed_url_is_refused => wire::malformed_url_is_refused,
            unreachable_database_fails_at_the_bound => connect::unreachable_fails_at_the_bound,
            bad_credentials_fail_at_once => connect::bad_credentials_are_permanent,
            a_connecting_create_is_retried_and_stops_on_permanent => connect::create_loop,
            lazy_readiness_waits_for_the_first_ping => lazy::waits_for_the_first_ping,
            health_goes_down_in_an_outage_and_recovers => health::outage_and_recovery,
            pool_closes_after_the_drain => close::after_the_drain,
            connecting_does_not_stall_the_thread => stall::connecting_does_not_stall_the_thread [block_on_one_thread],
            unsupported_runtime_is_refused => runtime::unsupported_runtime_is_refused,
            two_keyed_instances_stay_separate => keyed::two_instances,
            migrations_run_only_on_request => migrate::only_on_request,
            migrations_run_once_across_replicas => migrate::once_across_replicas,
        );
    };
    (@cases $host:ty; $($name:ident => $module:ident :: $case:ident $([$runner:ident])?),* $(,)?) => {
        static __ULO_DB_CONFORMANCE_SLOTS: $crate::__private::Slots = $crate::__private::Slots::new();
        $(
            __ulo_db_conformance_stamp!($name, fn $name() {
                let _slot = __ULO_DB_CONFORMANCE_SLOTS.hold(<$host as $crate::Host>::PARALLEL);
                $crate::conformance_suite!(@run $host, $crate::cases::$module::$case::<$host>() $(, $runner)?);
            });
        )*
    };
    // A case runs through `Harness::block_on` unless its entry names another runner.
    (@run $host:ty, $case:expr) => {
        $crate::conformance_suite!(@run $host, $case, block_on)
    };
    (@run $host:ty, $case:expr, $runner:ident) => {
        <<$host as $crate::Host>::Harness as $crate::Harness>::$runner($case)
    };
}
