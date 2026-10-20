//! How a wiring report names types and modules (section 10.1, the collision rule): every name is
//! cut to its last path segment, and a name that would print like a different one in the same
//! report prints with its full path, only that name.
//!
//! Capabilities pinned: 35 (module identity printed as a readable name in every error), 44 (the
//! report is one pass over every entry's names). The assertions here are on text because the rule
//! is about text.

#[macro_use]
mod common;

use std::any::type_name;

use common::*;
use ulo::{Dep, DynamicModule, Module, WiringError};

mod a {
    pub struct Config;
    pub struct Replica;
}
mod b {
    pub struct Config;
    pub struct Replica;
}
mod billing {
    pub struct Module;
}
mod users {
    pub struct Module;
}

struct Root;
struct Head;
struct Other;
struct Store;
struct Replica;
struct Primary;
struct Owner;

fn missing_in(m: &mut ulo::ModuleDef<'_>) {
    m.singleton(|_c: Dep<a::Config>| async { Head });
    m.singleton(|_c: Dep<b::Config>| async { 1_u8 });
    m.singleton(|_c: Dep<Other>| async { 2_u8 });
}

#[test]
fn colliding_types_print_in_full_only_for_those_types() {
    let errors = wire_err(Mod::<Root>::new(missing_in));
    let report = errors.to_string();

    let (a_config, b_config, other) = (type_name::<a::Config>(), type_name::<b::Config>(), type_name::<Other>());
    assert!(report.contains(&format!("missing dependency `{a_config}`")), "{report}");
    assert!(report.contains(&format!("missing dependency `{b_config}`")), "{report}");
    assert!(report.contains("missing dependency `Other`"), "{report}");
    assert!(!report.contains(other), "`Other` collides with nothing and prints short: {report}");
}

#[test]
fn a_type_prints_short_when_nothing_in_the_report_collides_with_it() {
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_c: Dep<a::Config>| async { Head });
    }));
    let report = errors.to_string();
    assert!(report.contains("missing dependency `Config`"), "{report}");
    assert!(!report.contains(type_name::<a::Config>()), "{report}");
}

#[test]
fn a_collision_is_decided_across_entries_and_leaves_the_qualifier_short() {
    // `a::Config` is missing in one entry and `b::Config @ Replica` in another.
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_c: Dep<a::Config>| async { Head });
        m.singleton(|_c: Dep<b::Config, Replica>| async { 1_u8 });
    }));
    let report = errors.to_string();
    assert!(report.contains(&format!("`{}`", type_name::<a::Config>())), "{report}");
    assert!(report.contains(&format!("`{} @ Replica`", type_name::<b::Config>())), "{report}");
}

#[test]
fn colliding_qualifiers_print_in_full_and_the_type_they_qualify_stays_short() {
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_s: Dep<Store, a::Replica>| async { Head });
        m.singleton(|_s: Dep<Store, b::Replica>| async { 1_u8 });
    }));
    let report = errors.to_string();
    assert!(report.contains(&format!("`Store @ {}`", type_name::<a::Replica>())), "{report}");
    assert!(report.contains(&format!("`Store @ {}`", type_name::<b::Replica>())), "{report}");
}

#[test]
fn a_keys_binding_kind_plays_no_part_in_a_collision() {
    // `a::Config` is a single binding in one entry and `b::Config` a collection in the other.
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_c: Dep<a::Config>| async { Head });
        m.contribute::<b::Config>().try_value(Err::<std::sync::Arc<b::Config>, _>(Boom::new("unreadable")));
    }));
    let report = errors.to_string();
    assert!(report.contains(&format!("`{}`", type_name::<a::Config>())), "{report}");
    assert!(report.contains(&format!("`{} (collection)`", type_name::<b::Config>())), "{report}");
}

#[test]
fn a_single_entry_displayed_on_its_own_applies_the_rule_to_its_own_names() {
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.singleton(|_c: Dep<a::Config>| async { Head });
        m.singleton(|_c: Dep<b::Config>| async { 1_u8 });
    }));
    // Each entry alone holds one of the two `Config`s, so alone it prints short...
    for entry in &errors {
        let alone = entry.to_string();
        assert!(alone.contains("missing dependency `Config`"), "{alone}");
    }
    // ...and in the report, where they meet, both print in full.
    assert!(errors.to_string().contains(type_name::<a::Config>()));
}

#[test]
fn modules_whose_names_print_alike_print_in_full() {
    let errors = wire_err(Mod::<Root>::new(|m| {
        m.import(Mod::<billing::Module>::new(|m| {
            m.singleton(|_c: Dep<Other>| async { Head });
        }));
        m.import(Mod::<users::Module>::new(|m| {
            m.singleton(|_c: Dep<Other>| async { 1_u8 });
        }));
    }));
    let report = errors.to_string();
    assert!(report.contains(&format!("in {}", type_name::<Mod<billing::Module>>())), "{report}");
    assert!(report.contains(&format!("in {}", type_name::<Mod<users::Module>>())), "{report}");

    let alone = wire_err(Mod::<Root>::new(|m| {
        m.import(Mod::<billing::Module>::new(|m| {
            m.singleton(|_c: Dep<Other>| async { Head });
        }));
    }));
    let report = alone.to_string();
    assert!(report.contains(&format!("in {}", mod_name::<billing::Module>())), "{report}");
    assert!(!report.contains(type_name::<Mod<billing::Module>>()), "{report}");
}

#[test]
fn a_keyed_module_prints_its_qualifier_and_a_second_configuration_its_position() {
    let twice = |m: &mut ulo::ModuleDef<'_>| {
        m.value(1_u32);
        m.value(2_u32);
    };
    let errors = wire_err(Mod::<Root>::new(move |m| {
        m.import(Mod::<Head>::new(twice).keyed::<Primary>());
        m.import(Mod::<Head>::new(twice).keyed::<Replica>());
    }));
    let modules = entries!(errors, WiringError::DuplicateBinding { module, .. } => module.to_string());
    assert_eq!(modules, [keyed_mod_name::<Head, Primary>(), keyed_mod_name::<Head, Replica>()]);

    // Two configurations of one owner with one label: the label, then `#2` from the second on.
    let errors = wire_err(Mod::<Root>::new(move |m| {
        m.import(DynamicModule::new::<Owner, u32>(1, twice).label("redis"));
        m.import(DynamicModule::new::<Owner, u32>(2, twice).label("redis"));
    }));
    let named = entries!(errors, WiringError::DuplicateBinding { module, .. } => (module.to_string(), format!("{module:#}")));
    let owner = type_name::<Owner>();
    assert_eq!(named[0], ("redis".to_owned(), format!("redis ({owner})")));
    assert_eq!(named[1], ("redis #2".to_owned(), format!("redis ({owner}) #2")));
}
