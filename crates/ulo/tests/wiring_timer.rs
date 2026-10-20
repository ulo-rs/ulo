//! Section 10.1 step 6, the environment: a `Timer` wherever a wait is written (section 3.9).
//!
//! Capabilities pinned: 42 (readiness bounds and backoff need a clock), 44 (reported by `wire()`
//! at the call that wrote the wait), 45. `Default` and `Unbounded` need no clock.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::{App, Bound, Dep, Key, Timer, WiringError};

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
    let line = line!() + 2;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc }).timeout(SECOND);
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 1, "{errors}");
    assert!(found[0].0.contains("Svc"), "the item names the binding: {}", found[0].0);
    assert_eq!(found[0].1, line, "the entry points at the registration");
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
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<Slow>();
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 1, "{errors}");
    assert!(found[0].0.contains("Slow"), "{}", found[0].0);
}

#[test]
fn a_readiness_timeout_and_attempt_timeout_on_an_app_with_no_timer() {
    let whole = line!() + 3;
    let attempt = line!() + 3;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc }).ready(check()).timeout(SECOND);
        m.singleton(|| async { A }).ready(check()).attempt_timeout(SECOND);
    }));
    let mut found = bounds(&errors);
    found.sort_by_key(|(_, line)| *line);
    assert_eq!(found.len(), 2, "{errors}");
    assert!(found[0].0.contains("readiness `.timeout`") && found[0].0.contains("Svc"), "{}", found[0].0);
    assert_eq!(found[0].1, whole, "the entry points at the `.ready` call");
    assert!(found[1].0.contains("readiness `.attempt_timeout`") && found[1].0.contains("A"), "{}", found[1].0);
    assert_eq!(found[1].1, attempt);
}

#[test]
fn a_hook_timeout_and_a_module_hook_timeout_on_an_app_with_no_timer() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Svc }).on_init(check()).timeout(SECOND);
        m.on_init(check()).timeout(SECOND);
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 2, "{errors}");
    assert!(found.iter().any(|(item, _)| item.contains("Svc")), "the binding's hook: {found:?}");
    assert!(found.iter().any(|(item, _)| item.contains(&mod_name::<A>())), "the module's hook: {found:?}");
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
    let errors = wire_err(Mod::<A>::new(|m| {
        m.provide::<Hooked>();
    }));
    let found = bounds(&errors);
    assert_eq!(found.len(), 1, "{errors}");
    assert!(found[0].0.contains("OnModuleInit") && found[0].0.contains("Hooked"), "{}", found[0].0);
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
