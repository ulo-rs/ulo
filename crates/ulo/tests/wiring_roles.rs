//! Section 10.1 step 2, role keys (section 7): a qualified contribution and a single binding under
//! a role key are refused, because a transport reads only the role key's unqualified collection.
//!
//! Capabilities pinned: 28 (a binding under a role key registers that role; the refusals are the
//! other half of the rule), 25 (the role key is per transport: `AnyGuard<TA>` is not
//! `AnyGuard<TB>`), 44.
//!
//! A role key is known to the freeze when a module contributes under it through
//! `ModuleDef::enhancer` or a mounted handler's transport reads it; every test here marks it with
//! one `enhancer` contribution and then writes the refused form beside it.

#[macro_use]
mod common;

use common::*;
use ulo::{AnyGuard, BindingKind, Key, WiringError};

struct A;
struct Replica;

fn mark_the_role(m: &mut ulo::ModuleDef<'_>) {
    m.enhancer::<AnyGuard<TA>>().provide::<GuardA>(|a| a);
}

#[test]
fn qualified_contribution_under_a_role_key() {
    let line = line!() + 3;
    let errors = wire_err(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.contribute::<AnyGuard<TA>>().qualified::<Replica>().provide::<GuardA>(|a| a);
    }));

    let (key, module, at) = entry!(errors, WiringError::QualifiedRoleContribution { key, module, at } => (*key, module.to_string(), **at));
    assert_eq!(key.key(), Key::of::<AnyGuard<TA>, Replica>());
    assert_eq!(key.kind(), BindingKind::Collection, "the qualified collection is named, not the role key");
    assert_eq!(module, mod_name::<A>());
    assert_eq!(at.line(), line, "the entry points at the contribution");
    assert_eq!(errors.len(), 1, "{errors}");
    // The help line is text the design fixes (section 12).
    assert!(errors.to_string().contains("contribute it unqualified"), "{errors}");
}

#[test]
fn an_unqualified_contribution_under_a_role_key_is_accepted() {
    wires(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.contribute::<AnyGuard<TA>>().provide::<GuardA>(|a| a);
    }));
}

#[test]
fn a_qualified_contribution_under_a_key_that_is_not_a_role_is_accepted() {
    wires(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.contribute::<dyn std::any::Any + Send + Sync>().qualified::<Replica>().value(std::sync::Arc::new(1_u8));
    }));
}

/// The refused key of the one `SingleRoleBinding` in the report, and the line it was written on.
fn single_role_binding(errors: &ulo::WiringErrors) -> (Key, BindingKind, String, u32) {
    let (key, module, at) = entry!(errors, WiringError::SingleRoleBinding { key, module, at } => (*key, module.to_string(), **at));
    (key.key(), key.kind(), module, at.line())
}

#[test]
fn also_as_on_a_provided_type_under_a_role_key() {
    let line = line!() + 3;
    let errors = wire_err(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.provide::<GuardA>().also_as::<AnyGuard<TA>>(|a| a);
    }));

    let (key, kind, module, at) = single_role_binding(&errors);
    assert_eq!((key, kind), (Key::of::<AnyGuard<TA>, ()>(), BindingKind::Single));
    assert_eq!(module, mod_name::<A>());
    assert_eq!(at, line, "the entry points at the `also_as` call");
    assert_eq!(errors.len(), 1, "{errors}");
    assert!(errors.to_string().contains("contribute it"), "{errors}");
}

#[test]
fn also_as_on_a_value_under_a_role_key() {
    let errors = wire_err(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.value(GuardA).also_as::<AnyGuard<TA>>(|a| a);
    }));
    let (key, kind, ..) = single_role_binding(&errors);
    assert_eq!((key, kind), (Key::of::<AnyGuard<TA>, ()>(), BindingKind::Single));
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn also_as_on_a_factory_under_a_role_key() {
    let errors = wire_err(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.singleton(|| async { GuardA }).also_as::<AnyGuard<TA>>(|a| a);
    }));
    let (key, kind, ..) = single_role_binding(&errors);
    assert_eq!((key, kind), (Key::of::<AnyGuard<TA>, ()>(), BindingKind::Single));
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_alias_under_a_role_key() {
    // The alias key is `AnyGuard<TA> @ Replica`. Its target cannot be a non-role binding of the
    // same type, so the alias dangles as well; the role entry is the one under test.
    let errors = wire_err(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.alias::<AnyGuard<TA>, Replica>().of::<A>();
    }));
    let (key, kind, ..) = single_role_binding(&errors);
    assert_eq!((key, kind), (Key::of::<AnyGuard<TA>, Replica>(), BindingKind::Single));
    assert_eq!(entries!(errors, WiringError::DanglingAlias { .. } => ()).len(), 1, "{errors}");
}

#[test]
fn the_role_is_per_transport() {
    // `AnyGuard<TB>` is read by no handler and marked by no `enhancer` call, so a single binding
    // under it is an ordinary binding while `AnyGuard<TA>` beside it is a role.
    struct GuardB;
    impl ulo::Guard<TB> for GuardB {
        async fn can_activate(&self, _cx: &()) -> Result<bool, ulo::BoxError> {
            Ok(true)
        }
    }
    let errors = wire_err(Mod::<A>::new(|m| {
        mark_the_role(m);
        m.value(GuardA).also_as::<AnyGuard<TA>>(|a| a);
        m.value(GuardB).also_as::<AnyGuard<TB>>(|a| a);
    }));
    let (key, ..) = single_role_binding(&errors);
    assert_eq!(key, Key::of::<AnyGuard<TA>, ()>(), "only the marked transport's key is a role");
    assert_eq!(errors.len(), 1, "{errors}");
}
