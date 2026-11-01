//! Section 10.1 as a whole: `wire()` runs six steps, collects every error and stops at none,
//! reports them in step order, and builds nothing.
//!
//! Capabilities pinned: 44 (every wiring error before any instance is built or any connection
//! opened, all of them in one pass), 45 (a typed error, no panic). The per-variant tests live in
//! the `wiring_*.rs` files; this file pins what they cannot, that the variants arrive together.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::{App, Dep, Key, WiringError};

struct Root;
struct SA;
struct SB;
struct SC;
struct Absent;
struct Absent2;
struct Svc;
struct Unbound;

/// A root with a fault for steps 1, 2, 3 and 4. The factories record into `probe`; none may run.
fn faulty(probe: &Probe) -> Mod<Root> {
    let probe = probe.clone();
    Mod::new(move |m| {
        let built = probe.clone();
        m.singleton(move || {
            let built = built.clone();
            async move {
                built.mark("svc");
                Svc
            }
        });
        // Step 1.
        m.export::<Unbound>();
        // Step 2.
        m.value(1_u32);
        m.value(2_u32);
        // Step 3.
        m.singleton(|_absent: Dep<Absent>| async { SA });
        // Step 4.
        m.singleton(|_c: Dep<SC>| async { SB });
        m.singleton(|_b: Dep<SB>| async { SC });
    })
}

#[test]
fn all_errors_arrive_in_one_pass_and_nothing_is_built() {
    let probe = Probe::new();
    // Step 6: a knob with no timer.
    let errors = wire_err_with(App::builder(faulty(&probe)).hook_timeout(Duration::from_secs(1)));

    assert_eq!(errors.len(), 5, "{errors}");
    assert_eq!(variants(&errors), ["ExportNotBound", "DuplicateBinding", "Missing", "Cycle", "KnobWithoutTimer"]);
    // Each entry carries its own fields, not just its kind.
    assert_eq!(entry!(errors, WiringError::ExportNotBound { key, .. } => key.key()), Key::of::<Unbound, ()>());
    assert_eq!(entry!(errors, WiringError::DuplicateBinding { key, .. } => key.key()), Key::of::<u32, ()>());
    assert_eq!(entry!(errors, WiringError::Missing { key, .. } => key.key()), Key::of::<Absent, ()>());
    assert_eq!(entry!(errors, WiringError::Cycle { path } => path.len()), 3, "SB, SC and the return to SB");
    assert_eq!(entry!(errors, WiringError::KnobWithoutTimer { knob } => *knob), "hook_timeout");
    assert!(probe.is_empty(), "wire() built an instance: {:?}", probe.log());
}

#[test]
fn errors_are_reported_in_step_order_whatever_the_registration_order() {
    // The same faults written backwards: the cycle first, the export last.
    let errors = wire_err_with(
        App::builder(Mod::<Root>::new(|m| {
            m.singleton(|_c: Dep<SC>| async { SB });
            m.singleton(|_b: Dep<SB>| async { SC });
            m.singleton(|_absent: Dep<Absent>| async { SA });
            m.value(1_u32);
            m.value(2_u32);
            m.export::<Unbound>();
        }))
        .drain_timeout(Duration::from_secs(1)),
    );
    assert_eq!(variants(&errors), ["ExportNotBound", "DuplicateBinding", "Missing", "Cycle", "KnobWithoutTimer"]);
}

#[test]
fn unrelated_missing_bindings_do_not_hide_each_other() {
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_a: Dep<Absent>| async { SA });
        m.singleton(|_a: Dep<Absent2>| async { SB });
        m.singleton(|_a: Dep<Absent>| async { SC });
    }));
    let missing = entries!(errors, WiringError::Missing { key, .. } => key.key());
    assert_eq!(missing, [Key::of::<Absent, ()>(), Key::of::<Absent2, ()>(), Key::of::<Absent, ()>()], "one entry per injection point:\n{errors}");
    assert_eq!(errors.len(), 3);
}

#[test]
fn a_missing_piece_skips_only_its_own_edges() {
    // `SA` reads one absent key and one present one, and the present one closes a cycle through
    // `SA`; a third service reads `SA`. The absent edge drops out, the rest of the graph is still
    // checked, and nothing downstream of the gap is reported as an error of its own.
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_absent: Dep<Absent>, _b: Dep<SB>| async { SA });
        m.singleton(|_a: Dep<SA>| async { SB });
        m.singleton(|_a: Dep<SA>| async { SC });
    }));
    assert_eq!(variants(&errors), ["Missing", "Cycle"], "{errors}");
    let cycle = entry!(errors, WiringError::Cycle { path } => path.iter().map(|(k, _)| k.key()).collect::<Vec<_>>());
    assert!(cycle.contains(&Key::of::<SA, ()>()) && cycle.contains(&Key::of::<SB, ()>()), "{cycle:?}");
    assert!(!cycle.contains(&Key::of::<SC, ()>()), "a reader of the tangle is not part of it");
}

#[test]
fn a_graph_without_faults_wires_and_builds_nothing_either() {
    let probe = Probe::new();
    let built = probe.clone();
    wires(Mod::<Root>::new(move |m| {
        let built = built.clone();
        m.singleton(move || {
            let built = built.clone();
            async move {
                built.mark("svc");
                Svc
            }
        });
    }));
    assert!(probe.is_empty(), "wire() is not connect(): {:?}", probe.log());
}
