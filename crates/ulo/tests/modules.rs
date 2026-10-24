//! Modules as units of visibility and identity: what leaves a module, global modules,
//! configured modules and the diamond, keyed instances, runtime-built modules and re-exports.
//!
//! Capabilities pinned (CAPABILITIES.md ids): 29 (a module's bindings, and an export as the only
//! way out), 30 (a global module's exports reach every module), 31 (one module type with two
//! configurations is two modules, the same configuration imported twice is one), 32 (keyed
//! instances of one module, each injected by its key), 33 (a module built at runtime by a
//! function), 34 (a module re-exports what one of its imports exports).
//!
//! Capability 29 also names controllers among a module's declarations. A controller is a
//! transport's fixture and belongs to the enhancer batch; this file declares providers, imports
//! and exports. The refusals of imports, exports and re-exports (`ImportCycle`, `ExportNotBound`,
//! `ReexportNotVisible`, `ReexportAmbiguous`) are pinned in `wiring_modules.rs`.

#[macro_use]
mod common;

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use ulo::{Dep, DynamicModule, Key, LookupError, LookupKind, Module, ModuleDef, ModuleIdentity, WiringError};

struct A;
struct B;
struct C;
struct G;
struct Primary;
struct Replica;
struct Absent;

struct Svc(u32);
struct Hidden;
struct Other;
struct Pool(u32);

/// A counter that every copy of a module shares and that takes no part in the module's identity.
#[derive(Clone, Default)]
struct Tally(Arc<AtomicUsize>);

impl Tally {
    fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl PartialEq for Tally {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for Tally {}

impl Hash for Tally {
    fn hash<H: Hasher>(&self, _state: &mut H) {}
}

/// A module configured by value: identity is the type and `url`. It counts how often it is
/// registered and how often its factory runs.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Db {
    url: u32,
    exports: bool,
    registered: Tally,
    built: Tally,
}

impl Db {
    fn new(url: u32, exports: bool, registered: &Tally, built: &Tally) -> Self {
        Db { url, exports, registered: registered.clone(), built: built.clone() }
    }
}

impl Module for Db {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        self.registered.bump();
        let (url, built) = (self.url, self.built.clone());
        m.singleton(move || {
            let built = built.clone();
            async move {
                built.bump();
                Pool(url)
            }
        });
        if self.exports {
            m.export::<Pool>();
        }
    }
}

fn names(candidates: &[ulo::ModuleName]) -> Vec<String> {
    candidates.iter().map(|name| name.to_string()).collect()
}

#[test]
fn an_unexported_binding_is_invisible_outside_its_module() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Svc(1) });
            m.singleton(|| async { Hidden });
            m.export::<Svc>();
        }));
        m.singleton(|svc: Dep<Svc>, _hidden: Dep<Hidden>| async move { svc.0 });
    }));

    let (key, module, near) = entry!(errors, WiringError::Missing { key, module, near, .. } => (
        key.key(),
        module.to_string(),
        near.is_some(),
    ));
    assert_eq!(key, Key::of::<Hidden, ()>());
    assert_eq!(module, mod_name::<A>());
    assert!(!near, "nothing visible is spelled like `Hidden`");
    assert_eq!(errors.len(), 1, "the exported `Svc` resolves, only `Hidden` is refused:\n{errors}");
}

#[tokio::test]
async fn an_export_is_the_only_way_a_binding_leaves_its_module() {
    // Inside `B` the unexported binding is an ordinary dependency. Outside, only the export reads.
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
            m.singleton(|_hidden: Dep<Hidden>| async { Svc(8) });
            m.export::<Svc>();
        }));
    }))
    .await;

    assert_eq!(app.get::<Svc>().await.unwrap().0, 8);
    assert!(matches!(app.get::<Hidden>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })));
}

#[tokio::test]
async fn a_global_module_s_exports_reach_modules_that_do_not_import_it() {
    // `B` imports nothing; the global module is imported by the root alone.
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<G>::new(|m| {
            m.global();
            m.singleton(|| async { Svc(5) });
            m.export::<Svc>();
        }));
        m.import(Mod::<B>::new(|m| {
            m.singleton(|svc: Dep<Svc>| async move { Pool(svc.0 + 1) });
            m.export::<Pool>();
        }));
    }))
    .await;

    assert_eq!(app.get::<Pool>().await.unwrap().0, 6);
}

