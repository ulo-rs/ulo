//! Section 10.1 step 1, the module graph: import cycles, re-exports, exports of unbound keys,
//! inputs declared by keyed modules and a keyed module keyed again. Each test makes `wire()` fail with one `WiringError` variant
//! and asserts the variant and its fields.
//!
//! Capabilities pinned: 29 (an export is the only exit, `ExportNotBound`), 34 (re-export and its
//! refusals), 44 (all of them reported by `wire()`, before any instance exists), 45 (typed error,
//! no panic), 35 (module names in the entries).

#[macro_use]
mod common;

use common::*;
use ulo::{Key, Module, TypeName, WiringError};

struct A;
struct B;
struct C;
struct Svc;
struct Head;
struct Primary;

trait Repo: Send + Sync {}

#[test]
fn an_import_cycle_is_refused_with_its_path() {
    let root = Mod::<A>::new(|m| m.import(Mod::<B>::new(|m| m.import(Mod::<A>::bare()))));
    let errors = wire_err(root);

    let path = entry!(errors, WiringError::ImportCycle { path } => path.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    assert_eq!(path, [mod_name::<A>(), mod_name::<B>(), mod_name::<A>()]);
    assert_eq!(errors.len(), 1, "a cycle is one entry and nothing else:\n{errors}");
}

#[test]
fn a_three_module_cycle_and_a_module_importing_itself_are_refused() {
    // A -> B -> C -> A
    let root = Mod::<A>::new(|m| m.import(Mod::<B>::new(|m| m.import(Mod::<C>::new(|m| m.import(Mod::<A>::bare()))))));
    let errors = wire_err(root);
    let path = entry!(errors, WiringError::ImportCycle { path } => path.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    assert_eq!(path, [mod_name::<A>(), mod_name::<B>(), mod_name::<C>(), mod_name::<A>()]);

    let errors = wire_err(Mod::<A>::new(|m| m.import(Mod::<A>::bare())));
    let path = entry!(errors, WiringError::ImportCycle { path } => path.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    assert_eq!(path, [mod_name::<A>(), mod_name::<A>()]);
}

#[test]
fn a_diamond_of_imports_is_not_a_cycle() {
    let shared = || Mod::<C>::bare();
    let root = Mod::<A>::new(move |m| {
        m.import(Mod::<B>::new(move |m| m.import(shared())));
        m.import(shared());
    });
    wires(root);
}

#[test]
fn reexport_of_a_key_no_import_exports() {
    let root = Mod::<A>::new(|m| {
        m.import(Mod::<B>::bare());
        m.reexport::<Svc>();
    });
    let errors = wire_err(root);

    let (module, key) =
        entry!(errors, WiringError::ReexportNotVisible { module, key, .. } => (module.to_string(), key.key()));
    assert_eq!(module, mod_name::<A>());
    assert_eq!(key, Key::of::<Svc, ()>());
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn reexport_of_a_key_two_imports_export() {
    // `A` is not the root, so the root's own sweep for ambiguous keys adds no entry of its own.
    let root = Mod::<Primary>::new(|m| {
        m.import(Mod::<A>::new(|m| {
            m.import(Mod::<B>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
            m.reexport::<Svc>();
        }));
    });
    let errors = wire_err(root);

    let (module, key, sources) = entry!(errors, WiringError::ReexportAmbiguous { module, key, sources, .. } => (
        module.to_string(),
        key.key(),
        sources.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    ));
    assert_eq!(module, mod_name::<A>());
    assert_eq!(key, Key::of::<Svc, ()>());
    assert_eq!(sources, [mod_name::<B>(), mod_name::<C>()]);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_reexport_of_a_key_one_import_exports_is_accepted() {
    let root = Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
            m.reexport::<Svc>();
        }));
        m.singleton(|_svc: ulo::Dep<Svc>| async { Head });
    });
    wires(root);
}

#[test]
fn a_keyed_module_may_not_declare_an_input() {
    let errors = wire_err(Mod::<B>::new(|m| {
        m.import(Mod::<A>::new(|m| m.input::<Head>().seeded_by::<TA>()).keyed::<Primary>());
    }));

    let (module, key) = entry!(errors, WiringError::KeyedInput { module, key, .. } => (module.to_string(), key.key()));
    assert_eq!(module, keyed_mod_name::<A, Primary>());
    assert_eq!(key, Key::of::<Head, ()>());
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_unkeyed_module_may_declare_an_input() {
    wires(Mod::<A>::new(|m| m.input::<Head>().seeded_by::<TA>()));
}

#[test]
fn export_of_an_unbound_key() {
    let errors = wire_err(Mod::<A>::new(|m| m.export::<Svc>()));

    let (module, key, imported, near) = entry!(errors, WiringError::ExportNotBound { module, key, imported, near, .. } => (
        module.to_string(),
        key.key(),
        *imported,
        near.is_some(),
    ));
    assert_eq!(module, mod_name::<A>());
    assert_eq!(key, Key::of::<Svc, ()>());
    assert!(!imported, "no import provides the key");
    assert!(!near, "nothing the module binds is spelled like it");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn export_of_a_key_an_import_provides_says_to_reexport_it() {
    let root = Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Svc });
            m.export::<Svc>();
        }));
        m.export::<Svc>();
    });
    let errors = wire_err(root);

    let (imported, near) = entry!(errors, WiringError::ExportNotBound { imported, near, .. } => (*imported, near.is_some()));
    assert!(imported, "an import of the module exports the key");
    assert!(!near);
    // The help line is text the design fixes (section 10.1 step 1).
    let help = errors.to_string();
    assert!(help.contains("re-export it with `reexport`"), "{help}");
}

