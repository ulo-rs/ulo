//! Attribute matching by last path segment, and the one qualifier a marker may carry.
//!
//! A marker is read by its last segment, so `#[guards]`, `#[ulo::guards]` and `#[wrong::guards]`
//! are all recognised as the marker `guards`. Its qualifier is then checked: the marker is written
//! bare or under `ulo`, the crate every generated path names (`::ulo`, DESIGN decision 27), with
//! or without a leading `::`. Any other qualifier is a compile error at the attribute, since a
//! macro cannot resolve a path and a crate depending on `ulo` under another name aliases it,
//! `extern crate framework as ulo;`, for the expansion to compile at all.
//!
//! The markers are `guards`, `interceptors`, `error_handlers` and `meta`, which `#[handlers]`
//! reads, and `construct`, which `#[injectable]` and `#[controller]` read on a constructor and as a
//! field's `construct(default)`. Rust resolves the attributes `#[controller]`,
//! `#[handlers]` and `#[injectable]` themselves, so a wrong path to one fails to resolve and any
//! path that reaches one is accepted; none of them checks its own.

use syn::{Attribute, Path};

pub(crate) fn is(attr: &Attribute, name: &str) -> bool {
    attr.path().segments.last().is_some_and(|s| s.ident == name)
}

/// Removes and returns every attribute named `name`, in order.
pub(crate) fn take_all(attrs: &mut Vec<Attribute>, name: &str) -> Vec<Attribute> {
    let mut taken = Vec::new();
    attrs.retain(|a| {
        if is(a, name) {
            taken.push(a.clone());
            false
        } else {
            true
        }
    });
    taken
}

/// Attributes that never make a method a handler: the language's own, the ulo enhancer markers
/// and `#[meta]`, which `#[handlers]` consumes, and `#[construct]`, which `#[controller]` consumes
/// when it stacks on the same impl. A method carrying `#[meta]` alone is not a handler. A marker
/// under a wrong qualifier is inert too, so it is reported for its qualifier rather than taken for
/// a transport attribute.
pub(crate) fn is_inert(attr: &Attribute) -> bool {
    const INERT: &[&str] = &[
        "doc", "allow", "warn", "deny", "expect", "cfg", "cfg_attr", "inline", "must_use", "deprecated",
        "track_caller", "guards", "interceptors", "error_handlers", "meta", "construct",
    ];
    INERT.iter().any(|name| is(attr, name))
}

/// `Ok` for a marker path written bare, `guards`, or as `ulo::guards` or `::ulo::guards`; an
/// error spanned at the path for any other qualifier.
pub(crate) fn check_marker(path: &Path) -> syn::Result<()> {
    refusal(path).map_or(Ok(()), |message| Err(syn::Error::new_spanned(path, message)))
}

/// The `ulo` segment of a marker written `ulo::name`, for the editor import; `None` when it is
/// written bare.
pub(crate) fn qualifier(path: &Path) -> Option<syn::Ident> {
    (path.segments.len() == 2).then(|| path.segments[0].ident.clone())
}

/// The message refusing `path`, `None` when it is bare or under `ulo`.
fn refusal(path: &Path) -> Option<String> {
    let segments: Vec<_> = path.segments.iter().collect();
    let accepted = match segments.as_slice() {
        [_] => path.leading_colon.is_none(),
        [first, _] => first.ident == "ulo" && first.arguments.is_none(),
        _ => false,
    };
    if accepted {
        return None;
    }
    let name = segments.last().map(|segment| segment.ident.to_string()).unwrap_or_default();
    Some(format!(
        "write the marker bare, or as `ulo::{name}`; under a renamed dependency, alias it at the crate root, \
         `extern crate <name> as ulo;`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(text: &str) -> Result<(), String> {
        check_marker(&syn::parse_str::<Path>(text).unwrap()).map_err(|e| e.to_string())
    }

    #[test]
    fn a_bare_marker_and_one_under_ulo_are_accepted() {
        for text in ["guards", "ulo::guards", "::ulo::guards"] {
            assert_eq!(checked(text), Ok(()), "{text}");
        }
    }

    #[test]
    fn any_other_qualifier_is_refused_naming_the_two_forms() {
        for text in ["wrong::guards", "framework::guards", "a::b::guards", "ulo::x::guards", "::guards", "crate::guards"] {
            let error = checked(text).expect_err(text);
            assert!(error.starts_with("write the marker bare, or as `ulo::guards`"), "{text}: {error}");
        }
    }
}