#[test]
fn a_global_module_s_unexported_bindings_stay_private() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<G>::new(|m| {
            m.global();
            m.singleton(|| async { Svc(5) });
            m.singleton(|| async { Hidden });
            m.export::<Svc>();
        }));
        m.import(Mod::<B>::new(|m| {
            m.singleton(|_svc: Dep<Svc>, _hidden: Dep<Hidden>| async { Pool(0) });
        }));
    }));

    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!(key, Key::of::<Hidden, ()>());
    assert_eq!(module, mod_name::<B>());
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_module_that_is_not_global_reaches_only_its_importers() {
    // The same graph as the global case with `global()` removed.
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<G>::new(|m| {
            m.singleton(|| async { Svc(5) });
            m.export::<Svc>();
        }));
        m.import(Mod::<B>::new(|m| {
            m.singleton(|svc: Dep<Svc>| async move { Pool(svc.0) });
        }));
    }));

    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!(key, Key::of::<Svc, ()>());
    assert_eq!(module, mod_name::<B>());
}

#[tokio::test]
async fn one_module_type_two_configs_is_two_modules() {
    let (registered, built) = (Tally::default(), Tally::default());
    let app = connects(Mod::<A>::new({
        let (registered, built) = (registered.clone(), built.clone());
        move |m| {
            m.import(Db::new(1, false, &registered, &built));
            m.import(Db::new(2, false, &registered, &built));
        }
    }))
    .await;

    assert_eq!((registered.get(), built.get()), (2, 2), "each configuration registers and builds on its own");
    let Err(LookupError::AmbiguousModule { module, candidates }) = app.module::<Db>() else {
        panic!("two configurations of `Db` are not one module");
    };
    assert_eq!(module, std::any::type_name::<Db>());
    assert_eq!(names(&candidates), ["Db", "Db #2"], "the second configuration prints its position");
}

#[tokio::test]
async fn a_diamond_import_with_equal_config_is_one_module() {
    struct ViaB(Dep<Pool>);
    struct ViaC(Dep<Pool>);

    let (registered, built) = (Tally::default(), Tally::default());
    let app = connects(Mod::<A>::new({
        let (registered, built) = (registered.clone(), built.clone());
        move |m| {
            let db = Db::new(1, true, &registered, &built);
            m.import(Mod::<B>::new({
                let db = db.clone();
                move |m| {
                    m.import(db.clone());
                    m.singleton(|pool: Dep<Pool>| async move { ViaB(pool) });
                    m.export::<ViaB>();
                }
            }));
            m.import(Mod::<C>::new(move |m| {
                m.import(db.clone());
                m.singleton(|pool: Dep<Pool>| async move { ViaC(pool) });
                m.export::<ViaC>();
            }));
        }
    }))
    .await;

    assert_eq!((registered.get(), built.get()), (1, 1), "one registration and one pool for the shared configuration");
    let via_b = app.get::<ViaB>().await.unwrap();
    let via_c = app.get::<ViaC>().await.unwrap();
    assert!(Arc::ptr_eq(via_b.0.arc(), via_c.0.arc()));
    assert!(app.module::<Db>().is_ok(), "`Db` names one module");
}

#[tokio::test]
async fn two_keyed_instances_are_injected_by_their_keys() {
    struct Both(u32, u32);

    let (registered, built) = (Tally::default(), Tally::default());
    let app = connects(Mod::<A>::new({
        let (registered, built) = (registered.clone(), built.clone());
        move |m| {
            m.import(Db::new(1, true, &registered, &built).keyed::<Primary>());
            m.import(Db::new(2, true, &registered, &built).keyed::<Replica>());
            m.singleton(|p: Dep<Pool, Primary>, r: Dep<Pool, Replica>| async move { Both(p.0, r.0) });
        }
    }))
    .await;

    let both = app.get::<Both>().await.unwrap();
    assert_eq!((both.0, both.1), (1, 2));
    assert_eq!((registered.get(), built.get()), (2, 2));

    // Each instance is reached by its key, and inside it the key is unqualified.
    let replica = app.module_keyed::<Db, Replica>().unwrap();
    assert_eq!(replica.name().to_string(), "Db @ Replica");
    assert_eq!(replica.get::<Pool>().await.unwrap().0, 2);
    assert_eq!(app.module_keyed::<Db, Primary>().unwrap().get::<Pool>().await.unwrap().0, 1);
    assert!(matches!(app.module::<Db>(), Err(LookupError::AmbiguousModule { .. })), "no key names two instances");
    assert!(matches!(
        app.module_keyed::<Db, Absent>(),
        Err(LookupError::NotFound { kind: LookupKind::Module, .. })
    ));
}

