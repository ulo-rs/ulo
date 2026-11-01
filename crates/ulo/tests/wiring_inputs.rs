//! Execution inputs (section 6.4): a non-optional input read on a path from a handler whose
//! transport does not seed it (`InputNotSeeded`, step 5) and an input with a second source
//! (`InputConflict`, step 2).
//!
//! Capabilities pinned: 12 (the per-execution-data site, its wiring half), 13 (an optional input
//! read is not refused), 44. The wiring rows of section 2 for `InputNotSeeded` and
//! `InputConflict`.

#[macro_use]
mod common;

use common::*;
use ulo::scope::PerExecution;
use ulo::{Dep, Dependencies, HandlerSpec, InputOrigin, Inputs, Key, Transport, TypeName, WiringError};

struct A;
struct Addr;

/// Seeds `Addr`, as an HTTP-like transport would.
struct Seeds;

impl Transport for Seeds {
    const KEY: &'static str = "seeds";
    type Cx = ();
    type Reply = ();

    fn inputs(d: &mut Inputs) {
        d.input::<Addr>();
    }
}

/// Seeds nothing.
struct Bare;

impl Transport for Bare {
    const KEY: &'static str = "bare";
    type Cx = ();
    type Reply = ();
}

/// A second transport declaring `Addr` itself, without naming `Seeds` as a co-seeder.
struct Rival;

impl Transport for Rival {
    const KEY: &'static str = "rival";
    type Cx = ();
    type Reply = ();

    fn inputs(d: &mut Inputs) {
        d.input::<Addr>();
    }
}

/// Two transports that declare `Addr` and name each other, which merges into one declaration.
struct Left;
struct Right;

impl Transport for Left {
    const KEY: &'static str = "left";
    type Cx = ();
    type Reply = ();

    fn inputs(d: &mut Inputs) {
        d.input::<Addr>().also_seeded_by::<Right>();
    }
}

impl Transport for Right {
    const KEY: &'static str = "right";
    type Cx = ();
    type Reply = ();

    fn inputs(d: &mut Inputs) {
        d.input::<Addr>().also_seeded_by::<Left>();
    }
}

reader!(ReadsAddr, PerExecution, addr: Dep<Addr>);
reader!(MaybeReadsAddr, PerExecution, addr: Option<Dep<Addr>>);

controller!(OnSeeds, ulo::scope::Auto, [r: Dep<ReadsAddr>], |m| m.handler::<Seeds, _>(HandlerSpec::new("h", ())));
controller!(OnSeedsPlain, ulo::scope::Auto, [], |m| m.handler::<Seeds, _>(HandlerSpec::new("h", ())));
controller!(OnBare, ulo::scope::Auto, [r: Dep<ReadsAddr>], |m| m.handler::<Bare, _>(HandlerSpec::new("h", ())));
controller!(OnBareMaybe, ulo::scope::Auto, [r: Dep<MaybeReadsAddr>], |m| m.handler::<Bare, _>(HandlerSpec::new("h", ())));
controller!(OnBareParam, ulo::scope::Auto, [], |m| {
    let mut reads = Dependencies::default();
    reads.add::<Dep<Addr>>();
    m.handler::<Bare, _>(HandlerSpec::new("h", ()).dependencies(reads));
});
controller!(OnRival, ulo::scope::Auto, [], |m| m.handler::<Rival, _>(HandlerSpec::new("h", ())));
controller!(OnLeft, ulo::scope::Auto, [], |m| m.handler::<Left, _>(HandlerSpec::new("h", ())));
controller!(OnRight, ulo::scope::Auto, [], |m| m.handler::<Right, _>(HandlerSpec::new("h", ())));

fn declares_addr(m: &mut ulo::ModuleDef<'_>) {
    m.input::<Addr>().seeded_by::<Seeds>();
}

fn not_seeded(errors: &ulo::WiringErrors) -> (String, TypeName, Key, TypeName, Vec<String>) {
    let (handler, transport, input, seeder, steps) = entry!(errors, WiringError::InputNotSeeded { handler, transport, input, seeder, steps }
        => (handler.clone(), *transport, input.key(), *seeder, steps.clone()));
    (handler, transport, input, seeder, steps)
}

