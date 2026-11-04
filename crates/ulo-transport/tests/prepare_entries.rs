//! What `PrepareError::entries` lists for a report built from `Failures`: one entry per failure,
//! carrying what each was given, and a component's own `PrepareError` passed on whole.

use std::panic::Location;

use ulo::{BoxError, PrepareEntry, PrepareError, TypeName, Unreached};
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

#[test]
fn a_links_failure_is_written_with_the_link_and_lists_it_as_the_subject() {
    let inner = PrepareError::new([TypeName::of::<Gateway>()], |_| "the gateway refused".to_owned()).with_entries(vec![
        PrepareEntry::new([TypeName::of::<Gateway>()]),
        PrepareEntry::new([]).about("own subject"),
    ]);
    let mut failures = Failures::new();
    failures.push_link_error("nats", "no io driver".into());
    failures.push_link_error("nats", Box::new(inner) as BoxError);

    let error = failures.into_error();
    let text = error.to_string();
    assert_eq!(text, "2 problems:\n  - link `nats`: no io driver\n  - link `nats`: the gateway refused", "{text}");
    let subjects: Vec<_> = error.entries().iter().map(|entry| entry.subject()).collect();
    assert_eq!(subjects, [Some("nats"), Some("nats"), Some("own subject")]);
}

#[test]
fn a_link_failure_alone_is_written_as_its_text() {
    let mut failures = Failures::new();
    failures.push_link_error("tcp", "refused".into());
    let error = failures.into_error();
    assert_eq!(error.to_string(), "link `tcp`: refused");
    assert_eq!(error.entries()[0].subject(), Some("tcp"));
}

#[test]
fn a_reason_given_to_a_failure_is_its_entrys_and_a_nested_entrys_reason_passes_on() {
    let inner = PrepareError::new([], |_| "the stage refused".to_owned())
        .with_entries(vec![PrepareEntry::new([]).scoped(["/health/*".to_owned()]).because(Unreached::HealthOnly)]);
    let mut failures = Failures::new();
    failures.push(Failure::plain("no route").scoped(["/typo".to_owned()]).because(Unreached::NoRoute));
    failures.push(Failure::plain("max_inflight is zero"));
    failures.push_error(Box::new(inner) as BoxError);

    let error = failures.into_error();
    let reasons: Vec<_> = error.entries().iter().map(PrepareEntry::reason).collect();
    assert_eq!(reasons, [Some(Unreached::NoRoute), None, Some(Unreached::HealthOnly)]);
}

#[test]
fn each_reason_writes_the_clause_its_refusal_ends_with() {
    assert_eq!(Unreached::NoRoute.to_string(), "matches no route's path");
    assert_eq!(
        Unreached::HealthOnly.to_string(),
        "matches only the gRPC health service's paths among those the server answers, and health runs no scoped entry, so the \
         entry would never run"
    );
}
