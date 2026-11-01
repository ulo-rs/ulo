//! Injection sites: the one rule fields, constructor parameters and factory parameters follow,
//! the four things a site can read, optional sites, owned state beside dependencies, a
//! constructor's failure, reading by type, and every holder of an instance holding the same
//! object.
//!
//! Capabilities pinned (CAPABILITIES.md ids): 11 (one rule for field, constructor and factory
//! sites), 12 (a site reads a service, a trait object, a collection or per-execution data), 13
//! (optional dependencies), 14 (owned state set by the constructor), 15 (a constructor's `Err` is
//! a startup error; its panic and timeout reasons belong to batch B4), 16 (a site is read by its
//! type, not its spelling), 17 (every holder holds the same object; a hook's effect is seen by
//! all; services need not be `Clone`).
//!
//! Capability 17 is listed under `scopes.rs` in the plan's row 17 and under batch B2 in section 5;
//! it is built here, in B2's file set, so batch B3's `scopes.rs` does not need it.

#[macro_use]
mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use ulo::scope::{Singleton, Transient};
use ulo::{
    ConnectError, Construct, ConstructError, Dep, Dependencies, ExecOptions, Ext, FailureReason, Key, Many, Resolver,
    StartupError,
};

struct A;
struct B;

struct Cfg(u32);

/// Declares its read of `Cfg` as a struct field.
struct FieldSvc {
    cfg: Dep<Cfg>,
}

impl Construct for FieldSvc {
    type Scope = Singleton;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(FieldSvc { cfg: r.dep::<Cfg>().await? })
    }
}

/// Declares its read of `Cfg` as a constructor parameter.
struct ParamSvc {
    cfg: Dep<Cfg>,
}

impl Construct for ParamSvc {
    type Scope = Singleton;

    fn dependencies(d: &mut Dependencies) {
        d.param::<Dep<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(ParamSvc { cfg: r.dep::<Cfg>().await? })
    }
}

#[tokio::test]
async fn field_ctor_and_factory_sites_resolve_alike() {
    struct FromFactory(Dep<Cfg>);

    // All three are registered before `Cfg`'s factory. Each is built only because the site it
    // declares was read by the same rule, whichever spelling declared it.
    let app = connects(Mod::<A>::new(|m| {
        m.provide::<FieldSvc>();
        m.provide::<ParamSvc>();
        m.singleton(|cfg: Dep<Cfg>| async move { FromFactory(cfg) });
        m.singleton(|| async { Cfg(1) });
    }))
    .await;

    let cfg = app.get::<Cfg>().await.unwrap();
    let field = app.get::<FieldSvc>().await.unwrap();
    let param = app.get::<ParamSvc>().await.unwrap();
    let factory = app.get::<FromFactory>().await.unwrap();
    for held in [&field.cfg, &param.cfg, &factory.0] {
        assert!(Arc::ptr_eq(held.arc(), cfg.arc()));
    }
}

trait Tr: Send + Sync {
    fn name(&self) -> &'static str;
}

struct Impl;

impl Tr for Impl {
    fn name(&self) -> &'static str {
        "impl"
    }
}

impl Construct for Impl {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(_r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Impl)
    }
}

trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
}

struct Named(&'static str);

impl Plugin for Named {
    fn id(&self) -> &'static str {
        self.0
    }
}

struct User(&'static str);

#[tokio::test]
async fn a_site_reads_each_of_the_four_shapes() {
    struct Shapes {
        service: u32,
        trait_object: &'static str,
        plugins: Vec<&'static str>,
        user: &'static str,
    }

    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { Cfg(4) });
        m.provide::<Impl>().also_as::<dyn Tr>(|a| a);
        m.contribute::<dyn Plugin>().value(Arc::new(Named("first")));
        m.contribute::<dyn Plugin>().value(Arc::new(Named("second")));
        m.execution(|cfg: Dep<Cfg>, tr: Dep<dyn Tr>, plugins: Many<dyn Plugin>, user: Ext<User>| async move {
            Shapes {
                service: cfg.0,
                trait_object: tr.name(),
                plugins: plugins.iter().map(|p| p.id()).collect(),
                user: user.0,
            }
        });
    }))
    .await;

    let shapes = app
        .execute(ExecOptions::new(), async |exec| {
            exec.extensions().insert(User("ann"));
            exec.get::<Shapes>().await
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shapes.service, 4);
    assert_eq!(shapes.trait_object, "impl");
    assert_eq!(shapes.plugins, ["first", "second"], "a `Many` site holds every contribution");
    assert_eq!(shapes.user, "ann");
}

#[tokio::test]
async fn an_optional_dependency_is_none_when_unbound_and_some_when_bound() {
    struct Missing;
    struct Seen {
        missing_is_none: bool,
        cfg: Option<u32>,
    }

    // `wires` is the check that an optional read of an unbound key is not a `Missing` entry.
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { Cfg(4) });
        m.singleton(|missing: Option<Dep<Missing>>, cfg: Option<Dep<Cfg>>| async move {
            Seen { missing_is_none: missing.is_none(), cfg: cfg.map(|c| c.0) }
        });
    }))
    .await;

    let seen = app.get::<Seen>().await.unwrap();
    assert!(seen.missing_is_none);
    assert_eq!(seen.cfg, Some(4));
}

