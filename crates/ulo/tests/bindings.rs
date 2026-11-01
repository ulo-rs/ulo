//! What a binding is once the graph is wired and connected: a built type, a value, an async
//! factory, an implementation under a trait, qualified bindings of one type, an alias,
//! contributions and a foreign type. Each test reads the bound object back and asserts what it is
//! and which object it is, never message text.
//!
//! Capabilities pinned (CAPABILITIES.md ids): 1 (a `Construct` type is built with its
//! dependencies injected), 2 (a value under a key), 3 (an async factory whose parameters are its
//! dependencies, and whose output is the binding), 4 (an implementation under a trait, injected by
//! the trait), 5 (qualified bindings of one type are told apart), 6 (an alias reaches the same
//! instance), 7 (contributions from several modules form one list, in collection order), 8 (a
//! foreign type is bound by value and by factory), 15 (the part of a factory failing as a startup
//! error that a binding owns; the panic and timeout reasons are batch B4's).
//!
//! Capability 9 (no string keys) is a compile-time property and lives in `ulo-codegen-tests`.

#[macro_use]
mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use common::*;
use ulo::scope::Singleton;
use ulo::{ConnectError, Construct, ConstructError, Dep, FailureReason, Key, LookupError, LookupKind, Many, Resolver, StartupError};

struct A;
struct B;
struct C;
struct Primary;
struct Replica;
struct ReadOnly;

struct Cfg(u32);

/// Reads `Cfg` in its constructor and declares the read as a field.
struct Svc {
    cfg: Dep<Cfg>,
}

impl Construct for Svc {
    type Scope = Singleton;

    fn dependencies(d: &mut ulo::Dependencies) {
        d.field::<Dep<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Svc { cfg: r.dep::<Cfg>().await? })
    }
}

trait Repo: Send + Sync {
    fn name(&self) -> &'static str;
}

struct PgRepo;

impl Repo for PgRepo {
    fn name(&self) -> &'static str {
        "pg"
    }
}

impl Construct for PgRepo {
    type Scope = Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(PgRepo)
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

struct Built(&'static str);

impl Plugin for Built {
    fn id(&self) -> &'static str {
        self.0
    }
}

impl Construct for Built {
    type Scope = Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Built("provided"))
    }
}

#[derive(Debug, PartialEq)]
struct Db(&'static str);

async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

#[tokio::test]
async fn a_construct_type_is_built_with_its_dependencies() {
    // `Svc` is registered before the factory that builds `Cfg`, and a factory is not built until
    // `connect` reaches it. `Svc` can be built only because its declared field orders `Cfg` first.
    let app = connects(Mod::<A>::new(|m| {
        m.provide::<Svc>();
        m.singleton(|| async { Cfg(7) });
    }))
    .await;

    let svc = app.get::<Svc>().await.unwrap();
    let cfg = app.get::<Cfg>().await.unwrap();
    assert_eq!(svc.cfg.0, 7);
    assert!(Arc::ptr_eq(svc.cfg.arc(), cfg.arc()), "the injected dependency is the bound instance");
}

#[tokio::test]
async fn a_value_is_read_back_by_its_key() {
    let app = connects(Mod::<A>::new(|m| {
        m.value(Cfg(3));
        m.value(String::from("name"));
        m.value(5_u32);
        m.provide::<Svc>();
    }))
    .await;

    assert_eq!(app.get::<Cfg>().await.unwrap().0, 3);
    assert_eq!(app.get::<String>().await.unwrap().as_str(), "name");
    assert_eq!(*app.get::<u32>().await.unwrap(), 5, "each value is read under its own type");

    // One object behind the key: every read and every holder gets the bound `Arc`.
    let first = app.get::<Cfg>().await.unwrap();
    let second = app.get::<Cfg>().await.unwrap();
    let held = app.get::<Svc>().await.unwrap();
    assert!(Arc::ptr_eq(first.arc(), second.arc()));
    assert!(Arc::ptr_eq(first.arc(), held.cfg.arc()));
}

#[tokio::test]
async fn a_factory_s_parameters_are_its_dependencies_and_its_output_the_binding() {
    struct Sum(u32);
    struct Left(u32);
    struct Right(u32);

    // The factory is registered first; its parameters are the only thing that orders the other two
    // before it. Its future is awaited, and its output type is the key it binds.
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|left: Dep<Left>, right: Dep<Right>| async move {
            yield_once().await;
            Sum(left.0 + right.0)
        });
        m.singleton(|| async { Left(2) });
        m.singleton(|| async { Right(5) });
    }))
    .await;

    assert_eq!(app.get::<Sum>().await.unwrap().0, 7);
    assert_eq!(app.get::<Left>().await.unwrap().0, 2, "a parameter stays a binding of its own");
    assert!(matches!(
        app.get::<(u32, u32)>().await,
        Err(LookupError::NotFound { kind: LookupKind::Binding, .. })
    ));
}

