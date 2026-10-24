//! Section 10.1 step 4, dependency cycles: a DFS over the resolved edges, the full path printed
//! with the module of each step.
//!
//! Capabilities pinned: 44 (a cycle is reported by `wire()`, printing its full path), 45 (a
//! typed entry, not a stack overflow or a hang). An `Option<Dep<_>>` edge to a bound key and a
//! contribution reading `Many` of its own key are cycles too (tenth response D2): a construction
//! reads what is bound whether or not the read may come back empty.

#[macro_use]
mod common;

use std::sync::Arc;

use common::*;
use ulo::{Dep, Key, Many, WiringError};

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

/// Asserts `path` is the closed cycle `expected` (given without its closing step), starting
/// wherever the report starts it: a rotation of `expected`, closed on its first step.
fn assert_cycle(path: &[(Key, String)], expected: &[(Key, String)]) {
    assert_eq!(path.len(), expected.len() + 1, "the path closes on its first step: {path:?}");
    assert_eq!(path.first(), path.last(), "the path closes on its first step: {path:?}");
    let start = expected.iter().position(|step| *step == path[0]).unwrap_or_else(|| panic!("{:?} is not a step of {expected:?}", path[0]));
    let rotated: Vec<(Key, String)> = expected.iter().cycle().skip(start).take(expected.len()).cloned().collect();
    assert_eq!(path[..expected.len()], rotated[..], "steps in reading order, each step the reader of the next");
}

#[test]
fn a_construction_reading_a_bound_key_through_option_is_a_cycle() {
    // SA (M1) reads `Option<Dep<SB>>` and SB (M2) is bound, so the read is an edge; SB reads SA.
    // M1 is global so M2 sees SA.
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.import(Mod::<M1>::new(|m| {
            m.global();
            m.import(Mod::<M2>::new(|m| {
                m.singleton(|_a: Dep<SA>| async { SB });
                m.export::<SB>();
            }));
            m.singleton(|_b: Option<Dep<SB>>| async { SA });
            m.export::<SA>();
        }));
    }));

    let path = steps(&errors);
    assert_cycle(&path, &[(Key::of::<SA, ()>(), mod_name::<M1>()), (Key::of::<SB, ()>(), mod_name::<M2>())]);
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_option_read_of_a_key_nothing_binds_is_no_edge_and_no_cycle() {
    // The control: with SB unbound the `Option` reads nothing, so SA and the SB reader that would
    // close the loop do not form one.
    wires(Mod::<Root>::new(|m| {
        m.singleton(|_b: Option<Dep<SB>>| async { SA });
    }));
}

trait Plugin: Send + Sync {}

struct Greedy;

impl Plugin for Greedy {}

struct Plain;

impl Plugin for Plain {}

#[test]
fn a_contribution_reading_many_of_its_own_key_is_a_cycle() {
    // The collection is its own contributions, so reading it from one of them reads that
    // contribution. The contribution sits in M2, the other one in M1: the cycle names M2.
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.import(Mod::<M1>::new(|m| {
            m.contribute::<dyn Plugin>().value(Arc::new(Plain));
        }));
        m.import(Mod::<M2>::new(|m| {
            m.contribute::<dyn Plugin>().singleton(|_all: Many<dyn Plugin>| async { Greedy }, |g| g);
        }));
    }));

    let path = steps(&errors);
    let greedy = path.first().cloned().expect("a path");
    assert_eq!(path, [greedy.clone(), greedy.clone()], "one contribution reading itself: {path:?}");
    assert_eq!(greedy.1, mod_name::<M2>(), "{path:?}");
    assert_eq!(greedy.0, Key::of::<dyn Plugin, ()>(), "a contribution is reported under the collection's key");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_service_outside_the_collection_reading_many_is_not_a_cycle() {
    wires(Mod::<Root>::new(|m| {
        m.contribute::<dyn Plugin>().value(Arc::new(Plain));
        m.singleton(|_all: Many<dyn Plugin>| async { SA });
    }));
}
