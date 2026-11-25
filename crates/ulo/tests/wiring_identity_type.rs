//! `#[module(identity = type)]`: a module identified by its type whatever its fields hold, for
//! fields that are `Clone` but not `Eq + Hash`. With fields it is registered once; a second
//! registration is `WiringError::RegisteredTwice`, since the second's fields would go unused.

#[macro_use]
mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use common::*;
use ulo::{LoadError, Module, ModuleDef, ModuleIdentity, ModuleSite, WiringError, injectable, module};

/// `Clone` and nothing else, as `ulo_config::Config` is.
#[derive(Clone)]
struct Opaque(#[allow(dead_code)] u8);

struct A;
struct B;

#[injectable]
struct Shared;

#[module(providers = [Shared], exports = [Shared])]
struct SharedModule;

/// Registers once, or fails `wire()`; imports a unit module beside its field.
#[module(identity = type, imports = [SharedModule])]
struct Holding {
    #[allow(dead_code)]
    opaque: Opaque,
}

#[module(identity = type, providers = [Shared])]
struct Bare;

#[module(identity = type, imports = [Holding { opaque: Opaque(1) }])]
struct Root {
    #[allow(dead_code)]
    opaque: Opaque,
}

/// Planted break: the macro writing `of_value(self)` for `identity = type`, which does not compile
/// for a field that is not `Eq + Hash`.
#[test]
fn a_root_holding_a_value_that_is_not_hashable_wires() {
    wire_result(Root { opaque: Opaque(0) }).unwrap_or_else(|error| panic!("{error}"));
}

static FIRST: AtomicU32 = AtomicU32::new(0);
static AGAIN: AtomicU32 = AtomicU32::new(0);

/// Planted break: `Differing::of` not reading the settings' mark, which wires the second import
/// with its fields unused.
#[test]
fn the_same_module_imported_twice_is_refused_naming_both_sites() {
    #[rustfmt::skip]
    let root = Mod::<A>::new(|m| {
        m.import(Holding { opaque: Opaque(1) }); FIRST.store(line!(), Ordering::SeqCst);
        m.import(Mod::<B>::new(|m| {
            m.import(Holding { opaque: Opaque(2) }); AGAIN.store(line!(), Ordering::SeqCst);
        }));
    });
    let errors = wire_err(root);
    let (module, first, second) =
        entry!(errors, WiringError::RegisteredTwice { module, first, second, .. } => (module.to_string(), first.clone(), second.clone()));
    assert_eq!(module, "Holding");
    let site = |site: &ModuleSite| match site {
        ModuleSite::Import { importer, at, .. } => (importer.to_string(), at.file().to_owned(), at.line()),
        other => panic!("expected an import site, got {other:?}"),
    };
    assert_eq!(site(&first), (mod_name::<A>(), file!().to_owned(), FIRST.load(Ordering::SeqCst)));
    assert_eq!(site(&second), (mod_name::<B>(), file!().to_owned(), AGAIN.load(Ordering::SeqCst)));
    assert_eq!(errors.len(), 1, "{errors}");

    let expected = format!(
        "Holding is registered twice, and its fields are not its identity\n\
         ├─ imported by {a} at {file}:{first}\n\
         ├─ imported again by {b} at {file}:{again}\n\
         └─ help: a module whose fields are not its identity is registered once; give it an identity (`Eq + Hash + Clone` fields, or `settings()`) to share it",
        a = mod_name::<A>(),
        b = mod_name::<B>(),
        file = file!(),
        first = FIRST.load(Ordering::SeqCst),
        again = AGAIN.load(Ordering::SeqCst),
    );
    assert_eq!(errors.iter().next().map(ToString::to_string), Some(expected));
}

/// Three registrations are two refusals, each naming the first import and its own.
#[test]
fn each_later_registration_is_its_own_entry() {
    let root = Mod::<A>::new(|m| {
        m.import(Holding { opaque: Opaque(1) });
        m.import(Holding { opaque: Opaque(1) });
        m.import(Holding { opaque: Opaque(1) });
    });
    let errors = wire_err(root);
    assert_eq!(entries!(errors, WiringError::RegisteredTwice { .. } => ()).len(), 2, "{errors}");
}

/// A module with no fields has nothing to differ in, so `identity = type` changes nothing for it.
/// Planted break: marking a field-less module unshared.
#[test]
fn a_module_without_fields_deduplicates_as_any_type_identified_module() {
    let root = Mod::<A>::new(|m| {
        m.import(Bare);
        m.import(Mod::<B>::new(|m| m.import(Bare)));
    });
    wire_result(root).unwrap_or_else(|error| panic!("{error}"));
}

/// A hand-written module claiming `Holding`'s identity, its settings unmarked.
struct Claiming;

impl Module for Claiming {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Holding>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// The mark is read on either registration, so the order of the two does not decide whether the
/// second is refused. Planted break: `Differing::of` reading the first registration's mark alone.
#[test]
fn a_marked_registration_after_an_unmarked_one_of_its_identity_is_refused() {
    let root = Mod::<A>::new(|m| {
        m.import(Claiming);
        m.import(Holding { opaque: Opaque(1) });
    });
    let errors = wire_err(root);
    assert_eq!(entries!(errors, WiringError::RegisteredTwice { .. } => ()).len(), 1, "{errors}");
}

/// Planted break: `reload_differs` not asking `Differing::of`, which returns the held handle.
#[tokio::test]
async fn loading_a_registered_module_again_is_refused() {
    let app = connects(Mod::<A>::new(|m| m.import(Holding { opaque: Opaque(1) }))).await;
    let handle = app.handle();
    match handle.load(Holding { opaque: Opaque(2) }).await {
        Err(LoadError::Wiring(errors)) => {
            let (first, second) = entry!(errors, WiringError::RegisteredTwice { first, second, .. } => (first.clone(), second.clone()));
            assert!(matches!(first, ModuleSite::Import { .. }), "{first:?}");
            assert!(matches!(second, ModuleSite::Load { .. }), "{second:?}");
        }
        Err(other) => panic!("expected wiring errors, got: {other}"),
        Ok(_) => panic!("the load was accepted"),
    }
    close(app).await;
}
