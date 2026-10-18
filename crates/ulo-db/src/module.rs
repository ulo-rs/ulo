use std::marker::PhantomData;
use std::time::Duration;

use ulo::{BoxError, BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Permanent, Runtime};
use ulo_config::{Config, ConfigError, Problems, Section};
use ulo_health::Check;

use crate::connect::{self, Policy};
use crate::error::ZeroConnectBound;
use crate::health::{StoreCheck, check_name};
use crate::migrate::{Migrate, MigrationLock, Migrations};
use crate::store::{LazyStore, Store};

const DEFAULT_CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// No count of its own: `connect_within` is what ends the loop, so an unreachable database fails at
/// the bound however fast each attempt is refused.
const DEFAULT_RETRIES: u32 = u32::MAX;

const DEFAULT_BACKOFF: Duration = Duration::from_secs(1);

/// The module every store crate names for its stores: `SqlxModule::<Postgres>`, `RedisModule`.
///
/// Its identity is by value over the store's options, so two equal configurations are one module
/// and a different URL is another. `Module::keyed::<Q>()` gives several instances of one store.
/// The builder settings are not part of the identity: two imports with equal options and different
/// settings are one module, registered with the settings of the first the graph reaches.
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
///
/// By default the loop has no count and waits 1 s between attempts, so the bound ends it. A single
/// attempt has no bound of its own inside `connect_within`: an attempt that waits on a pool's
/// acquire timeout spends that timeout, and a store keeps it below `connect_within`.
pub struct StoreModule<S: Store> {
    pub(crate) section: Section<S::Options>,
    pub(crate) connect_within: Duration,
    pub(crate) retries: u32,
    pub(crate) backoff: Duration,
    pub(crate) lazy: bool,
    pub(crate) health_check: bool,
    pub(crate) migrations: Option<Migrate<S>>,
    _store: PhantomData<fn() -> S>,
}

impl<S: Store> StoreModule<S> {
    /// Reads the section at `path` as the store's options. A failure is recorded and reported by
    /// `wire()` with every other problem.
    pub fn from_config(config: &Config, path: &str) -> Self {
        StoreModule::new(Section::read(config, path))
    }

    /// A module over options already in hand, for code and tests that build no `Config`.
    ///
    /// Nothing is read, so the module registers only the secrets [`Store::secrets`] answers. A
    /// `Secret` leaf the options hold beyond those, such as the whole URL, is registered by the
    /// importing module with `m.secret(..)`, which redacts it across the whole graph.
    pub fn from_options(options: S::Options) -> Self {
        StoreModule::new(Section::from_value("", options))
    }

