//! `CallError::find` reaches the `Redacted` an `ExtractError` holds in `Malformed`, `Mismatched`
//! and `Cancelled`, which the error's `source()` does not hand out, to the walk's 32-level bound.

use std::error::Error;
use std::fmt;

use ulo::{App, BoxError, Connected, Module, ModuleDef, ModuleIdentity};
use ulo_transport::{CallError, ExtractError};

#[derive(Debug)]
struct Marker;

impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("marker")
    }
}

impl Error for Marker {}

#[derive(Debug)]
struct Other;

impl fmt::Display for Other {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("other")
    }
}

impl Error for Other {}

/// An error keeping `0` as its `source()`.
#[derive(Debug)]
struct Layer(BoxError);

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("layer")
    }
}

impl Error for Layer {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.0)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _: &mut ModuleDef<'_>) {}
}

fn app() -> App<Connected> {
    futures_executor::block_on(App::builder(Root).wire().expect("the app wires").connect()).expect("the app connects")
}

/// `layers` errors over an `ExtractError` built by `variant` around a redacted `Marker`.
fn wrapped(variant: fn(&'static str, ulo::Redacted) -> ExtractError, layers: usize) -> BoxError {
    let app = app();
    let redacted = app.handle().redact(Box::new(Marker));
    let inner: BoxError = Box::new(variant("body", redacted));
    (0..layers).fold(inner, |inner, _| Box::new(Layer(inner)) as BoxError)
}

/// Planted break: `ExtractError::held` answering `None` fails the first and the last assertions.
#[test]
fn a_redacted_inside_each_holding_variant_is_found() {
    for variant in [ExtractError::malformed, ExtractError::mismatched, ExtractError::cancelled] {
        let error = wrapped(variant, 0);
        assert!(CallError::find::<Marker>(&error).is_some());
        assert!(CallError::find::<Other>(&error).is_none(), "a type the error does not hold");
    }
}

/// The `ExtractError` takes one level as a `Redacted` does: with 30 errors over it the marker sits
/// at depth 31 and is found, with 31 it sits at depth 32 and is not.
/// Planted break: a bound of 33 fails the second assertion.
#[test]
fn the_held_redacted_is_read_to_the_depth_bound() {
    assert!(CallError::find::<Marker>(&wrapped(ExtractError::malformed, 30)).is_some());
    assert!(CallError::find::<Marker>(&wrapped(ExtractError::malformed, 31)).is_none());
}

/// A call error whose source is the extract error, as a handler's `?` builds it.
#[test]
fn the_held_redacted_is_found_under_a_call_error() {
    let app = app();
    let held = ExtractError::malformed("body", app.handle().redact(Box::new(Marker)));
    let error: BoxError = Box::new(CallError::from(held));
    assert!(CallError::find::<Marker>(&error).is_some());
}
