//! Section 10.1 step 6, the environment: a `Timer` wherever a wait is written (section 3.9), and
//! no zero bound on any item `resolve_bound` resolves.
//!
//! Capabilities pinned: 42 (readiness bounds and backoff need a clock), 44 (reported by `wire()`
//! at the call that wrote the wait: `.timeout(..)`, `.attempt_timeout(..)` or `.backoff(..)`, and
//! the registration for a trait const), 45. `Default` and `Unbounded` need no clock.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::{App, Bound, BoundItem, Dep, HookKind, Key, Timer, WiringError};

struct A;
struct Svc;

const SECOND: Duration = Duration::from_secs(1);

fn check() -> impl Fn() -> std::future::Ready<Result<(), Boom>> + Send + Sync + 'static {
    || std::future::ready(Ok(()))
}

/// The `BoundWithoutTimer` entries as `(item, line)`.
fn bounds(errors: &ulo::WiringErrors) -> Vec<(String, u32)> {
    entries!(errors, WiringError::BoundWithoutTimer { item, at } => (item.clone(), at.line()))
}

#[test]
fn a_construction_timeout_on_an_app_with_no_timer() {
    // The registration and the bound on two lines, so the entry tells them apart.
    let registered = line!() + 3;
    let timeout = line!() + 3;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc })
            .timeout(SECOND);
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 1, "{errors}");
    assert!(found[0].0.contains("Svc"), "the item names the binding: {}", found[0].0);
    assert_eq!(found[0].1, timeout, "the entry points at the `.timeout(..)`, not the registration on line {registered}");
}

#[test]
fn a_constructs_after_bound_on_an_app_with_no_timer() {
    struct Slow;
    impl ulo::Construct for Slow {
        type Scope = ulo::scope::Singleton;
        const CONSTRUCT_TIMEOUT: Bound = Bound::After(SECOND);

        fn dependencies(_d: &mut ulo::Dependencies) {}

        async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
            Ok(Slow)
        }
    }
    let registered = line!() + 2;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<Slow>();
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 1, "{errors}");
    assert!(found[0].0.contains("Slow"), "{}", found[0].0);
    assert_eq!(found[0].1, registered, "a trait const has no call: the registration that brought the type in");
}

#[test]
fn a_readiness_timeout_and_attempt_timeout_on_an_app_with_no_timer() {
    // One call per line, so the registration, the `.ready` and the bound are three lines.
    let ready = line!() + 5;
    let whole = line!() + 5;
    let attempt = line!() + 7;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc })
            .ready(check())
            .timeout(SECOND);
        m.singleton(|| async { A })
            .ready(check())
            .attempt_timeout(SECOND);
    }));
    let mut found = bounds(&errors);
    found.sort_by_key(|(_, line)| *line);
    assert_eq!(found.len(), 2, "{errors}");
    assert!(found[0].0.contains("readiness `.timeout`") && found[0].0.contains("Svc"), "{}", found[0].0);
    assert_eq!(found[0].1, whole, "the entry points at the `.timeout(..)`, not the `.ready(..)` on line {ready}");
    assert!(found[1].0.contains("readiness `.attempt_timeout`") && found[1].0.contains("A"), "{}", found[1].0);
    assert_eq!(found[1].1, attempt, "the `.attempt_timeout(..)` line");
}

#[test]
fn a_hook_timeout_and_a_module_hook_timeout_on_an_app_with_no_timer() {
    let hook = line!() + 5;
    let module_hook = line!() + 6;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc })
            .on_init(check())
            .timeout(SECOND);
        m.on_init(check())
            .timeout(SECOND);
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 2, "{errors}");
    let line_of = |needle: &str| found.iter().find(|(item, _)| item.contains(needle)).map(|&(_, line)| line);
    assert_eq!(line_of("`Svc`"), Some(hook), "the binding's hook, at its `.timeout(..)`: {found:?}");
    assert_eq!(line_of(&mod_name::<A>()), Some(module_hook), "the module's hook, at its `.timeout(..)`: {found:?}");
}

#[test]
fn a_trait_hooks_after_bound_on_an_app_with_no_timer() {
    struct Hooked;
    impl ulo::Construct for Hooked {
        type Scope = ulo::scope::Singleton;

        fn dependencies(_d: &mut ulo::Dependencies) {}

        async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
            Ok(Hooked)
        }

        fn hooks(h: &mut ulo::Hooks<Self>) {
            h.on_module_init();
        }
    }
    impl ulo::OnModuleInit for Hooked {
        const TIMEOUT: Bound = Bound::After(SECOND);

        async fn on_module_init(&self) -> Result<(), ulo::BoxError> {
            Ok(())
        }
    }
    let registered = line!() + 2;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<Hooked>();
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 1, "{errors}");
    assert!(found[0].0.contains("OnModuleInit") && found[0].0.contains("Hooked"), "{}", found[0].0);
    assert_eq!(found[0].1, registered, "a trait const has no call: the registration that brought the type in");
}

