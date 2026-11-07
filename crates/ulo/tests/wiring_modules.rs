//! Section 10.1 step 1, the module graph: self-imports, re-exports, exports of unbound keys,
//! inputs declared by keyed modules, a keyed module keyed again and two registrations of one
//! identity with different settings. Each test makes `wire()` fail with one `WiringError` variant
//! and asserts the variant and its fields.
//!
//! Capabilities pinned: 29 (an export is the only exit, `ExportNotBound`), 34 (re-export and its
//! refusals), 44 (all of them reported by `wire()`, before any instance exists), 45 (typed error,
//! no panic), 35 (module names in the entries).

#[macro_use]
mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use common::*;
use ulo::{DifferingSetting, DynamicModule, Key, Module, ModuleDef, ModuleIdentity, ModuleSettings, ModuleSite, TypeName, WiringError};

struct A;
struct B;
struct C;
struct Svc;
struct Head;
struct Primary;

trait Repo: Send + Sync {}

/// Planted break: `enter` linking a self-import like an import closing a longer cycle.
#[test]
fn a_module_importing_itself_is_refused_naming_it_twice() {
    let errors = wire_err(Mod::<A>::new(|m| m.import(Mod::<A>::bare())));
    let path = entry!(errors, WiringError::ImportCycle { path } => path.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    assert_eq!(path, [mod_name::<A>(), mod_name::<A>()]);
    assert_eq!(errors.len(), 1, "the self-import and nothing else:\n{errors}");
    assert!(errors.to_string().contains(&format!("{} imports itself", mod_name::<A>())), "{errors}");
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

/// A configured module: `options` is its identity, `knob` and `mode` settings outside it.
struct Pool {
    options: &'static str,
    knob: u32,
    mode: &'static str,
}

impl Pool {
    fn new(options: &'static str, knob: u32) -> Self {
        Pool { options, knob, mode: "plain" }
    }
}

impl Module for Pool {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_owner::<Pool, _>(&self.options)
    }

    fn settings(&self) -> ModuleSettings {
        ModuleSettings::new().with("knob", self.knob).with("mode", self.mode)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Svc);
        m.export::<Svc>();
    }
}

/// Each setting as `(name, first value, second value)`.
fn sides(settings: &[DifferingSetting]) -> Vec<(&'static str, Option<String>, Option<String>)> {
    settings.iter().map(|setting| (setting.name, setting.first.clone(), setting.second.clone())).collect()
}

static FIRST: AtomicU32 = AtomicU32::new(0);
static AGAIN: AtomicU32 = AtomicU32::new(0);

/// Planted break: `enter` returning the claimed node without comparing settings.
#[test]
fn a_second_import_of_one_identity_with_other_settings_is_refused_naming_both_sites() {
    #[rustfmt::skip]
    let root = Mod::<A>::new(|m| {
        m.import(Pool::new("db", 5)); FIRST.store(line!(), Ordering::SeqCst);
        m.import(Mod::<B>::new(|m| {
            m.import(Pool::new("db", 10)); AGAIN.store(line!(), Ordering::SeqCst);
        }));
    });
    let errors = wire_err(root);
    let (module, first, second, settings) = entry!(errors, WiringError::DifferingSettings { module, first, second, settings } =>
        (module.to_string(), first.clone(), second.clone(), settings.clone()));
    assert_eq!(module, "Pool");
    let site = |site: &ModuleSite| match site {
        ModuleSite::Import { importer, at } => (importer.to_string(), at.file().to_owned(), at.line()),
        other => panic!("expected an import site, got {other:?}"),
    };
    assert_eq!(site(&first), (mod_name::<A>(), file!().to_owned(), FIRST.load(Ordering::SeqCst)), "the import that claimed the identity");
    assert_eq!(site(&second), (mod_name::<B>(), file!().to_owned(), AGAIN.load(Ordering::SeqCst)), "the later import");
    assert_eq!(sides(&settings), [("knob", Some("5".to_owned()), Some("10".to_owned()))], "only the setting that differs");
    assert_eq!(errors.len(), 1, "{errors}");

    let expected = format!(
        "two registrations of Pool differ in `knob`\n\
         ├─ imported by {a} at {file}:{first} with `knob` = 5\n\
         ├─ imported again by {b} at {file}:{again} with `knob` = 10\n\
         └─ help: an equal identity is one module, registered once; write the same settings at both, or key one with `.keyed::<Q>()` to make it a second instance",
        a = mod_name::<A>(),
        b = mod_name::<B>(),
        file = file!(),
        first = FIRST.load(Ordering::SeqCst),
        again = AGAIN.load(Ordering::SeqCst),
    );
    assert_eq!(errors.iter().next().map(ToString::to_string), Some(expected));
}

