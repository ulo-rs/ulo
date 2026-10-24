//! Section 11 at wiring (step 2, tests): the refusals a `TestApp` reports before anything is
//! built. A stale override, an override matching several bindings or modules, a change of a
//! key's kind, an override of the timer or the runtime, and the three `replace_module` faults.
//!
//! Capabilities pinned: 48 (the refusal half; the working overrides are in `testing.rs`), 44.

#[macro_use]
mod common;

use std::sync::Arc;

use common::*;
use ulo::testing::TestApp;
use ulo::{BindingKind, Key, Module, ModuleDef, ModuleIdentity, Runtime, Timer, WiringError};

struct Root;
struct Other;
struct Orig;
struct Fake;
struct FakeToo;
struct Primary;
struct Replica;

#[derive(Debug, PartialEq)]
struct Cfg(u8);

trait Plugin: Send + Sync {}
struct P1;
impl Plugin for P1 {}

/// A configured module: equal configurations are one module, different ones two. It binds and
/// exports a `Cfg` carrying its configuration.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Db(u8);

impl Module for Db {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Cfg(self.0));
        m.export::<Cfg>();
    }
}

fn exporting_cfg<K: 'static>(n: u8) -> Mod<K> {
    Mod::new(move |m| {
        m.value(Cfg(n));
        m.export::<Cfg>();
    })
}

/// A module that binds a `Cfg` and exports nothing, so two of them raise no ambiguity of their own.
fn binding_cfg<K: 'static>(n: u8) -> Mod<K> {
    Mod::new(move |m| {
        m.value(Cfg(n));
    })
}

/// `Db` without the export.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Quiet(u8);

impl Module for Quiet {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Cfg(self.0));
    }
}

fn cfg_override() -> Arc<Cfg> {
    Arc::new(Cfg(99))
}

fn errors_of<S>(app: TestApp<S>) -> ulo::WiringErrors {
    watchdog();
    wiring_errors(app.wire())
}

