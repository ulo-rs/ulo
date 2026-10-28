use std::marker::PhantomData;
use std::time::Duration;

use ulo::{Bound, BoxError, BoxFuture, Dep, Many, Module, ModuleDef, ModuleIdentity, Permanent, Runtime};
use ulo_config::{Config, ConfigError, Problems, Section};
use ulo_health::Check;

use crate::connect::{self, Policy};
use crate::error::{LazyRefused, ZeroConnectBound, ZeroMigrateBound};
use crate::health::{QualifierTag, StoreCheck, Tag, check_name};
use crate::migrate::{Bounds, Migrate, MigrationLock, Migrations};
use crate::store::Store;

const DEFAULT_CONNECT_WITHIN: Duration = Duration::from_secs(30);

/// No count of its own: `connect_within` is what ends the loop, so an unreachable database fails at
/// the bound however fast each attempt is refused.
const DEFAULT_RETRIES: u32 = u32::MAX;

const DEFAULT_BACKOFF: Duration = Duration::from_secs(1);

/// Added to the two migration bounds for the init hook's own, which covers the release after the
/// migrations (bounded inside `run` at five seconds) and the task's start and end.
const HOOK_MARGIN: Duration = Duration::from_secs(10);

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
/// per module. Options whose `create` connects nothing run `.ready(ping).retries(..).backoff(..)`
/// under that bound. Options whose `create` connects run `create` in `ulo-db`'s loop under it, the
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
    pub(crate) migrate_within: Bound,
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
            migrate_within: Bound::Default,
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
    /// core does not make on its own bounds. It also bounds the wait for the migration lock; the
    /// migrations have a bound of their own, [`migrate_within`](Self::migrate_within). A runtime
    /// that refuses the loop's bound or the lock's, having shut down, fails the start with
    /// `ulo::RuntimeShutDown` inside `ulo::Permanent`, in place of the bound passing.
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

    /// The constant wait between attempts, 1 s by default. A runtime that refuses it, having shut
    /// down, ends the loop with `ulo::RuntimeShutDown` inside `ulo::Permanent`, its source the last
    /// attempt's error.
    pub fn backoff(mut self, d: Duration) -> Self {
        self.backoff = d;
        self
    }
}

impl<S: Store> StoreModule<S> {
    /// Omits `.ready(..)`: the start does not wait for the database and the health check stays down
    /// until the first ping succeeds.
    ///
    /// Refused at registration, through `try_value` and naming the cause, when the options make
    /// `create` connect ([`Store::create_connects`]): a `mongodb+srv://` URI, whose DNS lookup is
    /// in `create`.
    pub fn lazy(mut self) -> Self {
        self.lazy = true;
        self
    }
}

impl<S: MigrationLock> StoreModule<S> {
    /// Runs `migrations` in the module's `on_init`, after connecting, under the lock. Off unless
    /// written. A failure is `MigrationFailed`.
    ///
    /// Two waits, each with its own bound. The wait for the lock is bounded by
    /// [`connect_within`](Self::connect_within) from the moment it is requested, and running out is
    /// retryable ([`MigrationFailed::waited_for_lock`](crate::MigrationFailed::waited_for_lock)):
    /// another replica is migrating. The migrations are bounded by
    /// [`migrate_within`](Self::migrate_within) from the moment the lock is granted, so a replica
    /// that waited out most of `connect_within` still gets all of `migrate_within`. Any other
    /// failure of the lock step is classified by the store, as its `ping` is; a failure after the
    /// lock is held is permanent. The init hook is bounded to cover both waits, so `hook_timeout`
    /// never cuts them.
    pub fn migrate(mut self, migrations: impl Migrations<S>) -> Self {
        self.migrations = Some(Migrate::new(migrations));
        self
    }

    /// The bound on the migrations themselves, counted from the moment the migration lock is
    /// granted. `Bound::Default`, the default, and `Bound::Unbounded` set none; `Bound::After(d)`
    /// fails the start with a permanent `MigrationFailed` when the migrations run past `d`. Zero is
    /// refused at registration. Without `.migrate(..)` it has no effect.
    pub fn migrate_within(mut self, bound: Bound) -> Self {
        self.migrate_within = bound;
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
        let check_path = path.to_owned();
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
        let connects = validated.as_ref().is_ok_and(|options| S::create_connects(options));
        if connects && self.lazy {
            let because = validated.as_ref().map_or("", |options| S::connects_because(options));
            m.try_value::<LazyRefused, _>(Err(LazyRefused::new(S::NAME, because)));
        }
        m.try_value(validated.map(|options| StoreOptions::<S> { options, _store: PhantomData }));

        // Under a key of the error's own type, which nothing depends on: `wire()` reports it
        // beside the options' problems rather than in their place.
        if self.connect_within.is_zero() {
            m.try_value::<ZeroConnectBound, _>(Err(ZeroConnectBound::new(S::NAME)));
        }
        if self.migrations.is_some() && self.migrate_within == Bound::After(Duration::ZERO) {
            m.try_value::<ZeroMigrateBound, _>(Err(ZeroMigrateBound::new(S::NAME)));
        }

        let policy = Policy { connect_within: self.connect_within, retries: self.retries, backoff: self.backoff };
        let client = m.try_singleton(move |stored: Dep<StoreOptions<S>>, runtime: Dep<dyn Runtime>| {
            connect::create::<S>(runtime.into_arc(), stored.options.clone(), policy)
        });
        if connects {
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

        let qualifier = m.qualifier();
        if self.health_check {
            m.contribute::<dyn Check>().singleton(
                move |client: Dep<S::Client>, runtime: Dep<dyn Runtime>, tags: Many<dyn QualifierTag>| {
                    let name = check_name::<S>(&check_path, qualifier, &tags);
                    let check = StoreCheck::<S>::new(name, runtime.into_arc(), (*client).clone());
                    async move { check }
                },
                |check| check,
            );
            if let Some(qualifier) = qualifier {
                // Read by every check, so the names that print alike can be written in full. Only a
                // module with a check contributes one: a lazily loaded module without its check
                // contributes to no collection, which `load` would refuse. After the check, so a
                // refused load names the check's collection rather than this one.
                m.contribute::<dyn QualifierTag>().singleton(move || async move { Tag(qualifier) }, |tag| tag);
            }
        }

        if let Some(migrate) = &self.migrations {
            let migrate = migrate.clone();
            let bounds = Bounds {
                lock: policy.connect_within,
                migrations: match self.migrate_within {
                    Bound::After(limit) => Some(limit),
                    Bound::Default | Bound::Unbounded => None,
                },
            };
            let hook = m.on_init(move |client: Dep<S::Client>, runtime: Dep<dyn Runtime>| {
                let run = migrate.run(runtime.into_arc(), (*client).clone(), bounds);
                async move {
                    run.await.map_err(|failed| {
                        // Retrying the start can mend a retryable failure: another replica holds
                        // the lock, or the connection was lost between the ping and the lock.
                        if failed.is_retryable() { BoxError::from(failed) } else { BoxError::from(Permanent::new(failed)) }
                    })
                }
            });
            // The module's own bounds, inside `run`, are the ones that end the step, so the failure
            // arrives as `MigrationFailed` rather than a hook timeout. A bounded step gets a hook
            // bound that covers both waits and the release.
            match bounds.migrations {
                Some(limit) => hook.timeout(policy.connect_within.saturating_add(limit).saturating_add(HOOK_MARGIN)),
                None => hook.unbounded(),
            };
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
