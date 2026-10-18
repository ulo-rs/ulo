use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use ulo::{Module, ModuleDef, ModuleIdentity};
use ulo_config::{Config, ConfigError, Section};

use crate::migrate::{MigrationLock, Migrations};
use crate::store::{LazyStore, Store};

/// The module every store crate names for its stores: `SqlxModule::<Postgres>`, `RedisModule`.
///
/// Its identity is by value over the store's options, so two equal configurations are one module
/// and a different URL is another. `Module::keyed::<Q>()` gives several instances of one store.
///
/// # Registration
///
/// `S::validate` runs on the options and its problems join the section's through `try_value`; the
/// URL's secrets are registered; `try_singleton` builds the client, after the runtime's name is
/// checked; unless the module is lazy, `.ready(ping)` retries under the policy below; a check is
/// contributed unless `.without_health_check()`; `.on_destroy` closes the client; the bare
/// `S::Client` is exported.
///
/// # The bounded connect
///
/// One total bound, [`connect_within`](Self::connect_within), 30 s by default, and one retry loop
/// per store. A store whose `create` connects nothing runs `.ready(ping).retries(..).backoff(..)`
/// under that bound. A store whose `create` connects runs `create` in `ulo-db`'s loop under it, the
/// construction bound set to the same value, then one confirming ping with `.retries(0)` under the
/// default attempt bound, which falls outside `connect_within`. Backoff is constant. An attempt
/// returning `Permanent` ends the loop at once; the bound reached is retryable.
pub struct StoreModule<S: Store> {
    pub(crate) section: Section<S::Options>,
    pub(crate) connect_within: Duration,
    pub(crate) retries: u32,
    pub(crate) backoff: Duration,
    pub(crate) lazy: bool,
    pub(crate) health_check: bool,
    pub(crate) migrations: Option<Arc<dyn Migrations<S>>>,
    _store: PhantomData<fn() -> S>,
}

impl<S: Store> StoreModule<S> {
    /// Reads the section at `path` as the store's options. A failure is recorded and reported by
    /// `wire()` with every other problem.
    pub fn from_config(config: &Config, path: &str) -> Self {
        StoreModule::new(Section::read(config, path))
    }

    /// A module over options already in hand, for code and tests that build no `Config`.
    pub fn from_options(options: S::Options) -> Self {
        StoreModule::new(Section::from_value("", options))
    }

    fn new(section: Section<S::Options>) -> Self {
        StoreModule {
            section,
            connect_within: Duration::from_secs(30),
            retries: 0,
            backoff: Duration::ZERO,
            lazy: false,
            health_check: true,
            migrations: None,
            _store: PhantomData,
        }
    }

    /// Leaves the store's check out. A module without a contribution can be loaded lazily; with one,
    /// `load` refuses it with `LoadRefusal::Contribution`, at the cost here of a readiness that
    /// cannot see the store.
    pub fn without_health_check(mut self) -> Self {
        self.health_check = false;
        self
    }

    /// The total connect bound, 30 s by default. Zero is refused at registration, a refusal the
    /// core does not make on its own bounds.
    pub fn connect_within(mut self, d: Duration) -> Self {
        self.connect_within = d;
        self
    }

    /// Attempts after the first, for the store's one retry loop.
    pub fn retries(mut self, n: u32) -> Self {
        self.retries = n;
        self
    }

    /// The constant wait between attempts.
    pub fn backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }
}

impl<S: LazyStore> StoreModule<S> {
    /// Omits `.ready(..)`: the start does not wait for the database and the health check stays down
    /// until the first ping succeeds.
    pub fn lazy(mut self) -> Self {
        self.lazy = true;
        self
    }
}

impl<S: MigrationLock> StoreModule<S> {
    /// Runs `migrations` in the module's `on_init`, after connecting, under the lock and its bound.
    /// Off unless written. A failure is `MigrationFailed` inside `Permanent`.
    pub fn migrate(mut self, migrations: impl Migrations<S>) -> Self {
        self.migrations = Some(Arc::new(migrations));
        self
    }
}

/// What distinguishes one module from another: the options when they were read, the failure when
/// they were not.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Identity<O> {
    Options(O),
    Failed(String, ConfigError),
}

impl<S> Module for StoreModule<S>
where
    S: Store,
    S::Options: 'static,
{
    fn identity(&self) -> ModuleIdentity {
        let key = match self.section.value() {
            Ok(options) => Identity::Options(options.clone()),
            Err(error) => Identity::Failed(self.section.path().to_owned(), error.clone()),
        };
        ModuleIdentity::of_value(&key)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
        todo!()
    }
}