#[test]
fn default_and_unbounded_need_no_timer() {
    wires(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc });
        m.singleton(|| async { A }).unbounded();
        m.singleton(|| async { SECOND }).ready(check()).unbounded();
        m.singleton(|| async { 1_u8 }).on_init(check()).unbounded();
        m.on_init(check()).unbounded();
    }));
}

#[test]
fn the_same_bounds_wire_on_an_app_with_a_timer() {
    wires_with(
        App::builder(Mod::<A>::new(|m| {
            m.singleton(|| async { Svc }).timeout(SECOND);
            m.singleton(|| async { A }).ready(check()).timeout(SECOND);
            m.singleton(|| async { SECOND }).ready(check()).attempt_timeout(SECOND).backoff(SECOND);
            m.singleton(|| async { 1_u8 }).on_init(check()).timeout(SECOND);
            m.on_init(check()).timeout(SECOND);
        }))
        .timer(Frozen::new()),
    );
}

#[test]
fn backoff_reports_its_own_line() {
    let ready = line!() + 3;
    let backoff = line!() + 3;
    let errors = wire_err(Mod::<A>::new(|m| {
        let handle = m.singleton(|| async { Svc }).ready(check());
        handle.backoff(SECOND);
    }));

    let (binding, line) = entry!(errors, WiringError::BackoffWithoutTimer { binding, at } => (binding.clone(), at.line()));
    assert!(binding.contains("Svc"), "{binding}");
    assert_eq!(line, backoff, "the `.backoff(..)` line, not the `.ready(..)` line {ready}");
    assert!(bounds(&errors).is_empty(), "a backoff is not a bound: {errors}");
}

#[test]
fn a_zero_backoff_waits_for_nothing_and_needs_no_timer() {
    wires(Mod::<A>::new(|m| {
        let handle = m.singleton(|| async { Svc }).ready(check());
        handle.backoff(Duration::ZERO);
    }));
}

#[test]
fn a_builder_knob_on_an_app_with_no_timer() {
    type Knob = fn(ulo::AppBuilder) -> ulo::AppBuilder;
    let knobs: [(&str, Knob); 4] = [
        ("drain_timeout", |b| b.drain_timeout(SECOND)),
        ("shutdown_timeout", |b| b.shutdown_timeout(SECOND)),
        ("hook_timeout", |b| b.hook_timeout(SECOND)),
        ("construct_timeout", |b| b.construct_timeout(SECOND)),
    ];
    for (name, set) in knobs {
        let errors = wire_err_with(set(App::builder(Empty)));
        let knob = entry!(errors, WiringError::KnobWithoutTimer { knob } => *knob);
        assert_eq!(knob, name);
        assert_eq!(errors.len(), 1, "{errors}");

        wires_with(set(App::builder(Empty)).timer(Frozen::new()));
    }

    let all = App::builder(Empty).construct_timeout(SECOND).hook_timeout(SECOND).shutdown_timeout(SECOND).drain_timeout(SECOND);
    let errors = wire_err_with(all);
    assert_eq!(
        entries!(errors, WiringError::KnobWithoutTimer { knob } => *knob),
        ["drain_timeout", "shutdown_timeout", "hook_timeout", "construct_timeout"],
        "every knob set is reported, in one fixed order"
    );
}

#[test]
fn a_module_binding_dyn_timer_is_a_second_source() {
    struct MyTimer(Frozen);
    impl Timer for MyTimer {
        fn sleep(&self, d: Duration) -> ulo::BoxFuture<'static, ()> {
            self.0.sleep(d)
        }

        fn now(&self) -> std::time::Instant {
            self.0.now()
        }
    }
    let root = Mod::<A>::new(|m| {
        m.import(Mod::<Svc>::new(|m| {
            m.singleton(|| async { MyTimer(Frozen::new()) }).also_as::<dyn Timer>(|a| a);
            m.export::<dyn Timer>();
        }));
        m.singleton(|_t: Dep<dyn Timer>| async { Svc });
    });
    let errors = wire_err_with(App::builder(root).timer(Frozen::new()));

    let (key, sources) = entry!(errors, WiringError::Ambiguous { key, sources, .. } => (key.key(), sources.len()));
    assert_eq!(key, Key::of::<dyn Timer, ()>());
    assert_eq!(sources, 2, "the app's clock and the module's");
}

// ---- Zero bounds (A3): refused on every item `resolve_bound` resolves, whatever the clock. ----

const ZERO: Duration = Duration::ZERO;

/// The `ZeroBound` entries as `(item, binding key, module, line)`.
fn zeros(errors: &ulo::WiringErrors) -> Vec<(BoundItem, Option<Key>, String, u32)> {
    entries!(errors, WiringError::ZeroBound { item, binding, module, at } => (*item, binding.as_ref().map(|b| b.key()), module.to_string(), at.line()))
}

