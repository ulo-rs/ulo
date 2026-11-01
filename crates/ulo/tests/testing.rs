//! `TestApp` (section 11): building a module graph with bindings overridden or modules replaced,
//! without editing the production modules. The refusals are in `wiring_overrides.rs`.
//!
//! Capabilities pinned: 48 (an override replaces the recipe and keeps the origin module and its
//! visibility; factories and try-factories; a whole collection; qualified keys; scoping to a module
//! by type, key or configuration, or everywhere; `replace_module` with a superset of exports).

#[macro_use]
mod common;

use std::sync::Arc;

use common::*;
use ulo::testing::TestApp;
use ulo::{ConnectError, Dep, ExecOptions, FailureReason, LookupError, Module, ModuleDef, ModuleIdentity, StartupError};

struct Root;
struct Db;
struct Inner;
struct Fake;
struct Primary;
struct Replica;

#[derive(Debug, PartialEq)]
struct Cfg(u8);

struct Svc(u8);

trait Repo: Send + Sync {
    fn name(&self) -> &'static str;
}
struct Pg;
struct Mem;
impl Repo for Pg {
    fn name(&self) -> &'static str {
        "pg"
    }
}
impl Repo for Mem {
    fn name(&self) -> &'static str {
        "mem"
    }
}

trait Plugin: Send + Sync {
    fn id(&self) -> u8;
}
struct Plug(u8);
impl Plugin for Plug {
    fn id(&self) -> u8 {
        self.0
    }
}

/// Production: a database module exporting its `Cfg`, built by a counted factory.
fn db(built: &Count) -> Mod<Db> {
    let built = built.clone();
    Mod::new(move |m| {
        let built = built.clone();
        m.singleton(move || {
            built.bump();
            async { Cfg(1) }
        });
        m.export::<Cfg>();
    })
}

#[tokio::test]
async fn override_value_replaces_the_recipe_and_the_production_factory_never_runs() {
    let built = Count::new();
    let d = built.clone();
    let app = TestApp::of(Mod::<Root>::new(move |m| m.import(db(&d))))
        .override_value::<Cfg>(Arc::new(Cfg(2)))
        .connect()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(*app.get::<Cfg>().await.unwrap(), Cfg(2));
    assert_eq!(built.get(), 0, "the replaced recipe is not run");
    close(app).await;
}

#[tokio::test]
async fn an_override_keeps_the_origin_modules_visibility() {
    struct Hidden(u8);
    let app = TestApp::of(Mod::<Root>::new(|m| {
        m.import(Mod::<Inner>::new(|m| {
            m.value(Hidden(1));
            m.value(Cfg(1));
            m.export::<Cfg>();
        }));
    }))
    .override_value::<Hidden>(Arc::new(Hidden(9)))
    .override_value::<Cfg>(Arc::new(Cfg(9)))
    .connect()
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(matches!(app.get::<Hidden>().await, Err(LookupError::NotFound { .. })), "unexported stays unexported");
    assert_eq!(app.module::<Mod<Inner>>().unwrap().get::<Hidden>().await.unwrap().0, 9, "and is replaced where it is bound");
    assert_eq!(app.get::<Cfg>().await.unwrap().0, 9, "an export stays an export");
    close(app).await;
}

#[tokio::test]
async fn an_implementation_under_a_trait_is_overridden_by_the_trait_key() {
    let app = TestApp::of(Mod::<Root>::new(|m| {
        m.value(Pg).also_as::<dyn Repo>(|a| a);
    }))
    .override_value::<dyn Repo>(Arc::new(Mem))
    .connect()
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(app.get::<dyn Repo>().await.unwrap().name(), "mem");
    close(app).await;
}