#[tokio::test]
async fn a_fallible_factory_binds_its_ok_type() {
    let app = connects(Mod::<A>::new(|m| {
        m.try_singleton(|| async { Ok::<_, Boom>(Cfg(11)) });
    }))
    .await;
    assert_eq!(app.get::<Cfg>().await.unwrap().0, 11);
}

#[tokio::test]
async fn a_fallible_factory_that_errs_fails_connect_with_the_binding_named() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.try_singleton(|| async { Err::<Cfg, _>(Boom::new("no connection")) });
    }))
    .await;

    let StartupError::Connect(ConnectError::Construct { key, module, reason }) = error else {
        panic!("expected a construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<Cfg, ()>());
    assert_eq!(module.to_string(), mod_name::<A>());
    assert!(matches!(reason, FailureReason::Errored(_)), "an `Err` is `Errored`, got: {reason}");
}

#[tokio::test]
async fn an_implementation_is_injected_by_its_trait() {
    // The implementation is bound under the trait, and the trait alone leaves the module.
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.provide::<PgRepo>().also_as::<dyn Repo>(|a| a);
            m.export::<dyn Repo>();
        }));
        m.provide::<Svc>();
        m.singleton(|| async { Cfg(0) });
    }))
    .await;

    let repo = app.get::<dyn Repo>().await.unwrap();
    assert_eq!(repo.name(), "pg");
    assert!(
        matches!(app.get::<PgRepo>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })),
        "the concrete key did not leave the module"
    );

    // Inside its module both keys reach the one object.
    let inside = app.module::<Mod<B>>().unwrap();
    let concrete = inside.get::<PgRepo>().await.unwrap();
    let by_trait = inside.get::<dyn Repo>().await.unwrap();
    assert_eq!(Arc::as_ptr(concrete.arc()) as *const (), Arc::as_ptr(by_trait.arc()) as *const ());
    assert_eq!(Arc::as_ptr(repo.arc()) as *const (), Arc::as_ptr(by_trait.arc()) as *const ());
}

#[tokio::test]
async fn qualified_bindings_of_one_type_are_separate() {
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { Db("primary") }).qualified::<Primary>();
        m.singleton(|| async { Db("replica") }).qualified::<Replica>();
        // A consumer names both qualifiers at its sites.
        m.singleton(|p: Dep<Db, Primary>, r: Dep<Db, Replica>| async move { (p.0, r.0) });
    }))
    .await;

    assert_eq!(*read_q::<Db, Primary>(&app).await.unwrap(), Db("primary"));
    assert_eq!(*read_q::<Db, Replica>(&app).await.unwrap(), Db("replica"));
    assert_eq!(*app.get::<(&'static str, &'static str)>().await.unwrap(), ("primary", "replica"));
    assert!(
        matches!(app.get::<Db>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })),
        "the unqualified key is a third key and nothing binds it"
    );
}

