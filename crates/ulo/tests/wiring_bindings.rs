//! Section 10.1 step 2, the binding pass: duplicate singles, single/collection mixes, dangling
//! aliases, `try_value` failures and duplicate readiness checks.
//!
//! Capabilities pinned: 10 (a second single and a single beside a collection are refused rather
//! than replacing), 6 (an alias must point at something), 44 (reported by `wire()`), 45 (a failed
//! value is a typed entry, not a panic), 42 (the entry's error is redacted, and a secret under
//! four characters is matched against no message).

#[macro_use]
mod common;

use std::sync::Arc;

use common::*;
use ulo::{BindingKind, Key, Secret, WiringError};

struct A;
struct B;
struct C;
struct Svc;
struct Primary;
struct Replica;

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

struct MemRepo;
impl Repo for MemRepo {}
impl ulo::Construct for MemRepo {
    type Scope = ulo::scope::Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(MemRepo)
    }
}

#[test]
fn duplicate_single_binding() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.value(1_u32);
        m.value(2_u32);
    }));

    let (key, module, first, second) = entry!(errors, WiringError::DuplicateBinding { key, module, first, second } => (
        key.key(),
        module.to_string(),
        **first,
        **second,
    ));
    assert_eq!(key, Key::of::<u32, ()>());
    assert_eq!(module, mod_name::<A>());
    assert_ne!(first, second, "the two registrations are two locations");
    assert!(first.file().ends_with("wiring_bindings.rs") && second.file().ends_with("wiring_bindings.rs"));
    assert_eq!(second.line(), first.line() + 1, "the second registration is the next statement");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn duplicate_single_through_a_second_key() {
    // Two providers answering to `dyn Repo` collide on the second key, though their own keys differ.
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<PgRepo>().also_as::<dyn Repo>(|a| a);
        m.provide::<MemRepo>().also_as::<dyn Repo>(|a| a);
    }));

    let key = entry!(errors, WiringError::DuplicateBinding { key, .. } => key.key());
    assert_eq!(key, Key::of::<dyn Repo, ()>());
}

#[test]
fn qualifiers_and_modules_keep_equal_types_apart() {
    // The same type under two qualifiers is two keys; the same type in two modules is two
    // bindings, and nothing imports either, so nothing is ambiguous.
    wires(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc });
        m.singleton(|| async { Svc }).qualified::<Replica>();
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Svc });
        }));
    }));
}

#[test]
fn single_beside_collection() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc });
        m.contribute::<Svc>().value(Arc::new(Svc));
    }));

    let (key, module, single, collection) = entry!(errors, WiringError::KindMix { key, module, single, collection } => (
        key.key(),
        module.to_string(),
        **single,
        **collection,
    ));
    assert_eq!(key, Key::of::<Svc, ()>());
    assert_eq!(module, mod_name::<A>());
    assert_ne!(single, collection, "both registrations are named");
    assert_eq!(single.line() + 1, collection.line(), "the single is first, the contribution follows");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_collection_is_app_wide_so_a_single_in_another_module_mixes_with_it() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.singleton(|| async { Svc });
        }));
        m.import(Mod::<C>::new(|m| {
            m.contribute::<Svc>().value(Arc::new(Svc));
        }));
    }));

    let module = entry!(errors, WiringError::KindMix { module, .. } => module.to_string());
    assert_eq!(module, mod_name::<B>(), "the entry names the module of the single binding");
}

#[test]
fn a_collection_of_many_contributions_and_a_differently_keyed_single_are_not_a_mix() {
    wires(Mod::<A>::new(|m| {
        m.contribute::<Svc>().value(Arc::new(Svc));
        m.contribute::<Svc>().value(Arc::new(Svc));
        m.singleton(|| async { Svc }).qualified::<Replica>();
    }));
}

