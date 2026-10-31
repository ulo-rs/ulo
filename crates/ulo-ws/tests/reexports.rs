//! Every type `ulo-ws`'s public signatures name from another crate that a handler or a builder
//! takes is reachable through `ulo_ws`, and is that crate's type rather than a copy: each
//! re-export names the same type as its defining path, and each re-exported trait is the one the
//! crate's extractors implement.

use std::any::TypeId;

use ulo_ws::Ws;

fn same<A: 'static, B: 'static>() -> bool {
    TypeId::of::<A>() == TypeId::of::<B>()
}

fn extracts<T: ulo_ws::FromCall<Ws>>() {}

fn replies<T: ulo_ws::IntoReply<Ws>>() {}

fn classifies<T: ulo_ws::Classify>() {}

fn validates<T: ulo_ws::Validate>() {}

#[derive(ulo_ws::Validate)]
struct Form {
    #[validate(length(min = 1))]
    _name: String,
}

#[test]
fn the_transport_neutral_types_are_ulo_transports() {
    assert!(same::<ulo_ws::CallError, ulo_transport::CallError>());
    assert!(same::<ulo_ws::ErrorKind, ulo_transport::ErrorKind>());
    assert!(same::<ulo_ws::ExtractError, ulo_transport::ExtractError>());
    assert!(same::<ulo_ws::IntoReplyError, ulo_transport::IntoReplyError>());
    assert!(same::<ulo_ws::Valid<u8>, ulo_transport::Valid<u8>>());
    extracts::<ulo_ws::Payload<u64>>();
    replies::<ulo_ws::Reply>();
    classifies::<ulo_transport::ExtractError>();
    validates::<Form>();
}

#[test]
fn the_other_crates_types_are_theirs() {
    assert!(same::<ulo_ws::Bytes, bytes::Bytes>());
    assert!(same::<ulo_ws::http::Uri, http::Uri>());
    assert!(same::<ulo_ws::http::HeaderMap, http::HeaderMap>());
    assert!(same::<ulo_ws::config::Count, ulo_transport::Count>());
    assert!(same::<ulo_ws::config::Bound, ulo::Bound>());
}

/// `Classify::details`, `CallError::with_details`, `Validate::validate` and `ExtractError::Invalid`
/// name these, so a handler's error is written with the transport crate alone.
#[test]
fn the_detail_types_are_ulo_transports() {
    assert!(same::<ulo_ws::Details, ulo_transport::Details>());
    assert!(same::<ulo_ws::Detail, ulo_transport::Detail>());
    assert!(same::<ulo_ws::FieldViolation, ulo_transport::FieldViolation>());
}
