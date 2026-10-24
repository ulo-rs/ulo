//! A controller's `.at(prefix)` that nothing reads (`UnreadPrefix`, reported at the freeze):
//! every handler belongs to a transport whose `READS_PREFIX` is `false`, or the controller mounts
//! no handler. A controller's key read by a binding that is not a controller (`ControllerRead`,
//! step 3): a controller is the edge of the application.
//!
//! Capabilities pinned: 29 (controllers declared by a module, the prefix half, and no binding but
//! a controller reads one), 44. The wiring rows of section 2 for `UnreadPrefix` and
//! `ControllerRead`.

#[macro_use]
mod common;

use common::*;
use ulo::scope::Auto;
use ulo::{Dep, HandlerSpec, Key, Transport, TypeName, WiringError};

struct A;
struct X;
struct Q;
struct Reader;

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

/// Each `ControllerRead` as `(controller, consumer, module, controller's module)`.
fn controller_reads(errors: &ulo::WiringErrors) -> Vec<(Key, String, String, String)> {
    entries!(errors, WiringError::ControllerRead { controller, consumer, module, controller_module }
        => (controller.key(), consumer.clone(), module.to_string(), controller_module.to_string()))
}

#[test]
fn a_provider_reading_a_controller_in_its_own_module_is_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<NoHandlers>();
        m.singleton(|_c: Dep<NoHandlers>| async { Reader });
    }));
    let reads = controller_reads(&errors);
    assert_eq!(reads.len(), 1, "{errors}");
    let (controller, consumer, module, controller_module) = &reads[0];
    assert_eq!(*controller, Key::of::<NoHandlers, ()>());
    assert!(consumer.starts_with("Reader"), "the reader is named: {consumer}");
    assert_eq!((module.as_str(), controller_module.as_str()), (mod_name::<A>().as_str(), mod_name::<A>().as_str()));
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn an_optional_read_of_an_exported_controller_from_another_module_is_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.import(Mod::<X>::new(|m| {
            m.controller::<NoHandlers>();
            m.export::<NoHandlers>();
        }));
        m.singleton(|_c: Option<Dep<NoHandlers>>| async { Reader });
    }));
    let reads = controller_reads(&errors);
    assert_eq!(reads.len(), 1, "{errors}");
    let (controller, consumer, module, controller_module) = &reads[0];
    assert_eq!(*controller, Key::of::<NoHandlers, ()>());
    assert!(consumer.starts_with("Reader"), "{consumer}");
    assert_eq!(module, &mod_name::<A>(), "the reader's module");
    assert_eq!(controller_module, &mod_name::<X>(), "the controller's module");
    assert_eq!(errors.len(), 1, "{errors}");
}

#[test]
fn a_readiness_check_a_hook_and_an_alias_reading_a_controller_are_refused() {
    let errors = wire_err(Mod::<A>::new(|m| {
        m.controller::<NoHandlers>();
        m.singleton(|| async { Reader })
            .ready(|_c: Dep<NoHandlers>| async { Ok::<(), Boom>(()) })
            .on_destroy(|_c: Dep<NoHandlers>| async {});
        m.alias::<NoHandlers, Q>().of::<()>();
    }));
    let reads = controller_reads(&errors);
    assert_eq!(reads.len(), 3, "{errors}");
    for (controller, _, module, controller_module) in &reads {
        assert_eq!(*controller, Key::of::<NoHandlers, ()>());
        assert_eq!((module, controller_module), (&mod_name::<A>(), &mod_name::<A>()));
    }
    let mut readers: Vec<&str> = reads.iter().map(|(_, consumer, _, _)| consumer.as_str()).collect();
    readers.sort_unstable();
    assert!(readers[0].starts_with("OnModuleDestroy hook of Reader"), "{readers:?}");
    assert!(readers[1].starts_with("readiness check of Reader"), "{readers:?}");
    assert!(readers[2].starts_with("the alias `NoHandlers @ Q`"), "{readers:?}");
    assert_eq!(errors.len(), 3, "{errors}");
}

#[test]
fn a_controller_reading_another_controller_wires() {
    controller!(ReadsController, Auto, [c: Dep<NoHandlers>], |_m| {});
    wires(Mod::<A>::new(|m| {
        m.controller::<NoHandlers>();
        m.controller::<ReadsController>();
    }));
}