#[tokio::test]
async fn an_alias_reads_the_same_instance() {
    static BUILT: AtomicUsize = AtomicUsize::new(0);

    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async {
            BUILT.fetch_add(1, Ordering::SeqCst);
            Db("primary")
        })
        .qualified::<Primary>();
        m.alias::<Db, ReadOnly>().of::<Primary>();
    }))
    .await;

    let original = read_q::<Db, Primary>(&app).await.unwrap();
    let alias = read_q::<Db, ReadOnly>(&app).await.unwrap();
    assert!(Arc::ptr_eq(original.arc(), alias.arc()), "both keys reach one object");
    assert_eq!(BUILT.load(Ordering::SeqCst), 1, "the target is built once and the alias builds nothing");
}

#[tokio::test]
async fn contributions_from_two_modules_form_one_many() {
    // Collection order: imports depth first in the order written, then the module's own, each in
    // declaration order. Nothing here is exported: a collection is app-wide.
    let app = connects(Mod::<A>::new(|m| {
        m.contribute::<dyn Plugin>().value(Arc::new(Named("root-1")));
        m.import(Mod::<B>::new(|m| {
            m.contribute::<dyn Plugin>().value(Arc::new(Named("b-1")));
            m.contribute::<dyn Plugin>().value(Arc::new(Named("b-2")));
        }));
        m.import(Mod::<C>::new(|m| {
            m.contribute::<dyn Plugin>().value(Arc::new(Named("c-1")));
        }));
        m.contribute::<dyn Plugin>().value(Arc::new(Named("root-2")));
    }))
    .await;

    let plugins = read_many::<dyn Plugin>(&app).await.unwrap();
    let ids: Vec<&str> = plugins.iter().map(|p| p.id()).collect();
    assert_eq!(ids, ["b-1", "b-2", "c-1", "root-1", "root-2"]);
}

#[tokio::test]
async fn a_contribution_is_a_value_a_type_or_a_factory_output() {
    let app = connects(Mod::<A>::new(|m| {
        m.contribute::<dyn Plugin>().value(Arc::new(Named("value")));
        m.contribute::<dyn Plugin>().provide::<Built>(|a| a);
        m.contribute::<dyn Plugin>().singleton(|cfg: Dep<Cfg>| async move { Built(if cfg.0 == 1 { "factory" } else { "wrong" }) }, |a| a);
        m.singleton(|| async { Cfg(1) });
    }))
    .await;

    let plugins = read_many::<dyn Plugin>(&app).await.unwrap();
    let ids: Vec<&str> = plugins.iter().map(|p| p.id()).collect();
    assert_eq!(ids, ["value", "provided", "factory"]);
    // The collection is not a single binding.
    assert!(matches!(app.get::<dyn Plugin>().await, Err(LookupError::WrongKind { .. })));
    // A `Many` site reads the same list.
    let again: Many<dyn Plugin> = read_many(&app).await.unwrap();
    assert_eq!(again.len(), 3);
}

#[tokio::test]
async fn a_foreign_type_is_bound_by_value_and_by_factory() {
    // Neither type is the crate's: no `Construct` impl, no annotation, no trait of ours.
    let app = connects(Mod::<A>::new(|m| {
        m.value(Mutex::new(vec![1_u8, 2]));
        m.singleton(|| async { HashMap::from([(String::from("k"), 9_u32)]) });
        m.singleton(|pool: Dep<Mutex<Vec<u8>>>, map: Dep<HashMap<String, u32>>| async move {
            pool.lock().unwrap().push(3);
            (pool.lock().unwrap().len(), map["k"])
        });
    }))
    .await;

    assert_eq!(*app.get::<(usize, u32)>().await.unwrap(), (3, 9));
    // The factory's mutation went through the shared `Arc` the value is bound as.
    assert_eq!(*app.get::<Mutex<Vec<u8>>>().await.unwrap().lock().unwrap(), vec![1, 2, 3]);
}