/// Every differing setting is named, in the order the first registration wrote them, and a
/// setting equal on both sides is left out.
#[test]
fn every_setting_that_differs_is_named() {
    let root = Mod::<A>::new(|m| {
        m.import(Pool { options: "db", knob: 1, mode: "plain" });
        m.import(Pool { options: "db", knob: 2, mode: "tls" });
    });
    let errors = wire_err(root);
    let names = entry!(errors, WiringError::DifferingSettings { settings, .. } =>
        settings.iter().map(|setting| setting.name).collect::<Vec<_>>());
    assert_eq!(names, ["knob", "mode"]);
    let text = errors.to_string();
    assert!(text.contains("differ in `knob`, `mode`"), "{text}");
    assert!(text.contains("with `knob` = 1, `mode` = \"plain\""), "{text}");

    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Pool { options: "db", knob: 1, mode: "plain" });
        m.import(Pool { options: "db", knob: 1, mode: "tls" });
    }));
    let names = entry!(errors, WiringError::DifferingSettings { settings, .. } =>
        settings.iter().map(|setting| setting.name).collect::<Vec<_>>());
    assert_eq!(names, ["mode"]);
}

/// Planted break: `Keyed::settings` left at the trait's default.
#[test]
fn a_keyed_module_compares_the_settings_of_the_module_it_keys() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Pool::new("db", 1).keyed::<Primary>());
        m.import(Pool::new("db", 2).keyed::<Primary>());
    }));
    let module = entry!(errors, WiringError::DifferingSettings { module, .. } => module.to_string());
    assert_eq!(module, format!("Pool @ {}", TypeName::of::<Primary>()));
}

/// Planted break: `DynamicModule::settings` answering none.
#[test]
fn a_dynamic_module_compares_the_settings_it_was_given() {
    struct Owner;
    let dynamic = |knob: u32| {
        DynamicModule::new::<Owner, _>("cfg", |m| {
            m.value(Svc);
            m.export::<Svc>();
        })
        .with_settings(ModuleSettings::new().with("knob", knob))
    };
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(dynamic(1));
        m.import(dynamic(2));
    }));
    let settings = entry!(errors, WiringError::DifferingSettings { settings, .. } => settings.clone());
    assert_eq!(sides(&settings), [("knob", Some("1".to_owned()), Some("2".to_owned()))]);
}

/// A module writing one setting name twice is refused at `wire()`, imported once, naming the
/// name and both `with` calls. Planted break: `freeze` reporting no repeat.
#[test]
fn a_setting_name_a_module_writes_twice_is_refused_with_both_lines() {
    struct Owner;
    let first_line = line!() + 2;
    let settings = ModuleSettings::new()
        .with("knob", 1)
        .with("mode", "fast")
        .with("knob", 2);
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(DynamicModule::new::<Owner, _>("cfg", |_| {}).with_settings(settings.clone()));
    }));

    let (setting, module, first, second) = entry!(errors, WiringError::DuplicateSetting { setting, module, first, second } =>
        (*setting, module.to_string(), (first.file(), first.line()), (second.file(), second.line())));
    assert_eq!(setting, "knob");
    assert_eq!(module, "Owner");
    assert_eq!(first, (file!(), first_line));
    assert_eq!(second, (file!(), first_line + 2));
    assert_eq!(errors.len(), 1, "one entry:\n{errors}");
    let text = errors.to_string();
    assert!(text.contains(&format!("first written at {}:{}", file!(), first_line)), "the first line:\n{text}");
    assert!(text.contains(&format!("written again at {}:{}", file!(), first_line + 2)), "the repeat's line:\n{text}");
}

/// Every repeat is an entry of its own, in the order written, one with the value already
/// recorded among them, each naming the line that recorded the name. Planted breaks: `freeze`
/// reporting a module's first repeat alone; `with` skipping a repeat of an equal value.
#[test]
fn every_repeat_is_refused_in_the_order_written_an_equal_value_among_them() {
    struct Owner;
    let line = line!() + 2;
    let settings = ModuleSettings::new()
        .with("knob", 1)
        .with("mode", "fast")
        .with("knob", 1)
        .with("mode", "slow")
        .with("knob", 3);
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(DynamicModule::new::<Owner, _>("cfg", |_| {}).with_settings(settings.clone()));
    }));
    let repeats: Vec<_> = errors
        .iter()
        .filter_map(|error| match error {
            WiringError::DuplicateSetting { setting, first, second, .. } => Some((*setting, first.line(), second.line())),
            _ => None,
        })
        .collect();
    assert_eq!(repeats, [("knob", line, line + 2), ("mode", line + 1, line + 3), ("knob", line, line + 4)], "{errors}");
    assert_eq!(errors.len(), 3, "{errors}");
}

