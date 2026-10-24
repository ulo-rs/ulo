//! A controller's `.at(prefix)` that nothing reads (`UnreadPrefix`, reported at the freeze):
//! every handler belongs to a transport whose `READS_PREFIX` is `false`, or the controller mounts
//! no handler.
//!
//! Capabilities pinned: 29 (controllers declared by a module, the prefix half), 44. The wiring
//! row of section 2 for `UnreadPrefix`.

#[macro_use]
mod common;

use common::*;
use ulo::scope::Auto;
use ulo::{HandlerSpec, Key, Transport, TypeName, WiringError};

struct A;

/// A transport that joins a prefix to its routes, as HTTP does.
struct Joins;

impl Transport for Joins {
    const KEY: &'static str = "joins";
    const READS_PREFIX: bool = true;
    type Cx = ();
    type Reply = ();
}

/// A transport that does not, as RPC does.
struct Ignores;

impl Transport for Ignores {
    const KEY: &'static str = "ignores";
    type Cx = ();
    type Reply = ();
}

controller!(OnIgnores, Auto, [], |m| m.handler::<Ignores, _>(HandlerSpec::new("h", ())));
controller!(OnJoins, Auto, [], |m| m.handler::<Joins, _>(HandlerSpec::new("h", ())));
controller!(OnBoth, Auto, [], |m| {
    m.handler::<Ignores, _>(HandlerSpec::new("a", ()));
    m.handler::<Joins, _>(HandlerSpec::new("b", ()));
});
controller!(OnTwoIgnoring, Auto, [], |m| {
    m.handler::<Ignores, _>(HandlerSpec::new("a", ()));
    m.handler::<Ignores, _>(HandlerSpec::new("b", ()));
});
controller!(NoHandlers, Auto, [], |_m| {});

#[test]
fn at_prefix_on_a_controller_no_transport_reads() {
    let line = line!() + 2;
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<OnIgnores>().at("/api");
    }));
    let (controller, module, prefix, transports, at) = entry!(errors, WiringError::UnreadPrefix { controller, module, prefix, transports, at }
        => (controller.key(), module.to_string(), prefix.clone(), transports.clone(), **at));
    assert_eq!(controller, Key::of::<OnIgnores, ()>());
    assert_eq!(module, mod_name::<A>());
    assert_eq!(prefix, "/api");
    assert_eq!(transports, [TypeName::of::<Ignores>()]);
    assert_eq!(at.line(), line, "the entry points at the `.at(..)` call");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_transport_is_listed_once_however_many_handlers_it_has() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<OnTwoIgnoring>().at("/api");
    }));
    let transports = entry!(errors, WiringError::UnreadPrefix { transports, .. } => transports.clone());
    assert_eq!(transports, [TypeName::of::<Ignores>()]);
}

#[test]
fn on_a_controller_with_no_handlers() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<NoHandlers>().at("/api");
    }));
    let transports = entry!(errors, WiringError::UnreadPrefix { transports, .. } => transports.clone());
    assert!(transports.is_empty(), "{errors}");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_prefix_one_handler_reads_is_accepted_and_so_is_no_prefix() {
    wires(Mod::<A>::new(|m| {
        m.controller::<OnJoins>().at("/api");
        m.controller::<OnBoth>().at("/both");
        m.controller::<OnIgnores>();
        m.controller::<NoHandlers>();
    }));
}