#[test]
fn export_of_a_key_spelled_like_a_bound_one_names_the_bound_key() {
    let root = Mod::<A>::new(|m| {
        m.provide::<PgRepoSvc>().also_as::<dyn Repo + Send + Sync>(|a| a);
        m.export::<dyn Repo>();
    });
    let errors = wire_err(root);

    let (key, near) =
        entry!(errors, WiringError::ExportNotBound { key, near, .. } => (key.key(), near.as_ref().map(|n| n.key())));
    assert_eq!(key, Key::of::<dyn Repo, ()>());
    assert_eq!(near, Some(Key::of::<dyn Repo + Send + Sync, ()>()), "{errors}");
}

struct PgRepoSvc;

impl Repo for PgRepoSvc {}

impl ulo::Construct for PgRepoSvc {
    type Scope = ulo::scope::Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(PgRepoSvc)
    }
}

#[test]
fn an_export_of_a_key_the_module_binds_is_accepted_and_is_visible_to_an_importer() {
    let root = Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.provide::<PgRepoSvc>().also_as::<dyn Repo>(|a| a);
            m.export::<dyn Repo>();
        }));
        m.singleton(|_repo: ulo::Dep<dyn Repo>| async { Head });
    });
    wires(root);
}

struct Outer;
struct Middle;

#[test]
fn a_keyed_module_keyed_again_is_refused_naming_both_qualifiers() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.value(Svc);
            m.export::<Svc>();
        })
        .keyed::<Primary>()
        .keyed::<Outer>());
    }));
    let (module, outer, inner) =
        entry!(errors, WiringError::NestedKeyed { module, outer, inner } => (module.to_string(), *outer, *inner));
    assert_eq!(outer, TypeName::of::<Outer>(), "the qualifier the identity carries");
    assert_eq!(inner, TypeName::of::<Primary>(), "the qualifier the exports leave under");
    assert_eq!(module, keyed_mod_name::<B, Outer>(), "the module as its identity names it");
    assert_eq!(errors.len(), 1, "{errors}");

    // Three deep: the outermost key is the identity's and the innermost the exports', whatever
    // sits between them.
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.value(Svc);
            m.export::<Svc>();
        })
        .keyed::<Primary>()
        .keyed::<Middle>()
        .keyed::<Outer>());
    }));
    let found = entry!(errors, WiringError::NestedKeyed { module, outer, inner } => (module.to_string(), *outer, *inner));
    assert_eq!(found, (keyed_mod_name::<B, Outer>(), TypeName::of::<Outer>(), TypeName::of::<Primary>()));
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_module_keyed_once_is_accepted() {
    wires(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.value(Svc);
            m.export::<Svc>();
        })
        .keyed::<Outer>());
    }));
}