/// A repeat is refused whichever registration of an identity writes it: a later import whose
/// settings equal the first's is dropped once compared, its repeat reported all the same, and a
/// repeat both imports write is one entry. Planted break: `later_repeats` recording nothing.
#[test]
fn a_repeat_a_later_equal_import_writes_is_refused_and_a_shared_one_is_one_entry() {
    struct Owner;
    let dynamic = |settings: ModuleSettings| DynamicModule::new::<Owner, _>("cfg", |_| {}).with_settings(settings);
    let once = || ModuleSettings::new().with("knob", 1);
    let twice = || ModuleSettings::new().with("knob", 1).with("knob", 2);
    for repeat_first in [true, false] {
        let errors = wire_err(Mod::<A>::new(move |m| {
            let (first, second) = if repeat_first { (twice(), once()) } else { (once(), twice()) };
            m.import(dynamic(first));
            m.import(dynamic(second));
        }));
        let setting = entry!(errors, WiringError::DuplicateSetting { setting, .. } => *setting);
        assert_eq!(setting, "knob");
        assert_eq!(errors.len(), 1, "the repeat in the import written first: {repeat_first}\n{errors}");
    }
    let errors = wire_err(Mod::<A>::new(move |m| {
        m.import(dynamic(twice()));
        m.import(dynamic(twice()));
    }));
    assert_eq!(errors.len(), 1, "one repeat written by both imports:\n{errors}");
}

#[tokio::test]
async fn two_imports_with_equal_settings_are_one_module() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Pool::new("db", 5));
        m.import(Mod::<B>::new(|m| m.import(Pool::new("db", 5))));
    }))
    .await;
    app.module::<Pool>().unwrap_or_else(|error| panic!("one module under the identity: {error}"));
}

#[tokio::test]
async fn other_options_are_another_identity_whatever_the_settings() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Pool::new("primary", 5).keyed::<Primary>());
        m.import(Pool::new("replica", 10).keyed::<Outer>());
        // Each in a module of its own, so no module sees two `Svc`s.
        m.import(Mod::<B>::new(|m| m.import(Pool::new("a", 5))));
        m.import(Mod::<C>::new(|m| m.import(Pool::new("b", 10))));
    }))
    .await;
    let lookup = app.module::<Pool>().map(drop);
    assert!(matches!(lookup, Err(ulo::LookupError::AmbiguousModule { .. })), "two unkeyed modules: {lookup:?}");
}

#[tokio::test]
async fn two_keyed_instances_of_equal_options_with_other_settings_are_two_modules() {
    let app = connects(Mod::<A>::new(|m| {
        m.import(Pool::new("db", 5).keyed::<Primary>());
        m.import(Pool::new("db", 10).keyed::<Outer>());
    }))
    .await;
    app.module_keyed::<Pool, Primary>().unwrap_or_else(|error| panic!("the first instance: {error}"));
    app.module_keyed::<Pool, Outer>().unwrap_or_else(|error| panic!("the second instance: {error}"));
}

/// A setting one registration writes and the other does not is named, the side without it
/// printed `unset`. Planted break: `differences` leaving out the names only the second has.
#[test]
fn a_setting_only_the_second_registration_writes_is_named_unset_on_the_first() {
    struct Optional(Option<u32>);
    impl Module for Optional {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_owner::<Optional, _>(&"db")
        }

        fn settings(&self) -> ModuleSettings {
            match self.0 {
                Some(extra) => ModuleSettings::new().with("extra", extra),
                None => ModuleSettings::new(),
            }
        }

        fn register(&self, _: &mut ModuleDef<'_>) {}
    }
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Optional(None));
        m.import(Optional(Some(3)));
    }));
    let settings = entry!(errors, WiringError::DifferingSettings { settings, .. } => settings.clone());
    assert_eq!(sides(&settings), [("extra", None, Some("3".to_owned()))]);
    let text = errors.to_string();
    assert!(text.contains("with `extra` unset"), "{text}");
    assert!(text.contains("with `extra` = 3"), "{text}");
}

/// A module of identity `"db"` that imports itself with `next` as its setting, directly or, with
/// `through`, through `Mod<B>`.
struct SelfImporting {
    knob: u32,
    next: Option<u32>,
    through: bool,
}

impl Module for SelfImporting {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_owner::<SelfImporting, _>(&"db")
    }

    fn settings(&self) -> ModuleSettings {
        ModuleSettings::new().with("knob", self.knob)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let Some(knob) = self.next else { return };
        let again = move || SelfImporting { knob, next: None, through: false };
        if self.through {
            m.import(Mod::<B>::new(move |m| m.import(again())));
        } else {
            m.import(again());
        }
    }
}

