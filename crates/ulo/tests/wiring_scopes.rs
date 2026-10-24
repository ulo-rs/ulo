//! Section 10.1 step 5, the needs-execution pass: a binding that cannot run in an execution
//! reading per-execution data (`ScopeViolation`), hooks on a binding the pass inferred
//! per-execution (`HooksOnPerExecution`), an explicit-singleton enhancer closure reading
//! per-execution data (`ClosureScopeViolation`), and a hook, readiness, module-hook or metadata
//! closure that runs where no execution exists (`ClosureNeedsExecution`).
//!
//! Capabilities pinned: 21 (a singleton on an execution-scoped binding is refused at startup,
//! directly and through a transient), 22 (the accepted half: an `Auto` controller or enhancer
//! reading execution data wires), 41 (the wiring half: a hook on an `Auto` type inferred
//! per-execution), 26 (a closure's scope rule, its refusal half), 44.

#[macro_use]
mod common;

use common::*;
use ulo::scope::{Auto, PerExecution, Singleton, Transient};
use ulo::{AnyGuard, Dependencies, Ext, HandlerSpec, Key, ScopeKind, TypeName, WiringError};

struct A;
struct B;
struct C;
struct User;

reader!(Report, Singleton, user: Ext<User>);
reader!(AutoReport, Auto, user: Ext<User>);
reader!(Tr, Transient, user: Ext<User>);
reader!(ViaTransient, Singleton, tr: ulo::Dep<Tr>);
reader!(Audit, PerExecution, user: Ext<User>);
reader!(OverAudit, Singleton, audit: ulo::Dep<Audit>);

/// (declared scope, by_closure, module, path) of the `ScopeViolation` for `binding`.
fn violation(errors: &ulo::WiringErrors, binding: Key) -> (ScopeKind, bool, String, Vec<String>) {
    let found = entries!(errors, WiringError::ScopeViolation { binding: b, declared, by_closure, module, path }
        => (b.key(), *declared, *by_closure, module.to_string(), path.clone()));
    found
        .into_iter()
        .find(|(key, ..)| *key == binding)
        .map(|(_, declared, by_closure, module, path)| (declared, by_closure, module, path))
        .unwrap_or_else(|| panic!("no ScopeViolation for the key; the report:\n{errors}"))
}

#[test]
fn a_singleton_reading_execution_data_is_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<Report>();
    }));
    let (declared, by_closure, module, path) = violation(&errors, Key::of::<Report, ()>());
    assert_eq!(declared, ScopeKind::Singleton);
    assert!(!by_closure, "a `Construct` type writes its scope on the type");
    assert_eq!(module, mod_name::<A>());
    assert_eq!(path.len(), 2, "the binding, then the read: {path:?}");
    assert!(path[1].contains("Ext<"), "the path ends at the `Ext`: {path:?}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_singleton_reaching_execution_data_through_a_transient_is_refused_once() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<Tr>();
        m.provide::<ViaTransient>();
    }));
    let (declared, _, _, path) = violation(&errors, Key::of::<ViaTransient, ()>());
    assert_eq!(declared, ScopeKind::Singleton);
    assert_eq!(path.len(), 3, "the singleton, the transient, the read: {path:?}");
    assert!(path[1].contains("transient"), "the middle step carries its scope: {path:?}");
    assert!(path[2].contains("Ext<"), "{path:?}");
    // The transient itself may read execution data; only its singleton reader is refused.
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_singleton_reading_an_execution_scoped_binding_is_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.provide::<Audit>();
            m.provide::<OverAudit>();
        }));
    }));
    let (declared, _, module, path) = violation(&errors, Key::of::<OverAudit, ()>());
    assert_eq!(declared, ScopeKind::Singleton);
    assert_eq!(module, mod_name::<B>(), "the module the binding is in, not the root");
    assert!(path.iter().any(|step| step.contains("execution")), "{path:?}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_factory_singleton_is_refused_by_closure() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|_user: Ext<User>| async { B });
    }));
    let (declared, by_closure, ..) = violation(&errors, Key::of::<B, ()>());
    assert_eq!(declared, ScopeKind::Singleton);
    assert!(by_closure, "a factory's scope is written where the closure is registered");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_auto_provider_reading_execution_data_is_refused_by_type_and_by_closure() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<AutoReport>();
        m.with(|_user: Ext<User>| async { B });
    }));
    let by_type = violation(&errors, Key::of::<AutoReport, ()>());
    assert_eq!((by_type.0, by_type.1), (ScopeKind::Auto, false));
    let by_closure = violation(&errors, Key::of::<B, ()>());
    assert_eq!((by_closure.0, by_closure.1), (ScopeKind::Auto, true));
    assert_eq!(errors.len(), 2, "{errors}");
}