/// `root` wired with no clock and with one: the same zero bounds refused both times, and on the app
/// with no clock as `ZeroBound` alone, never also as `BoundWithoutTimer`.
fn refused_either_way(root: impl Fn() -> Mod<A>) -> Vec<(BoundItem, Option<Key>, String, u32)> {
    let untimed = wire_err(root());
    assert!(bounds(&untimed).is_empty(), "a zero bound is reported once, as `ZeroBound`: {untimed}");
    let timed = wire_err_with(App::builder(root()).timer(Frozen::new()));
    let found = zeros(&timed);
    assert_eq!(zeros(&untimed), found, "the clock plays no part");
    assert_eq!(found.len(), timed.len(), "nothing else is reported: {timed}");
    found
}

#[test]
fn a_zero_construction_timeout_is_refused_at_its_timeout_call() {
    struct Inner;
    let timeout = line!() + 5;
    let found = refused_either_way(|| {
        Mod::<A>::new(|m| {
            m.import(Mod::<Inner>::new(|m| {
                m.singleton(|| async { Svc })
                    .timeout(ZERO);
            }));
        })
    });
    assert_eq!(found, [(BoundItem::Construction, Some(Key::of::<Svc, ()>()), mod_name::<Inner>(), timeout)], "the module that binds it");
}

#[test]
fn a_zero_readiness_timeout_and_attempt_timeout_are_refused_at_their_calls() {
    let whole = line!() + 6;
    let attempt = line!() + 8;
    let found = refused_either_way(|| {
        Mod::<A>::new(|m| {
            m.singleton(|| async { Svc })
                .ready(check())
                .timeout(ZERO);
            m.singleton(|| async { SECOND })
                .ready(check())
                .attempt_timeout(ZERO);
        })
    });
    assert_eq!(
        found,
        [
            (BoundItem::ReadinessWhole, Some(Key::of::<Svc, ()>()), mod_name::<A>(), whole),
            (BoundItem::ReadinessAttempt, Some(Key::of::<Duration, ()>()), mod_name::<A>(), attempt),
        ]
    );
}

#[test]
fn a_zero_hook_timeout_on_a_binding_and_on_a_module_is_refused_at_its_call() {
    struct Hooks;
    let hook = line!() + 6;
    let module_hook = line!() + 8;
    let found = refused_either_way(|| {
        Mod::<A>::new(|m| {
            m.singleton(|| async { Svc })
                .on_destroy(|| async {})
                .timeout(ZERO);
            m.import(Mod::<Hooks>::new(|m| {
                m.on_init(check())
                    .timeout(ZERO);
            }));
        })
    });
    assert_eq!(
        found,
        [
            (BoundItem::Hook(HookKind::OnModuleDestroy), Some(Key::of::<Svc, ()>()), mod_name::<A>(), hook),
            (BoundItem::Hook(HookKind::OnModuleInit), None, mod_name::<Hooks>(), module_hook),
        ]
    );
}

#[test]
fn a_zero_trait_const_is_refused_at_the_registration_that_brought_the_type_in() {
    struct Instant0;
    impl ulo::Construct for Instant0 {
        type Scope = ulo::scope::Singleton;
        const CONSTRUCT_TIMEOUT: Bound = Bound::After(ZERO);

        fn dependencies(_d: &mut ulo::Dependencies) {}

        async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
            Ok(Instant0)
        }

        fn hooks(h: &mut ulo::Hooks<Self>) {
            h.on_application_shutdown();
        }
    }
    impl ulo::OnApplicationShutdown for Instant0 {
        const TIMEOUT: Bound = Bound::After(ZERO);

        async fn on_application_shutdown(&self, _signal: &ulo::Signal) {}
    }
    let registered = line!() + 3;
    let found = refused_either_way(|| {
        Mod::<A>::new(|m| {
            m.provide::<Instant0>();
        })
    });
    let key = Some(Key::of::<Instant0, ()>());
    assert_eq!(
        found,
        [
            (BoundItem::Construction, key, mod_name::<A>(), registered),
            (BoundItem::Hook(HookKind::OnApplicationShutdown), key, mod_name::<A>(), registered),
        ]
    );
}

#[test]
fn a_bound_above_zero_and_unbounded_are_not_zero_bounds() {
    wires_with(
        App::builder(Mod::<A>::new(|m| {
            m.singleton(|| async { Svc }).timeout(Duration::from_nanos(1));
            m.singleton(|| async { A }).ready(check()).attempt_timeout(Duration::from_nanos(1));
            m.singleton(|| async { SECOND }).ready(check()).unbounded();
            m.on_init(check()).unbounded();
        }))
        .timer(Frozen::new()),
    );
}