    fn new(section: Section<S::Options>) -> Self {
        StoreModule {
            section,
            connect_within: DEFAULT_CONNECT_WITHIN,
            retries: DEFAULT_RETRIES,
            backoff: DEFAULT_BACKOFF,
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
    /// core does not make on its own bounds. It also bounds the migration lock, the migrations and
    /// the release together.
    pub fn connect_within(mut self, d: Duration) -> Self {
        self.connect_within = d;
        self
    }

    /// Attempts after the first, for the store's one retry loop. Unbounded by default, so
    /// `connect_within` ends the loop; a count ends it earlier, with the last attempt's error.
    pub fn retries(mut self, n: u32) -> Self {
        self.retries = n;
        self
    }

    /// The constant wait between attempts, 1 s by default.
    pub fn backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }
}

impl<S: LazyStore> StoreModule<S> {
    /// Omits `.ready(..)`: the start does not wait for the database and the health check stays down
    /// until the first ping succeeds.
    pub fn lazy(mut self) -> Self {
        const { assert!(!S::CREATE_CONNECTS, "a store whose `create` connects cannot be lazy") };
        self.lazy = true;
        self
    }
}

impl<S: MigrationLock> StoreModule<S> {
    /// Runs `migrations` in the module's `on_init`, after connecting, under the lock and its bound.
    /// Off unless written. A failure is `MigrationFailed` inside `Permanent`.
    ///
    /// The bound is [`connect_within`](Self::connect_within), counted from the moment the lock is
    /// requested: a replica waiting on another's migrations waits inside it, and a long migration
    /// needs a longer `connect_within`.
    pub fn migrate(mut self, migrations: impl Migrations<S>) -> Self {
        self.migrations = Some(Migrate::new(migrations));
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

/// The validated options, bound inside the module for the client's factory. A failed `validate`
/// leaves the binding failed, and `wire()` reports it before anything is built.
struct StoreOptions<S: Store> {
    options: S::Options,
    _store: PhantomData<fn() -> S>,
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
        let path = self.section.path();
        let validated: Result<S::Options, BoxError> = match self.section.register(m) {
            Ok(options) => {
                for secret in S::secrets(&options) {
                    m.secret(&secret);
                }
                match S::validate(&options) {
                    Ok(()) => Ok(options),
                    Err(problems) => Err(BoxError::from(under(path, problems))),
                }
            }
            Err(error) => Err(BoxError::from(error)),
        };
        m.try_value(validated.map(|options| StoreOptions::<S> { options, _store: PhantomData }));

        // Under a key of the error's own type, which nothing depends on: `wire()` reports it
        // beside the options' problems rather than in their place.
        if self.connect_within.is_zero() {
            m.try_value::<ZeroConnectBound, _>(Err(ZeroConnectBound::new(S::NAME)));
        }

        let policy = Policy { connect_within: self.connect_within, retries: self.retries, backoff: self.backoff };
        let client = m.try_singleton(move |stored: Dep<StoreOptions<S>>, runtime: Dep<dyn Runtime>| {
            connect::create::<S>(runtime.into_arc(), stored.options.clone(), policy)
        });
        if S::CREATE_CONNECTS {
            // The loop around `create` is the store's retrying, so the construction bound is the
            // total bound and the readiness check confirms the client once.
            client
                .timeout(policy.connect_within)
                .ready(ready_ping::<S>)
                .retries(0)
                .on_destroy(close::<S>);
        } else if self.lazy {
            client.on_destroy(close::<S>);
        } else {
            client
                .ready(ready_ping::<S>)
                .retries(policy.retries)
                .backoff(policy.backoff)
                .timeout(policy.connect_within)
                .on_destroy(close::<S>);
        }

        if self.health_check {
            let name = check_name::<S>(path);
            m.contribute::<dyn Check>().singleton(
                move |client: Dep<S::Client>, runtime: Dep<dyn Runtime>| {
                    let check = StoreCheck::<S>::new(name.clone(), runtime.into_arc(), (*client).clone());
                    async move { check }
                },
                |check| check,
            );
        }

        if let Some(migrate) = &self.migrations {
            let migrate = migrate.clone();
            let bound = policy.connect_within;
            // Unbounded as a hook so the module's own bound, inside `run`, is the one that ends it
            // and the failure arrives as `MigrationFailed` rather than a hook timeout.
            m.on_init(move |client: Dep<S::Client>, runtime: Dep<dyn Runtime>| {
                let run = migrate.run(runtime.into_arc(), (*client).clone(), bound);
                async move { run.await.map_err(Permanent::new) }
            })
            .unbounded();
        }

        m.export::<S::Client>();
    }
}

fn ready_ping<S: Store>(client: Dep<S::Client>, runtime: Dep<dyn Runtime>) -> BoxFuture<'static, Result<(), BoxError>> {
    connect::ping::<S>(runtime.into_arc(), (*client).clone())
}

fn close<S: Store>(client: Dep<S::Client>, runtime: Dep<dyn Runtime>) -> BoxFuture<'static, ()> {
    connect::close::<S>(runtime.into_arc(), (*client).clone())
}

/// `problems` with each path made absolute under the section's `path`: `Store::validate` writes
/// paths relative to the options, as a section read does before prefixing.
fn under(path: &str, problems: Problems) -> Problems {
    if path.is_empty() {
        return problems;
    }
    let mut absolute = Problems::new();
    for problem in problems.iter() {
        let at = match problem.path() {
            "" => path.to_owned(),
            index if index.starts_with('[') => format!("{path}{index}"),
            field => format!("{path}.{field}"),
        };
        absolute.at(at, problem.message());
    }
    absolute
}