#[tokio::test]
async fn an_optional_extension_is_none_until_something_writes_it() {
    struct Reads(Option<&'static str>);

    let app = connects(Mod::<A>::new(|m| {
        m.execution(|user: Option<Ext<User>>| async move { Reads(user.map(|u| u.0)) });
    }))
    .await;

    // Two standalone executions: the first writes nothing, the second writes the extension.
    let silent = app.execute(ExecOptions::new(), async |exec| exec.get::<Reads>().await.map(|r| r.0)).await.unwrap().unwrap();
    assert_eq!(silent, None);

    let written = app
        .execute(ExecOptions::new(), async |exec| {
            exec.extensions().insert(User("bob"));
            exec.get::<Reads>().await.map(|r| r.0)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(written, Some("bob"));
}

/// Owned state the constructor sets beside the dependency it reads.
struct Counted {
    cfg: Dep<Cfg>,
    id: usize,
    doubled: u32,
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

impl Construct for Counted {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let cfg = r.dep::<Cfg>().await?;
        Ok(Counted { id: NEXT_ID.fetch_add(1, Ordering::SeqCst), doubled: cfg.0 * 2, cfg })
    }
}

#[tokio::test]
async fn a_constructor_sets_owned_state_beside_its_dependencies() {
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { Cfg(21) });
        m.provide::<Counted>();
    }))
    .await;

    let first = app.get::<Counted>().await.unwrap();
    let second = app.get::<Counted>().await.unwrap();
    assert_eq!(first.doubled, 42, "owned state derived from the dependency");
    assert_ne!(first.id, second.id, "each instance has state of its own");
    assert!(Arc::ptr_eq(first.cfg.arc(), second.cfg.arc()), "the dependency is the one shared instance");
}

/// A constructor that awaits before it fails.
struct Fails;

impl Construct for Fails {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(_r: &Resolver<'_>) -> Result<Self, ConstructError> {
        std::future::ready(()).await;
        Err(ConstructError::failed(Boom::new("refused")))
    }
}

#[tokio::test]
async fn a_failing_constructor_is_a_startup_error_naming_the_binding() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.provide::<Fails>();
    }))
    .await;

    let StartupError::Connect(ConnectError::Construct { key, module, reason }) = error else {
        panic!("expected a construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<Fails, ()>());
    assert_eq!(module.to_string(), mod_name::<A>());
    assert!(matches!(reason, FailureReason::Errored(_)), "got: {reason}");
}

mod left {
    pub struct Pool(pub u32);
}

mod right {
    pub struct Pool(pub u32);
}

type Aliased = left::Pool;
use left::Pool as Renamed;

#[tokio::test]
async fn a_type_alias_and_a_renamed_import_read_the_same_key() {
    struct Seen {
        left: u32,
        right: u32,
        same: bool,
    }

    // `left::Pool` and `right::Pool` share a short name and are two keys; `Aliased` and `Renamed`
    // spell `left::Pool` and read its key.
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { left::Pool(1) });
        m.singleton(|| async { right::Pool(2) });
        m.singleton(|aliased: Dep<Aliased>, renamed: Dep<Renamed>, right: Dep<right::Pool>| async move {
            Seen { left: aliased.0, right: right.0, same: Arc::ptr_eq(aliased.arc(), renamed.arc()) }
        });
    }))
    .await;

    let seen = app.get::<Seen>().await.unwrap();
    assert_eq!((seen.left, seen.right), (1, 2));
    assert!(seen.same);
}

/// Deliberately not `Clone`.
struct Shared {
    log: Mutex<Vec<&'static str>>,
}

struct HolderOne {
    shared: Dep<Shared>,
}

struct HolderTwo {
    shared: Dep<Shared>,
}

#[tokio::test]
async fn two_holders_share_one_non_clone_singleton() {
    let app = connects(Mod::<B>::new(|m| {
        m.singleton(|shared: Dep<Shared>| async move { HolderOne { shared } });
        m.singleton(|shared: Dep<Shared>| async move { HolderTwo { shared } });
        m.singleton(|| async { Shared { log: Mutex::new(Vec::new()) } }).on_init(|shared: Dep<Shared>| async move {
            shared.log.lock().unwrap().push("init");
            Ok::<(), Boom>(())
        });
    }))
    .await;

    let one = app.get::<HolderOne>().await.unwrap();
    let two = app.get::<HolderTwo>().await.unwrap();
    let direct = app.get::<Shared>().await.unwrap();
    assert!(Arc::ptr_eq(one.shared.arc(), two.shared.arc()));
    assert!(Arc::ptr_eq(one.shared.arc(), direct.arc()));

    // The startup hook ran on the one object, and one holder's mutation reaches the others.
    assert_eq!(*two.shared.log.lock().unwrap(), ["init"]);
    one.shared.log.lock().unwrap().push("from one");
    assert_eq!(*two.shared.log.lock().unwrap(), ["init", "from one"]);
    assert_eq!(*direct.log.lock().unwrap(), ["init", "from one"]);
}