#[tokio::test]
async fn override_factory_reads_its_own_parameters() {
    let app = TestApp::of(Mod::<Root>::new(|m| {
        m.value(Cfg(5));
        m.singleton(|| async { Svc(0) });
    }))
    .override_factory(|cfg: Dep<Cfg>| async move { Svc(cfg.0 + 1) })
    .connect()
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(app.get::<Svc>().await.unwrap().0, 6);
    close(app).await;
}

#[tokio::test]
async fn an_override_drops_the_replaced_recipes_dependencies() {
    struct NotBound;
    let app = TestApp::of(Mod::<Root>::new(|m| {
        m.singleton(|_missing: Dep<NotBound>| async { Svc(0) });
    }))
    .override_value::<Svc>(Arc::new(Svc(4)))
    .connect()
    .await
    .unwrap_or_else(|e| panic!("the replaced factory's unmet parameter must not be reported:\n{e}"));
    assert_eq!(app.get::<Svc>().await.unwrap().0, 4);
    close(app).await;
}

#[tokio::test]
async fn override_try_factory_builds_on_ok_and_fails_connect_on_err() {
    let production = || {
        Mod::<Root>::new(|m| {
            m.singleton(|| async { Svc(0) });
        })
    };
    let app = TestApp::of(production())
        .override_try_factory(|| async { Ok::<_, Boom>(Svc(3)) })
        .connect()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(app.get::<Svc>().await.unwrap().0, 3);
    close(app).await;

    let failed = TestApp::of(production()).override_try_factory(|| async { Err::<Svc, _>(Boom::new("no")) }).connect().await;
    match failed {
        Err(StartupError::Connect(ConnectError::Construct { reason: FailureReason::Errored(_), .. })) => {}
        other => panic!("expected Connect(Construct(Errored)), found {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn override_many_replaces_a_whole_collection_and_qualified_targets_the_qualified_one() {
    let production = || {
        Mod::<Root>::new(|m| {
            m.contribute::<dyn Plugin>().value(Arc::new(Plug(1)));
            m.contribute::<dyn Plugin>().value(Arc::new(Plug(2)));
            m.contribute::<dyn Plugin>().qualified::<Replica>().value(Arc::new(Plug(3)));
            m.singleton(|plugins: ulo::Many<dyn Plugin>| async move { Svc(plugins.iter().map(|p| p.id()).sum()) });
            m.singleton(|plugins: ulo::Many<dyn Plugin, Replica>| async move { Plug(plugins.iter().map(|p| p.id()).sum()) });
        })
    };
    let app = TestApp::of(production()).override_many::<dyn Plugin>([Arc::new(Plug(10)) as Arc<dyn Plugin>]).connect().await.unwrap();
    assert_eq!(app.get::<Svc>().await.unwrap().0, 10, "both production contributions are gone");
    assert_eq!(app.get::<Plug>().await.unwrap().0, 3, "the qualified collection is another key");
    close(app).await;

    let app = TestApp::of(production())
        .override_many::<dyn Plugin>([Arc::new(Plug(20)) as Arc<dyn Plugin>, Arc::new(Plug(21)) as Arc<dyn Plugin>])
        .qualified::<Replica>()
        .connect()
        .await
        .unwrap();
    assert_eq!(app.get::<Svc>().await.unwrap().0, 3, "the unqualified collection is untouched");
    assert_eq!(app.get::<Plug>().await.unwrap().0, 41);
    close(app).await;
}

#[tokio::test]
async fn qualified_targets_the_qualified_binding_only() {
    let app = TestApp::of(Mod::<Root>::new(|m| {
        m.value(Cfg(1));
        m.value(Cfg(2)).qualified::<Replica>();
    }))
    .override_value::<Cfg>(Arc::new(Cfg(9)))
    .qualified::<Replica>()
    .connect()
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(app.get::<Cfg>().await.unwrap().0, 1);
    let replica = app.execute(ExecOptions::new(), async |exec| exec.resolver().dep_qualified::<Cfg, Replica>().await.map(|d| d.0)).await;
    assert_eq!(replica.unwrap().unwrap(), 9);
    close(app).await;
}

/// Two non-exporting modules that each bind a `Cfg`, so a module handle is how each is read.
fn two_dbs() -> Mod<Root> {
    Mod::new(|m| {
        m.import(Mod::<Db>::new(|m| {
            m.value(Cfg(1));
        }));
        m.import(Mod::<Inner>::new(|m| {
            m.value(Cfg(2));
        }));
    })
}

async fn cfgs(app: &ulo::App<ulo::Connected>) -> (u8, u8) {
    (
        app.module::<Mod<Db>>().unwrap().get::<Cfg>().await.unwrap().0,
        app.module::<Mod<Inner>>().unwrap().get::<Cfg>().await.unwrap().0,
    )
}

#[tokio::test]
async fn in_module_replaces_only_that_modules_binding_and_everywhere_replaces_both() {
    let app = TestApp::of(two_dbs()).override_value::<Cfg>(Arc::new(Cfg(9))).in_module::<Mod<Inner>>().connect().await.unwrap();
    assert_eq!(cfgs(&app).await, (1, 9));
    close(app).await;
    let app = TestApp::of(two_dbs()).override_value::<Cfg>(Arc::new(Cfg(9))).everywhere().connect().await.unwrap();
    assert_eq!(cfgs(&app).await, (9, 9));
    close(app).await;
}

/// A configured module binding a `Cfg` of its configuration, exporting nothing.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Conf(u8);

impl Module for Conf {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Cfg(self.0));
    }
}