#[test]
fn a_keyed_module_s_export_is_read_only_under_its_key() {
    let (registered, built) = (Tally::default(), Tally::default());
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(Db::new(1, true, &registered, &built).keyed::<Primary>());
        m.singleton(|pool: Dep<Pool>| async move { Svc(pool.0) });
    }));

    let key = entry!(errors, WiringError::Missing { key, .. } => key.key());
    assert_eq!(key, Key::of::<Pool, ()>(), "the export left as `Pool @ Primary`, so the bare key is unbound");
}

struct Owner;

fn dynamic(config: u32, registered: &Tally) -> DynamicModule {
    let registered = registered.clone();
    DynamicModule::new::<Owner, u32>(config, move |m| {
        registered.bump();
        m.singleton(move || async move { Pool(config) });
        m.export::<Pool>();
    })
}

#[tokio::test]
async fn a_dynamic_module_registers_what_its_function_registers() {
    let registered = Tally::default();
    let app = connects(Mod::<A>::new({
        let registered = registered.clone();
        move |m| {
            // Imported along two paths: the function runs for the one module.
            m.import(Mod::<B>::new({
                let registered = registered.clone();
                move |m| m.import(dynamic(5, &registered))
            }));
            m.import(dynamic(5, &registered));
            m.singleton(|pool: Dep<Pool>| async move { Svc(pool.0) });
        }
    }))
    .await;

    assert_eq!(registered.get(), 1, "an equal identity registers once");
    assert_eq!(app.get::<Svc>().await.unwrap().0, 5, "the function's binding and export are the module's");
    assert_eq!(app.module::<Owner>().unwrap().name().to_string(), "Owner");
}

#[tokio::test]
async fn a_dynamic_module_is_identified_by_owner_and_config_and_prints_its_label() {
    let registered = Tally::default();
    let app = connects(Mod::<A>::new({
        let registered = registered.clone();
        move |m| {
            m.import(dynamic(1, &registered).label("cache"));
            m.import(Mod::<B>::new({
                let registered = registered.clone();
                move |m| m.import(dynamic(2, &registered).label("cache"))
            }));
        }
    }))
    .await;

    assert_eq!(registered.get(), 2, "two configurations of one owner are two modules");
    let Err(LookupError::AmbiguousModule { candidates, .. }) = app.module::<Owner>() else {
        panic!("two configurations of `Owner` are not one module");
    };
    assert_eq!(names(&candidates), ["cache", "cache #2"], "the label replaces the type name, the position stays");
}

#[tokio::test]
async fn reexport_passes_an_import_s_export_on() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { Svc(3) });
                m.singleton(|| async { Other });
                m.export::<Svc>();
                m.export::<Other>();
            }));
            m.reexport::<Svc>();
        }));
        m.singleton(|svc: Dep<Svc>| async move { Pool(svc.0) });
    }))
    .await;

    assert_eq!(app.get::<Pool>().await.unwrap().0, 3);
    let origin = app.module::<Mod<C>>().unwrap().get::<Svc>().await.unwrap();
    let reached = app.get::<Svc>().await.unwrap();
    assert!(Arc::ptr_eq(origin.arc(), reached.arc()), "the re-export is the same instance");
    assert!(
        matches!(app.get::<Other>().await, Err(LookupError::NotFound { .. })),
        "`B` re-exported `Svc` alone; `Other` stops at `B`"
    );
}

#[test]
fn a_reexport_names_one_key_and_not_the_import_s_whole_surface() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { Svc(3) });
                m.singleton(|| async { Other });
                m.export::<Svc>();
                m.export::<Other>();
            }));
            m.reexport::<Svc>();
        }));
        m.singleton(|_svc: Dep<Svc>, _other: Dep<Other>| async { Pool(0) });
    }));

    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!(key, Key::of::<Other, ()>());
    assert_eq!(module, mod_name::<A>());
}
