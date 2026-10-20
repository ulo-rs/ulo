//! Section 10.1 step 4, dependency cycles: a DFS over the resolved edges, the full path printed
//! with the module of each step.
//!
//! Capabilities pinned: 44 (a cycle is reported by `wire()`, printing its full path), 45 (a
//! typed entry, not a stack overflow or a hang). Cycles through `Option<Dep<_>>` and `Many<_>`
//! edges are not tested: the design leaves their status to a ruling (plan question 3).

#[macro_use]
mod common;

use common::*;
use ulo::{Dep, Key, WiringError};

struct M1;
struct M2;
struct M3;
struct Root;

struct SA;
struct SB;
struct SC;
struct SD;

/// The steps of the one `Cycle` entry as `(key, module)` text.
fn steps(errors: &ulo::WiringErrors) -> Vec<(Key, String)> {
    entry!(errors, WiringError::Cycle { path } => path.iter().map(|(k, m)| (k.key(), m.to_string())).collect::<Vec<_>>())
}

#[test]
fn a_dependency_cycle_prints_its_full_path() {
    // SA (M1) reads SB (M2) reads SC (M3) reads SA; M1 is global so M3 sees SA without an import
    // cycle between the modules.
    let root = Mod::<Root>::new(|m| {
        m.import(Mod::<M1>::new(|m| {
            m.global();
            m.import(Mod::<M2>::new(|m| {
                m.import(Mod::<M3>::new(|m| {
                    m.singleton(|_a: Dep<SA>| async { SC });
                    m.export::<SC>();
                }));
                m.singleton(|_c: Dep<SC>| async { SB });
                m.export::<SB>();
            }));
            m.singleton(|_b: Dep<SB>| async { SA });
            m.export::<SA>();
        }));
    });
    let errors = wire_err(root);
    let path = steps(&errors);

    assert_eq!(path.first(), path.last(), "the path closes on its first step: {path:?}");
    assert_eq!(path.len(), 4, "three steps and the return to the first: {path:?}");
    let module_of = |key: Key| path.iter().find(|(k, _)| *k == key).map(|(_, module)| module.clone());
    assert_eq!(module_of(Key::of::<SA, ()>()), Some(mod_name::<M1>()));
    assert_eq!(module_of(Key::of::<SB, ()>()), Some(mod_name::<M2>()));
    assert_eq!(module_of(Key::of::<SC, ()>()), Some(mod_name::<M3>()));
    // Each step reads the next: SA reads SB, SB reads SC, SC reads SA.
    for pair in path.windows(2) {
        let reads = |from: Key, to: Key| pair[0].0 == from && pair[1].0 == to;
        assert!(
            reads(Key::of::<SA, ()>(), Key::of::<SB, ()>())
                || reads(Key::of::<SB, ()>(), Key::of::<SC, ()>())
                || reads(Key::of::<SC, ()>(), Key::of::<SA, ()>()),
            "{:?} does not read {:?}",
            pair[0],
            pair[1]
        );
    }
    assert_eq!(errors.len(), 1, "{errors}");
    // The report writes the path as `X -> Y -> Z -> X` (section 10.1 step 4).
    let report = errors.to_string();
    assert!(report.contains("dependency cycle: ") && report.matches(" → ").count() == 3, "{report}");
}

#[test]
fn a_service_reading_itself_is_a_cycle() {
    let errors = wire_err(Mod::<M1>::new(|m| {
        m.singleton(|_a: Dep<SA>| async { SA });
    }));
    let path = steps(&errors);
    assert_eq!(path.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [Key::of::<SA, ()>(), Key::of::<SA, ()>()]);
    assert_eq!(path[0].1, mod_name::<M1>());
}

#[test]
fn two_separate_tangles_are_two_entries() {
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_b: Dep<SB>| async { SA });
        m.singleton(|_a: Dep<SA>| async { SB });
        m.singleton(|_d: Dep<SD>| async { SC });
        m.singleton(|_c: Dep<SC>| async { SD });
    }));
    let cycles = entries!(errors, WiringError::Cycle { path } => path.iter().map(|(k, _)| k.key()).collect::<Vec<_>>());
    assert_eq!(cycles.len(), 2, "{errors}");
    let has = |cycle: &Vec<Key>, key: Key| cycle.contains(&key);
    assert!(cycles.iter().any(|c| has(c, Key::of::<SA, ()>()) && has(c, Key::of::<SB, ()>())));
    assert!(cycles.iter().any(|c| has(c, Key::of::<SC, ()>()) && has(c, Key::of::<SD, ()>())));
}

#[test]
fn a_diamond_of_dependencies_is_not_a_cycle() {
    wires(Mod::<Root>::new(|m| {
        m.singleton(|| async { SD });
        m.singleton(|_d: Dep<SD>| async { SB });
        m.singleton(|_d: Dep<SD>| async { SC });
        m.singleton(|_b: Dep<SB>, _c: Dep<SC>| async { SA });
    }));
}
