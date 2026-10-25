//! What `PrepareError::entries` lists for a report built from `Failures`: one entry per failure,
//! carrying what each was given, and a component's own `PrepareError` passed on whole.

use std::panic::Location;

use ulo::{BoxError, PrepareEntry, PrepareError, TypeName};
use ulo_transport::prepare::{Failure, Failures};

struct Gateway;

#[test]
fn a_failure_from_a_components_prepare_error_passes_its_entries_on_whole() {
    let at = Location::caller();
    let inner = PrepareError::new([TypeName::of::<Gateway>()], |_| "the gateway refused".to_owned()).with_entries(vec![
        PrepareEntry::new([TypeName::of::<Gateway>()]).at(at).about("pre-dispatch entry").scoped(["/ws".to_owned()]),
        PrepareEntry::new([]).about("upgrade path"),
    ]);
    let mut failures = Failures::new();
    failures.push(Failure::plain("max_inflight is zero").about("knob"));
    failures.push_error(Box::new(inner) as BoxError);

    let error = failures.into_error();
    let entries = error.entries();
    assert_eq!(entries.len(), 3, "{entries:?}");
    assert_eq!((entries[0].subject(), entries[0].location(), entries[0].names()), (Some("knob"), None, &[][..]));
    assert_eq!(entries[1].names(), [TypeName::of::<Gateway>()]);
    assert_eq!(entries[1].location(), Some(at));
    assert_eq!(entries[1].subject(), Some("pre-dispatch entry"));
    assert_eq!(entries[1].scopes(), ["/ws".to_owned()]);
    assert_eq!((entries[2].subject(), entries[2].scopes()), (Some("upgrade path"), &[][..]));
}