#[test]
fn stale_override() {
    let line = line!() + 2;
    let errors = errors_of(
        TestApp::of(Mod::<Root>::new(|m| m.import(exporting_cfg::<Other>(1)))).override_value::<u32>(Arc::new(7)),
    );
    let (key, keyed, at) = entry!(errors, WiringError::OverrideUnmatched { key, keyed, at } => (key.key(), keyed.clone(), **at));
    assert_eq!(key, Key::of::<u32, ()>());
    assert!(keyed.is_none());
    assert_eq!(at.line(), line, "the entry points at the override call");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_qualified_override_over_a_keyed_module_names_the_module_to_scope_to() {
    let root = || Mod::<Root>::new(|m| m.import(Db(1).keyed::<Primary>()));
    let errors = errors_of(TestApp::of(root()).override_value::<Cfg>(cfg_override()).qualified::<Primary>());
    let keyed = entry!(errors, WiringError::OverrideUnmatched { keyed, .. } => keyed.as_ref().map(|name| name.to_string()));
    assert!(keyed.is_some_and(|name| name.contains("Primary")), "{errors}");

    // The binding inside a keyed module is unqualified, and `in_module_keyed` reaches it as written.
    let app = TestApp::of(root()).override_value::<Cfg>(cfg_override()).in_module_keyed::<Db, Primary>();
    watchdog();
    app.wire().unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn override_matching_two_modules() {
    let root = || {
        Mod::<Root>::new(|m| {
            m.import(binding_cfg::<Orig>(1));
            m.import(binding_cfg::<Other>(2));
        })
    };
    let errors = errors_of(TestApp::of(root()).override_value::<Cfg>(cfg_override()));
    let (key, matches) = entry!(errors, WiringError::OverrideAmbiguous { key, matches, .. } => (key.key(), matches.iter().map(|m| m.to_string()).collect::<Vec<_>>()));
    assert_eq!(key, Key::of::<Cfg, ()>());
    assert_eq!(matches, [mod_name::<Orig>(), mod_name::<Other>()]);
    assert_eq!(errors.len(), 1, "{errors}");

    // The two ways out the error names.
    let all = TestApp::of(root()).override_value::<Cfg>(cfg_override()).everywhere();
    watchdog();
    all.wire().unwrap_or_else(|e| panic!("{e}"));
    let one = TestApp::of(root()).override_value::<Cfg>(cfg_override()).in_module::<Mod<Orig>>();
    watchdog();
    one.wire().unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn in_module_over_two_instances() {
    let root = || {
        Mod::<Root>::new(|m| {
            m.import(Quiet(1));
            m.import(Quiet(2));
        })
    };
    let errors = errors_of(TestApp::of(root()).override_value::<Cfg>(cfg_override()).in_module::<Quiet>());
    let candidates = entry!(errors, WiringError::OverrideModuleAmbiguous { candidates, .. } => candidates.iter().map(|m| m.to_string()).collect::<Vec<_>>());
    assert_eq!(candidates.len(), 2, "{errors}");
    assert_eq!(errors.len(), 1, "{errors}");

    let by_config = TestApp::of(root()).override_value::<Cfg>(cfg_override()).in_module_of(&Quiet(2));
    watchdog();
    by_config.wire().unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn in_module_over_two_keyed_instances_is_ambiguous_and_in_module_keyed_names_one() {
    let root = || {
        Mod::<Root>::new(|m| {
            m.import(Db(1).keyed::<Primary>());
            m.import(Db(1).keyed::<Replica>());
        })
    };
    let errors = errors_of(TestApp::of(root()).override_value::<Cfg>(cfg_override()).in_module::<Db>());
    entry!(errors, WiringError::OverrideModuleAmbiguous { .. } => ());

    let keyed = TestApp::of(root()).override_value::<Cfg>(cfg_override()).in_module_keyed::<Db, Replica>();
    watchdog();
    keyed.wire().unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn override_value_over_a_collection() {
    let errors = errors_of(
        TestApp::of(Mod::<Root>::new(|m| {
            m.contribute::<dyn Plugin>().value(Arc::new(P1));
        }))
        .override_value::<dyn Plugin>(Arc::new(P1)),
    );
    let (key, expected, found) = entry!(errors, WiringError::OverrideKind { key, expected, found, .. } => (key.key(), *expected, *found));
    assert_eq!(key, Key::of::<dyn Plugin, ()>());
    // `expected` is the kind the key is bound as, `found` the kind the override would make it.
    assert_eq!((expected, found), (BindingKind::Collection, BindingKind::Single));
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn overriding_the_timer_is_refused_at_wiring() {
    let line = line!() + 1;
    let errors = errors_of(TestApp::of(Empty).override_value::<dyn Timer>(Arc::new(Frozen::new())));
    let at = entry!(errors, WiringError::TimerOverride { at } => **at);
    assert_eq!(at.line(), line);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[tokio::test]
async fn overriding_the_runtime_is_refused_at_wiring() {
    let errors = errors_of(TestApp::of(Empty).override_value::<dyn Runtime>(Arc::new(ulo_runtime_tokio::Tokio::current())));
    entry!(errors, WiringError::RuntimeOverride { .. } => ());
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn replacement_exports_a_subset() {
    let root = Mod::<Root>::new(|m| m.import(exporting_cfg::<Orig>(1)));
    let errors = errors_of(TestApp::of(root).replace_module(exporting_cfg::<Orig>(1), Mod::<Fake>::bare()));
    let (original, replacement, missing) = entry!(errors, WiringError::ReplacementMissingExports { original, replacement, missing }
        => (original.to_string(), replacement.to_string(), missing.iter().map(|k| k.key()).collect::<Vec<_>>()));
    assert_eq!((original, replacement), (mod_name::<Orig>(), mod_name::<Fake>()));
    assert_eq!(missing, [Key::of::<Cfg, ()>()]);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn replace_a_module_nobody_imports() {
    let line = line!() + 2;
    let errors = errors_of(
        TestApp::of(Mod::<Root>::bare()).replace_module(Mod::<Other>::bare(), Mod::<Fake>::bare()),
    );
    let (original, at) = entry!(errors, WiringError::ReplacementUnmatched { original, at } => (original.to_string(), **at));
    assert_eq!(original, mod_name::<Other>());
    assert_eq!(at.line(), line);
    assert_eq!(errors.len(), 1, "the first call is the sole entry: {errors}");
}

#[test]
fn second_replace_module_of_one_original() {
    let root = || Mod::<Root>::new(|m| m.import(exporting_cfg::<Orig>(1)));
    let errors = errors_of(
        TestApp::of(root())
            .replace_module(exporting_cfg::<Orig>(1), exporting_cfg::<Fake>(2))
            .replace_module(exporting_cfg::<Orig>(1), exporting_cfg::<FakeToo>(3)),
    );
    let (original, first, second) = entry!(errors, WiringError::DuplicateReplacement { original, first, second }
        => (original.to_string(), **first, **second));
    assert_eq!(original, mod_name::<Orig>());
    assert_ne!(first.line(), second.line(), "both calls are named");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_duplicate_over_an_unimported_original_is_reported_once_per_call_never_both() {
    let errors = errors_of(
        TestApp::of(Mod::<Root>::bare())
            .replace_module(Mod::<Other>::bare(), Mod::<Fake>::bare())
            .replace_module(Mod::<Other>::bare(), Mod::<FakeToo>::bare()),
    );
    assert_eq!(entries!(errors, WiringError::ReplacementUnmatched { .. } => ()).len(), 1, "{errors}");
    assert_eq!(entries!(errors, WiringError::DuplicateReplacement { .. } => ()).len(), 1, "{errors}");
    assert_eq!(errors.len(), 2, "{errors}");
}

#[test]
fn an_override_factory_s_parameters_are_checked_by_the_wiring_pass() {
    struct NotBound;
    struct Svc;
    let errors = errors_of(
        TestApp::of(Mod::<Root>::new(|m| {
            m.singleton(|| async { Svc });
        }))
        .override_factory(|_missing: ulo::Dep<NotBound>| async { Svc }),
    );
    let key = entry!(errors, WiringError::Missing { key, .. } => key.key());
    assert_eq!(key, Key::of::<NotBound, ()>(), "the replacement reads what it declares, not what the replaced factory read");
    assert_eq!(errors.len(), 1, "{errors}");
}
