//! Section 10.1 step 3, visibility: every injection point is resolved against its module's table.
//!
//! Capabilities pinned: 29 (an unexported binding is invisible outside its module: `Missing`), 30
//! (a global's exports reach every module), 44 (reported by `wire()`, every one of them), 35
//! (module names in the entries). The `+ Send + Sync` spelling hint and the builder hints are text
//! the design fixes, asserted as text.

#[macro_use]
mod common;

use common::*;
use ulo::{Dep, Key, Runtime, Timer, WiringError};

struct A;
struct B;
struct C;
struct Svc;
struct Head;

trait Repo: Send + Sync {}

struct PgRepo;
impl Repo for PgRepo {}
impl ulo::Construct for PgRepo {
    type Scope = ulo::scope::Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(PgRepo)
    }
}

#[test]
fn missing_dependency() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|_svc: Dep<Svc>| async { Head });
    }));

    let (key, consumer, module, near) =
        entry!(errors, WiringError::Missing { key, consumer, module, near } => (key.key(), consumer.clone(), module.to_string(), near.is_some()));
    assert_eq!(key, Key::of::<Svc, ()>());
    assert!(consumer.contains("Head"), "the consumer is named: {consumer}");
    assert_eq!(module, mod_name::<A>());
    assert!(!near);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_binding_that_is_not_exported_is_missing_outside_its_module() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Svc });
        }));
        m.singleton(|_svc: Dep<Svc>| async { Head });
    }));
    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!((key, module), (Key::of::<Svc, ()>(), mod_name::<A>()));
}

#[test]
fn an_exported_binding_is_visible_to_its_importer_and_not_to_a_module_that_only_shares_the_importer() {
    // `C` is imported by `A` only; `B` imports nothing, so it does not see `C`'s export.
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<C>::new(|m| {
            m.singleton(|| async { Svc });
            m.export::<Svc>();
        }));
        m.singleton(|_svc: Dep<Svc>| async { Head });
        m.import(Mod::<B>::new(|m| {
            m.singleton(|_svc: Dep<Svc>| async { B });
        }));
    }));
    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!((key, module), (Key::of::<Svc, ()>(), mod_name::<B>()));
}

#[test]
fn a_global_modules_exports_reach_a_module_that_does_not_import_it() {
    wires(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.global();
            m.singleton(|| async { Svc });
            m.export::<Svc>();
        }));
        m.import(Mod::<C>::new(|m| {
            m.singleton(|_svc: Dep<Svc>| async { Head });
        }));
    }));
}

#[test]
fn a_global_modules_unexported_bindings_stay_private() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.global();
            m.singleton(|| async { Svc });
        }));
        m.import(Mod::<C>::new(|m| {
            m.singleton(|_svc: Dep<Svc>| async { Head });
        }));
    }));
    let (key, module) = entry!(errors, WiringError::Missing { key, module, .. } => (key.key(), module.to_string()));
    assert_eq!((key, module), (Key::of::<Svc, ()>(), mod_name::<C>()));
}

#[test]
fn missing_dependency_whose_key_differs_only_by_send_and_sync_names_both_spellings() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.provide::<PgRepo>().also_as::<dyn Repo + Send + Sync>(|a| a);
            m.export::<dyn Repo + Send + Sync>();
        }));
        m.singleton(|_repo: Dep<dyn Repo>| async { Head });
    }));

    let (key, near) = entry!(errors, WiringError::Missing { key, near, .. } => (key.key(), near.clone()));
    assert_eq!(key, Key::of::<dyn Repo, ()>());
    let (bound, exporter) = near.expect("the bound spelling is named");
    assert_eq!(bound.key(), Key::of::<dyn Repo + Send + Sync, ()>());
    assert_eq!(exporter.to_string(), mod_name::<B>());
    let report = errors.to_string();
    assert!(report.contains("`dyn Repo + Send + Sync`") && report.contains("`dyn Repo`"), "{report}");
}

#[test]
fn two_imports_export_one_key() {
    let exporter = |m: &mut ulo::ModuleDef<'_>| {
        m.singleton(|| async { Svc });
        m.export::<Svc>();
    };
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(Mod::<B>::new(exporter));
        m.import(Mod::<C>::new(exporter));
        m.singleton(|_svc: Dep<Svc>| async { Head });
    }));

    let (key, module, sources) = entry!(errors, WiringError::Ambiguous { key, module, sources, .. } => (
        key.key(),
        module.to_string(),
        sources.iter().map(|(m, _)| m.to_string()).collect::<Vec<_>>(),
    ));
    assert_eq!(key, Key::of::<Svc, ()>());
    assert_eq!(module, mod_name::<A>());
    assert_eq!(sources, [mod_name::<B>(), mod_name::<C>()]);
    assert_eq!(errors.len(), 1, "reported once, though the root's table also holds the key:\n{errors}");
}

#[test]
fn an_ambiguous_key_in_the_root_is_reported_though_nothing_reads_it() {
    let errors = wire_err(Mod::<A>::new(|m| {
        for _ in 0..2 {
            m.import(Mod::<B>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
        }
        m.import(Mod::<C>::new(|m| {
            m.singleton(|| async { Svc });
            m.export::<Svc>();
        }));
    }));
    let key = entry!(errors, WiringError::Ambiguous { key, .. } => key.key());
    assert_eq!(key, Key::of::<Svc, ()>());
}

#[test]
fn a_service_reading_the_timer_of_an_app_with_none_is_told_how_to_set_one() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|_t: Dep<dyn Timer>| async { Head });
    }));
    let key = entry!(errors, WiringError::Missing { key, .. } => key.key());
    assert_eq!(key, Key::of::<dyn Timer, ()>());
    assert!(errors.to_string().contains("`.timer(..)`"), "{errors}");

    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|_r: Dep<dyn Runtime>| async { Head });
    }));
    let key = entry!(errors, WiringError::Missing { key, .. } => key.key());
    assert_eq!(key, Key::of::<dyn Runtime, ()>());
    assert!(errors.to_string().contains("`.runtime(..)`"), "{errors}");

    wires_with(ulo::App::builder(Mod::<A>::new(|m| {
        m.singleton(|_t: Dep<dyn Timer>| async { Head });
    }))
    .timer(Frozen::new()));
}