/// A self-import that differs in its settings is one mistake, reported once as the self-import.
/// Planted break: `enter` comparing settings before returning on a self-import.
#[test]
fn a_self_import_with_other_settings_is_reported_as_the_cycle_alone() {
    let errors = wire_err(SelfImporting { knob: 1, next: Some(2), through: false });
    let path = entry!(errors, WiringError::ImportCycle { path } => path.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    assert_eq!(path, ["SelfImporting", "SelfImporting"]);
    assert_eq!(errors.len(), 1, "the cycle and nothing else:\n{errors}");
}

/// An import closing a cycle through another module is a later registration of an identity like
/// any other, so other settings are `DifferingSettings` and the cycle is not an entry. Planted
/// break: `enter` returning before the comparison for every import closing a cycle.
#[test]
fn an_import_closing_a_cycle_with_other_settings_is_refused_as_differing_settings() {
    let errors = wire_err(SelfImporting { knob: 1, next: Some(2), through: true });
    let (module, first, second, settings) = entry!(errors, WiringError::DifferingSettings { module, first, second, settings } =>
        (module.to_string(), first.clone(), second.clone(), sides(settings)));
    assert_eq!(module, "SelfImporting");
    assert!(matches!(first, ModuleSite::Root), "the first registration is the root: {first:?}");
    assert!(
        matches!(&second, ModuleSite::Import { importer, .. } if importer.to_string() == mod_name::<B>()),
        "the second is the import written in {}: {second:?}",
        mod_name::<B>()
    );
    assert_eq!(settings, [("knob", Some("1".to_owned()), Some("2".to_owned()))]);
    assert_eq!(errors.len(), 1, "the differing settings and nothing else:\n{errors}");
}

/// A setting value holding a secret the graph registered is printed redacted, and kept whole in
/// the entry's field. Planted breaks: the report printing `first`/`second` instead of the
/// redacted copies; `DifferingSetting`'s `Debug` derived, which prints the fields.
#[test]
fn a_registered_secret_in_a_setting_value_is_redacted_in_the_report() {
    const SECRET: &str = "s3cret-token-value";
    struct Holding(&'static str);
    impl Module for Holding {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_owner::<Holding, _>(&"db")
        }

        fn settings(&self) -> ModuleSettings {
            ModuleSettings::new().with("auth", format_args!("{}", self.0))
        }

        fn register(&self, _: &mut ModuleDef<'_>) {}
    }
    let errors = wire_err(Mod::<A>::new(|m| {
        m.secret(&ulo::Secret::new(SECRET.to_owned()));
        m.import(Holding("bearer s3cret-token-value"));
        m.import(Holding("none"));
    }));
    let settings = entry!(errors, WiringError::DifferingSettings { settings, .. } => settings.clone());
    assert_eq!(
        sides(&settings),
        [("auth", Some(format!("bearer {SECRET}")), Some("none".to_owned()))],
        "the field keeps the value as written"
    );
    for text in [errors.to_string(), format!("{errors:?}"), errors.iter().next().map(ToString::to_string).unwrap_or_default()] {
        assert!(!text.contains(SECRET), "the secret is printed:\n{text}");
        assert!(text.contains("with `auth` = bearer [redacted]"), "{text}");
    }
    let entry = format!("{:?}", errors.iter().next());
    assert!(!entry.contains(SECRET), "the entry's `Debug` prints the secret:\n{entry}");
}

mod one {
    pub struct Host;
}

mod two {
    pub struct Host;
}

/// Two importers whose short names collide print in full. Planted break: the entry's
/// `module_names` leaving out the importers.
#[test]
fn importers_sharing_a_short_name_are_printed_in_full() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<one::Host>::new(|m| m.import(Pool::new("db", 1))));
        m.import(Mod::<two::Host>::new(|m| m.import(Pool::new("db", 2))));
    }));
    let text = errors.to_string();
    for full in ["one::Host", "two::Host"] {
        assert!(text.contains(full), "{full} not printed in full:\n{text}");
    }
}

/// A registered secret is redacted on whichever side of the report it sits. Planted break: the
/// second side's printed copy left unscrubbed.
#[test]
fn a_registered_secret_is_redacted_on_either_side_of_the_report() {
    const SECRET: &str = "s3cret-token-value";
    struct Holding(&'static str);
    impl Module for Holding {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_owner::<Holding, _>(&"db")
        }

        fn settings(&self) -> ModuleSettings {
            ModuleSettings::new().with("auth", format_args!("{}", self.0))
        }

        fn register(&self, _: &mut ModuleDef<'_>) {}
    }
    for (first, second) in [("bearer s3cret-token-value", "none"), ("none", "bearer s3cret-token-value")] {
        let errors = wire_err(Mod::<A>::new(move |m| {
            m.secret(&ulo::Secret::new(SECRET.to_owned()));
            m.import(Holding(first));
            m.import(Holding(second));
        }));
        let text = errors.to_string();
        assert!(!text.contains(SECRET), "the secret is printed:\n{text}");
        assert!(text.contains("with `auth` = bearer [redacted]"), "{text}");
    }
}