#[test]
fn a_controller_declared_singleton_that_reads_execution_data_is_refused() {
    controller!(Pinned, Singleton, [user: Ext<User>], |m| m.handler::<TA, _>(HandlerSpec::new("h", ())));
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<Pinned>();
    }));
    let (declared, by_closure, ..) = violation(&errors, Key::of::<Pinned, ()>());
    assert_eq!((declared, by_closure), (ScopeKind::Singleton, false), "opting out of the inference makes the read a violation");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn bindings_built_in_an_execution_may_read_execution_data() {
    controller!(Ctl, Auto, [user: Ext<User>], |m| m.handler::<TA, _>(HandlerSpec::new("h", ())));
    wires(Mod::<A>::new(|m| {
        m.provide::<Audit>();
        m.provide::<Tr>();
        m.controller::<Ctl>();
        m.enhancer::<AnyGuard<TA>>().with(|_user: Ext<User>| async { GuardA }, |a| a);
    }));
}

/// An `Auto` controller with a trait hook, written out because `controller!` leaves `hooks` empty.
struct TraitHooked;

impl ulo::Construct for TraitHooked {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Ext<User>>("user");
    }

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(TraitHooked)
    }

    fn hooks(h: &mut ulo::Hooks<Self>) {
        h.on_module_init();
    }
}

impl ulo::OnModuleInit for TraitHooked {
    async fn on_module_init(&self) -> Result<(), ulo::BoxError> {
        Ok(())
    }
}

impl ulo::Controller for TraitHooked {
    fn mount(m: &mut ulo::Mount<'_>) {
        m.handler::<TA, _>(HandlerSpec::new("h", ()));
    }
}

#[test]
fn hooks_on_an_auto_type_inferred_per_execution_are_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.enhancer::<AnyGuard<TA>>()
            .with(|_user: Ext<User>| async { GuardA }, |a| a)
            .on_init(|| async { Ok::<(), Boom>(()) });
        m.controller::<TraitHooked>();
    }));
    let found = entries!(errors, WiringError::HooksOnPerExecution { binding, module } => (binding.key(), module.to_string()));
    assert_eq!(found.len(), 2, "a closure hook on an enhancer and a trait hook on a controller: {errors}");
    assert!(found.iter().all(|(_, module)| *module == mod_name::<A>()));
    assert!(found.iter().any(|(key, _)| *key == Key::of::<TraitHooked, ()>()), "the controller with a trait hook");
    assert_eq!(errors.len(), 2, "{errors}");
}

#[test]
fn a_readiness_check_on_a_binding_inferred_per_execution_is_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<B>::new(|m| {
            m.enhancer::<AnyGuard<TA>>()
                .with(|_user: Ext<User>| async { GuardA }, |a| a)
                .ready(|| async { Ok::<(), Boom>(()) });
        }));
    }));
    let module = entry!(errors, WiringError::HooksOnPerExecution { module, .. } => module.to_string());
    assert_eq!(module, mod_name::<B>(), "the module the contribution is in, not the root");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn hooks_on_a_singleton_enhancer_and_a_per_execution_enhancer_without_hooks_are_accepted() {
    wires(Mod::<A>::new(|m| {
        m.enhancer::<AnyGuard<TA>>().singleton(|| async { GuardA }, |a| a).on_init(|| async { Ok::<(), Boom>(()) });
        m.enhancer::<AnyGuard<TA>>().with(|_user: Ext<User>| async { GuardA }, |a| a);
    }));
}

/// An `Auto` controller whose one handler declares a guard closure in the given tier, built by
/// `$build` over the tier's `EnhancerSpec`.
macro_rules! guard_closure_controller {
    ($name:ident, $tier:ident, $build:expr) => {
        struct $name;

        impl ulo::Construct for $name {
            type Scope = Auto;

            fn dependencies(_d: &mut Dependencies) {}

            async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                Ok($name)
            }
        }

        impl ulo::Controller for $name {
            fn mount(m: &mut ulo::Mount<'_>) {
                let mut spec = ulo::EnhancerSpec::<TA>::new();
                ($build)(&mut spec);
                m.handler::<TA, _>(HandlerSpec::new("h", ()).$tier(spec));
            }
        }
    };
}