#[test]
fn input_read_on_a_path_from_a_transport_that_does_not_seed_it() {
    let errors = wire_err(Mod::<A>::new(|m| {
        declares_addr(m);
        m.provide::<ReadsAddr>();
        m.controller::<OnBare>();
    }));
    let (handler, transport, input, seeder, steps) = not_seeded(&errors);
    assert_eq!(transport, TypeName::of::<Bare>());
    assert_eq!(seeder, TypeName::of::<Seeds>());
    assert_eq!(input, Key::of::<Addr, ()>());
    assert!(handler.contains("OnBare"), "the handler is named by its controller: {handler}");
    assert!(steps.len() >= 2 && steps.last().is_some_and(|s| s.contains("Dep<")), "the walk from the controller to the read: {steps:?}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_handler_parameter_reading_an_unseeded_input_is_refused_with_one_step() {
    let errors = wire_err(Mod::<A>::new(|m| {
        declares_addr(m);
        m.controller::<OnBareParam>();
    }));
    let (_, transport, input, _, steps) = not_seeded(&errors);
    assert_eq!((transport, input), (TypeName::of::<Bare>(), Key::of::<Addr, ()>()));
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_input_read_from_the_seeding_transport_or_optionally_is_accepted() {
    wires(Mod::<A>::new(|m| {
        declares_addr(m);
        m.provide::<ReadsAddr>();
        m.provide::<MaybeReadsAddr>();
        m.controller::<OnSeeds>();
    }));
    wires(Mod::<A>::new(|m| {
        declares_addr(m);
        m.provide::<MaybeReadsAddr>();
        m.controller::<OnBareMaybe>();
    }));
}

fn conflict(errors: &ulo::WiringErrors) -> (Key, InputOrigin, InputOrigin) {
    entry!(errors, WiringError::InputConflict { key, first, second } => (key.key(), first.clone(), second.clone()))
}

#[test]
fn transport_vs_transport_different_seeders() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<OnSeedsPlain>();
        m.controller::<OnRival>();
    }));
    let (key, first, second) = conflict(&errors);
    assert_eq!(key, Key::of::<Addr, ()>());
    assert!(matches!(&first, InputOrigin::Transport { name, .. } if *name == TypeName::of::<Seeds>()), "{first:?}");
    assert!(matches!(&second, InputOrigin::Transport { name, .. } if *name == TypeName::of::<Rival>()), "{second:?}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn transport_vs_module() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<OnSeedsPlain>();
        m.input::<Addr>().seeded_by::<Rival>();
    }));
    let (key, first, second) = conflict(&errors);
    assert_eq!(key, Key::of::<Addr, ()>());
    assert!(matches!(&first, InputOrigin::Transport { name, .. } if *name == TypeName::of::<Seeds>()), "{first:?}");
    match &second {
        InputOrigin::Module { module, seeders, .. } => {
            assert_eq!(module.to_string(), mod_name::<A>());
            assert_eq!(seeders, &[TypeName::of::<Rival>()]);
        }
        other => panic!("expected a module origin, found {other:?}"),
    }
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn transport_vs_binding() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<OnSeedsPlain>();
        m.value(Addr);
    }));
    let (key, first, second) = conflict(&errors);
    assert_eq!(key, Key::of::<Addr, ()>());
    assert!(matches!(&first, InputOrigin::Transport { name, .. } if *name == TypeName::of::<Seeds>()), "{first:?}");
    assert!(matches!(&second, InputOrigin::Binding { module, .. } if module.to_string() == mod_name::<A>()), "{second:?}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn two_transports_naming_each_other_as_seeders_merge() {
    wires(Mod::<A>::new(|m| {
        m.controller::<OnLeft>();
        m.controller::<OnRight>();
    }));
}

#[test]
fn a_module_declaration_the_transport_repeats_is_one_declaration() {
    wires(Mod::<A>::new(|m| {
        m.controller::<OnSeedsPlain>();
        m.input::<Addr>().seeded_by::<Seeds>();
    }));
}

#[test]
fn two_module_declarations_of_one_input_stay_a_duplicate_binding() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.input::<Addr>().seeded_by::<Seeds>();
        m.input::<Addr>().seeded_by::<Rival>();
    }));
    assert!(entries!(errors, WiringError::InputConflict { .. } => ()).is_empty(), "{errors}");
    entry!(errors, WiringError::DuplicateBinding { .. } => ());
}