#[tokio::test]
async fn in_module_of_and_in_module_keyed_name_one_instance() {
    watchdog();
    let root = || {
        Mod::<Root>::new(|m| {
            m.import(Conf(1));
            m.import(Conf(2));
            m.import(Conf(3).keyed::<Primary>());
            m.import(Conf(3).keyed::<Replica>());
        })
    };
    let app = TestApp::of(root()).override_value::<Cfg>(Arc::new(Cfg(9))).in_module_of(&Conf(2)).connect().await.unwrap();
    assert!(matches!(app.module::<Conf>(), Err(LookupError::AmbiguousModule { .. })));
    let mut by_config = Vec::new();
    for n in [1, 2] {
        let module = app.module_of(&Conf(n)).unwrap_or_else(|e| panic!("{e}"));
        by_config.push(module.get::<Cfg>().await.unwrap().0);
    }
    assert_eq!(by_config, [1, 9], "the configuration named is the instance replaced");
    let primary = app.module_keyed::<Conf, Primary>().unwrap().get::<Cfg>().await.unwrap().0;
    assert_eq!(primary, 3, "the other instances are as production wrote them");
    close(app).await;

    let app = TestApp::of(root()).override_value::<Cfg>(Arc::new(Cfg(9))).in_module_keyed::<Conf, Replica>().connect().await.unwrap();
    assert_eq!(app.module_keyed::<Conf, Replica>().unwrap().get::<Cfg>().await.unwrap().0, 9);
    assert_eq!(app.module_keyed::<Conf, Primary>().unwrap().get::<Cfg>().await.unwrap().0, 3);
    close(app).await;
}

#[tokio::test]
async fn replace_module_swaps_an_import_for_a_superset_of_its_exports() {
    let built = Count::new();
    let d = built.clone();
    let fake = Mod::<Fake>::new(|m| {
        m.value(Cfg(7));
        m.value(Svc(8));
        m.export::<Cfg>();
        m.export::<Svc>();
    });
    let app = TestApp::of(Mod::<Root>::new(move |m| m.import(db(&d))))
        .replace_module(db(&Count::new()), fake)
        .connect()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(app.get::<Cfg>().await.unwrap().0, 7);
    assert_eq!(app.get::<Svc>().await.unwrap().0, 8, "the replacement may export more");
    assert_eq!(built.get(), 0, "the original module's factory is never run");
    close(app).await;
}