#[test]
fn alias_to_nothing() {
    let errors = wire_err(Mod::<A>::new(|m| m.alias::<Svc, Replica>().of::<Primary>()));

    let (alias, target, module) =
        entry!(errors, WiringError::DanglingAlias { alias, target, module, .. } => (*alias, *target, module.to_string()));
    assert_eq!(alias.key(), Key::of::<Svc, Replica>());
    assert_eq!(alias.kind(), BindingKind::Single);
    assert_eq!(target.key(), Key::of::<Svc, Primary>());
    assert_eq!(module, mod_name::<A>());
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_alias_to_a_binding_the_module_sees_is_accepted() {
    wires(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc }).qualified::<Primary>();
        m.alias::<Svc, Replica>().of::<Primary>();
    }));
}

#[test]
fn try_value_err_via_module_def_and_via_contribute() {
    const SECRET: &str = "hunter2-the-password";
    let errors = wire_err(Mod::<A>::new(|m| {
        m.secret(&Secret::new(SECRET.to_owned()));
        m.try_value::<u32, _>(Err(Boom::new(format!("could not reach the database with {SECRET}"))));
        m.contribute::<Svc>().try_value(Err::<Arc<Svc>, _>(Boom::new(format!("plugin rejected {SECRET}"))));
    }));

    let failed = entries!(errors, WiringError::ValueFailed { module, key, error, .. } => (module.to_string(), key.key(), key.kind(), error));
    assert_eq!(failed.len(), 2, "{errors}");
    let (module, key, kind, error) = &failed[0];
    assert_eq!((module.as_str(), *key, *kind), (mod_name::<A>().as_str(), Key::of::<u32, ()>(), BindingKind::Single));
    let original = error.downcast_ref::<Boom>().expect("the original error stays reachable by type");
    assert!(original.0.contains(SECRET), "the original is untouched, so the check below can fail");
    assert!(!error.to_string().contains(SECRET), "the stored error is redacted");

    let (module, key, kind, error) = &failed[1];
    assert_eq!((module.as_str(), *key, *kind), (mod_name::<A>().as_str(), Key::of::<Svc, ()>(), BindingKind::Collection));
    assert!(error.downcast_ref::<Boom>().is_some());
    assert!(!error.to_string().contains(SECRET));
    assert!(!errors.to_string().contains(SECRET), "nor does the report");
    // A derived `Debug` on the entries reaches the stored error's own `Debug`.
    for entry in &errors {
        assert!(!format!("{entry:?}").contains(SECRET), "nor an entry's Debug: {entry:?}");
    }
}

#[test]
fn two_readiness_checks_for_one_key() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc })
            .ready(|| async { Ok::<(), Boom>(()) })
            .ready(|| async { Ok::<(), Boom>(()) });
    }));

    let (key, module, first, second) = entry!(errors, WiringError::DuplicateReadiness { key, module, first, second } => (
        key.key(),
        module.to_string(),
        **first,
        **second,
    ));
    assert_eq!(key, Key::of::<Svc, ()>());
    assert_eq!(module, mod_name::<A>());
    assert_ne!(first, second);
    assert_eq!(second.line(), first.line() + 1);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn one_readiness_check_per_key_is_accepted() {
    wires(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc }).ready(|| async { Ok::<(), Boom>(()) });
    }));
}

#[test]
fn a_secret_under_four_characters_is_matched_against_no_message() {
    // Three characters in four bytes: the floor counts characters.
    const SHORT: &str = "p\u{e4}1";
    const FOUR: &str = "q7x9";
    let errors = wire_err(Mod::<A>::new(|m| {
        m.secret(&Secret::new(SHORT.to_owned()));
        m.secret(&Secret::new(FOUR.to_owned()));
        m.try_value::<u32, _>(Err(Boom::new(format!("tokens {SHORT} and {FOUR} refused"))));
    }));
    let (key, error) = entry!(errors, WiringError::ValueFailed { key, error, .. } => (key.key(), error.to_string()));
    assert_eq!(key, Key::of::<u32, ()>());
    assert_eq!(error, format!("tokens {SHORT} and [redacted] refused"), "the four-character secret is replaced and the shorter one left");
}
