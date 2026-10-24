//! `ModuleRef`: the handle to one module, whose lookups are limited to what that module sees.
//!
//! Capabilities pinned (CAPABILITIES.md ids): 37 (a handle to the current or a named module for
//! runtime lookups limited to what the module sees), with the typed errors of capability 47 that
//! such a lookup can answer (`NotFound`, `Ambiguous`, `AmbiguousModule`, `ExecutionRequired`).
//! The full `LookupError` table belongs to `lookup.rs`; `NotReady` during `connect` is B3's.

#[macro_use]
mod common;

use std::sync::Arc;

use common::*;
use ulo::{ExecOptions, LookupError, LookupKind, Meta, Module, ModuleDef, ModuleIdentity, ModuleRef};

struct A;
struct B;
struct C;
struct D;
struct Absent;

struct Svc;
struct Hidden;
struct Other;
struct PerCall;

#[tokio::test]
async fn a_module_ref_sees_only_what_its_module_sees() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
            m.singleton(|| async { Svc });
            m.export::<Svc>();
        }));
        m.import(Mod::<C>::new(|m| {
            m.singleton(|| async { Other });
            m.export::<Other>();
        }));
    }))
    .await;

    let b = app.module::<Mod<B>>().unwrap();
    assert!(b.get::<Hidden>().await.is_ok(), "a module sees its own bindings");
    assert!(b.get::<Svc>().await.is_ok());
    assert!(
        matches!(b.get::<Other>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })),
        "`B` does not import `C`"
    );

    // The root sees `B`'s export and `C`'s, not `B`'s private binding; `C` sees none of `B`.
    assert!(matches!(app.get::<Hidden>().await, Err(LookupError::NotFound { .. })));
    assert!(app.get::<Svc>().await.is_ok());
    let c = app.module::<Mod<C>>().unwrap();
    assert!(matches!(c.get::<Svc>().await, Err(LookupError::NotFound { .. })));
}

#[tokio::test]
async fn a_module_ref_reports_a_key_its_module_sees_twice() {
    // Nothing in `B` reads `Svc`, so wiring accepts the two sources; a runtime lookup meets them.
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
            m.import(Mod::<D>::new(|m| {
                m.singleton(|| async { Svc });
                m.export::<Svc>();
            }));
        }));
    }))
    .await;

    let b = app.module::<Mod<B>>().unwrap();
    let Err(LookupError::Ambiguous { key, sources }) = b.get::<Svc>().await else {
        panic!("`B` sees `Svc` from two imports");
    };
    assert_eq!(key.key(), ulo::Key::of::<Svc, ()>());
    let sources: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
    assert_eq!(sources, [mod_name::<C>(), mod_name::<D>()]);
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Configured(u32);

impl Module for Configured {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

#[tokio::test]
async fn a_module_lookup_by_type_is_not_found_or_ambiguous() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Configured(1));
        m.import(Configured(2));
        m.import(Mod::<B>::bare());
    }))
    .await;

    let Err(LookupError::AmbiguousModule { candidates, .. }) = app.module::<Configured>() else {
        panic!("two configurations of `Configured` are not one module");
    };
    assert_eq!(candidates.len(), 2);
    assert!(matches!(app.module::<Mod<Absent>>(), Err(LookupError::NotFound { kind: LookupKind::Module, .. })));
    assert!(app.module::<Mod<B>>().is_ok());
}

#[tokio::test]
async fn a_module_ref_inside_an_execution_builds_per_execution_bindings_once() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.execution(|| async { PerCall });
        }));
    }))
    .await;

    let b = app.module::<Mod<B>>().unwrap();
    assert!(
        matches!(b.get::<PerCall>().await, Err(LookupError::ExecutionRequired { .. })),
        "outside an execution a per-execution binding has nowhere to live"
    );

    let (first, second, inside_again) = app
        .execute(ExecOptions::new(), async |exec| {
            let here = b.with_execution(&exec.handle());
            let first = here.get::<PerCall>().await.unwrap();
            let second = here.get::<PerCall>().await.unwrap();
            // A second handle for the same execution reads the same cache.
            let again = b.with_execution(&exec.handle()).get::<PerCall>().await.unwrap();
            (first, second, again)
        })
        .await
        .unwrap();
    assert!(Arc::ptr_eq(first.arc(), second.arc()));
    assert!(Arc::ptr_eq(first.arc(), inside_again.arc()));

    // Another execution builds its own.
    let other = app
        .execute(ExecOptions::new(), async |exec| b.with_execution(&exec.handle()).get::<PerCall>().await.unwrap())
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(first.arc(), other.arc()));
}

#[tokio::test]
async fn execute_on_a_module_ref_resolves_with_that_module_s_visibility() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
        }));
    }))
    .await;

    let from_b = app.module::<Mod<B>>().unwrap().execute(ExecOptions::new(), async |exec| exec.get::<Hidden>().await.is_ok()).await.unwrap();
    let from_root = app.execute(ExecOptions::new(), async |exec| exec.get::<Hidden>().await.is_ok()).await.unwrap();
    assert!(from_b);
    assert!(!from_root);
}

#[derive(Default)]
struct Routes {
    names: Vec<&'static str>,
}

impl Meta for Routes {}

#[tokio::test]
async fn meta_is_what_the_module_wrote_and_nothing_for_another_module() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.meta::<Routes>().names.push("health");
            m.meta::<Routes>().names.push("metrics");
        }));
        m.import(Mod::<C>::bare());
    }))
    .await;

    let written = app.module::<Mod<B>>().unwrap().meta::<Routes>().expect("`B` wrote `Routes`");
    assert_eq!(written.names, ["health", "metrics"], "both writes land in the one value");
    assert!(app.module::<Mod<C>>().unwrap().meta::<Routes>().is_none());
}

struct Holder(ModuleRef);

#[tokio::test]
async fn an_injected_module_ref_is_the_enclosing_module() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Hidden });
            m.singleton(|here: ModuleRef| async move { Holder(here) });
            m.export::<Holder>();
        }));
    }))
    .await;

    let holder = app.get::<Holder>().await.unwrap();
    assert_eq!(holder.0.name().to_string(), mod_name::<B>());
    assert!(holder.0.get::<Hidden>().await.is_ok(), "the handle reads what `B` sees, a private binding included");
    assert!(app.get::<Hidden>().await.is_err());
}