guard_closure_controller!(SingletonAtController, controller, |s: &mut ulo::EnhancerSpec<TA>| {
    s.guard_with_in::<Singleton, _, _>(|_user: Ext<User>| async { GuardA });
});
guard_closure_controller!(SingletonAtMethod, method, |s: &mut ulo::EnhancerSpec<TA>| {
    s.guard_with_in::<Singleton, _, _>(|_user: Ext<User>| async { GuardA });
});
guard_closure_controller!(AutoAtMethod, method, |s: &mut ulo::EnhancerSpec<TA>| {
    s.guard_with(|_user: Ext<User>| async { GuardA });
});
guard_closure_controller!(PerExecutionAtMethod, method, |s: &mut ulo::EnhancerSpec<TA>| {
    s.guard_with_in::<PerExecution, _, _>(|_user: Ext<User>| async { GuardA });
});
guard_closure_controller!(SingletonReadingNothing, method, |s: &mut ulo::EnhancerSpec<TA>| {
    s.guard_with_in::<Singleton, _, _>(|| async { GuardA });
});

#[test]
fn an_explicit_singleton_closure_enhancer_reading_execution_data_is_refused_in_both_tiers() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<SingletonAtController>();
        m.controller::<SingletonAtMethod>();
    }));
    let found = entries!(errors, WiringError::ClosureScopeViolation { closure, transport, role, path, .. }
        => (closure.clone(), *transport, *role, path.clone()));
    assert_eq!(found.len(), 2, "{errors}");
    for (_, transport, role, path) in &found {
        assert_eq!(*transport, TypeName::of::<TA>());
        assert_eq!(*role, "guard");
        assert!(path.iter().any(|step| step.contains("Ext<")), "{path:?}");
    }
    let named = |handler: &str| {
        found.iter().find(|(closure, ..)| closure.contains(handler)).unwrap_or_else(|| panic!("no closure of `{handler}`: {found:?}")).0.clone()
    };
    assert!(named("SingletonAtController::").starts_with("controller-level guard #1"), "{found:?}");
    assert!(named("SingletonAtMethod::").starts_with("method-level guard #1"), "{found:?}");
    assert_eq!(errors.len(), 2, "{errors}");
}

#[test]
fn an_auto_or_per_execution_closure_enhancer_may_read_execution_data_and_a_singleton_may_read_nothing() {
    wires(Mod::<A>::new(|m| {
        m.controller::<AutoAtMethod>();
        m.controller::<PerExecutionAtMethod>();
        m.controller::<SingletonReadingNothing>();
    }));
}

/// Metadata whose declared dependency reads an extension.
#[derive(Default)]
struct MetaReads;

impl ulo::Meta for MetaReads {
    fn dependencies(&self, d: &mut Dependencies) {
        d.add::<Ext<User>>();
    }
}

#[test]
fn hook_readiness_module_hook_and_metadata_closures_may_not_read_execution_data() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.value(B).ready(|_user: Ext<User>| async { Ok::<(), Boom>(()) });
        m.value(C).on_init(|_user: Ext<User>| async { Ok::<(), Boom>(()) });
        m.on_init(|_user: Ext<User>| async { Ok::<(), Boom>(()) });
        m.meta::<MetaReads>();
    }));
    let found = entries!(errors, WiringError::ClosureNeedsExecution { closure, path, at } => (closure.clone(), path.clone(), at.is_some()));
    assert_eq!(found.len(), 4, "{errors}");
    let named = |what: &str| found.iter().find(|(closure, ..)| closure.contains(what)).unwrap_or_else(|| panic!("no `{what}` in {found:?}"));
    assert!(named("readiness check").2, "a readiness closure points at its call");
    assert!(named("OnModuleInit` hook of `").2, "a binding's hook closure points at its call");
    assert!(named("hook of module").2, "a module hook points at its call");
    assert!(!named("metadata").2, "a metadata value has no call site");
    assert!(found.iter().all(|(_, path, _)| path.len() == 1 && path[0].contains("Ext<")), "{found:?}");
    assert_eq!(errors.len(), 4, "{errors}");
}

#[test]
fn an_optional_read_of_execution_data_in_a_hook_closure_is_refused_too() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.value(B).on_init(|_user: Option<Ext<User>>| async { Ok::<(), Boom>(()) });
    }));
    entry!(errors, WiringError::ClosureNeedsExecution { .. } => ());
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_hook_closure_reading_a_singleton_is_accepted() {
    wires(Mod::<A>::new(|m| {
        m.value(B);
        m.value(C).on_init(|_b: ulo::Dep<B>| async { Ok::<(), Boom>(()) });
    }));
}
