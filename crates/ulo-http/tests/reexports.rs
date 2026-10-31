//! Every type `ulo-http`'s public signatures name from another crate is reachable through
//! `ulo_http`, and is that crate's type rather than a copy: each re-export names the same type as
//! its defining path, and each re-exported trait is the one the crate's extractors and replies
//! implement.

use std::any::TypeId;

use ulo_http::{Http, Path, Response};

fn same<A: 'static, B: 'static>() -> bool {
    TypeId::of::<A>() == TypeId::of::<B>()
}

fn extracts<T: ulo_http::FromCall<Http>>() {}

fn replies<T: ulo_http::IntoReply<Http>>() {}

fn validates<T: ulo_http::Validate>() {}

fn classifies<T: ulo_http::Classify>() {}

#[derive(ulo_http::Validate)]
struct Form {
    #[validate(length(min = 1))]
    _name: String,
}

#[test]
fn the_transport_neutral_types_are_ulo_transports() {
    assert!(same::<ulo_http::CallError, ulo_transport::CallError>());
    assert!(same::<ulo_http::ErrorKind, ulo_transport::ErrorKind>());
    assert!(same::<ulo_http::ExtractError, ulo_transport::ExtractError>());
    assert!(same::<ulo_http::IntoReplyError, ulo_transport::IntoReplyError>());
    assert!(same::<ulo_http::Valid<Path<u64>>, ulo_transport::Valid<Path<u64>>>());
    extracts::<Path<u64>>();
    replies::<Response>();
    validates::<Path<Form>>();
    classifies::<ulo_transport::ExtractError>();
}

#[test]
fn the_header_crates_are_reachable_as_modules() {
    assert!(same::<ulo_http::headers::UserAgent, headers::UserAgent>());
    assert!(same::<ulo_http::http::Uri, http::Uri>());
    assert!(same::<ulo_http::http::request::Parts, http::request::Parts>());
}

#[test]
fn the_multipart_types_are_multers() {
    assert!(same::<ulo_http::multipart::Field<'static>, multer::Field<'static>>());
    assert!(same::<ulo_http::multipart::Error, multer::Error>());
}

/// `Classify::details`, `CallError::with_details`, `Validate::validate` and `ExtractError::Invalid`
/// name these, so a handler's error is written with the transport crate alone.
#[test]
fn the_detail_types_are_ulo_transports() {
    assert!(same::<ulo_http::Details, ulo_transport::Details>());
    assert!(same::<ulo_http::Detail, ulo_transport::Detail>());
    assert!(same::<ulo_http::FieldViolation, ulo_transport::FieldViolation>());
}
