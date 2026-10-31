//! Every type `ulo-rpc`'s public signatures name from another crate that a handler or a builder
//! takes is reachable through `ulo_rpc`, and is that crate's type rather than a copy: each
//! re-export names the same type as its defining path, and each re-exported trait is the one the
//! crate's extractors implement.

use std::any::TypeId;

use ulo_rpc::Rpc;

fn same<A: 'static, B: 'static>() -> bool {
    TypeId::of::<A>() == TypeId::of::<B>()
}

fn extracts<T: ulo_rpc::FromCall<Rpc>>() {}

fn replies<T: ulo_rpc::IntoReply<Rpc>>() {}

fn classifies<T: ulo_rpc::Classify>() {}

fn validates<T: ulo_rpc::Validate>() {}

#[derive(ulo_rpc::Validate)]
struct Form {
    #[validate(length(min = 1))]
    _name: String,
}

#[test]
fn the_transport_neutral_types_are_ulo_transports() {
    assert!(same::<ulo_rpc::CallError, ulo_transport::CallError>());
    assert!(same::<ulo_rpc::ErrorKind, ulo_transport::ErrorKind>());
    assert!(same::<ulo_rpc::ExtractError, ulo_transport::ExtractError>());
    assert!(same::<ulo_rpc::IntoReplyError, ulo_transport::IntoReplyError>());
    assert!(same::<ulo_rpc::Valid<u8>, ulo_transport::Valid<u8>>());
    extracts::<ulo_rpc::Payload<u64>>();
    replies::<ulo_rpc::Reply>();
    classifies::<ulo_transport::ExtractError>();
    validates::<Form>();
}

#[test]
fn the_other_crates_types_are_theirs() {
    assert!(same::<ulo_rpc::Bytes, bytes::Bytes>());
    assert!(same::<ulo_rpc::Details, ulo_transport::Details>());
    assert!(same::<ulo_rpc::Detail, ulo_transport::Detail>());
    assert!(same::<ulo_rpc::config::Count, ulo_transport::Count>());
    assert!(same::<ulo_rpc::config::Bound, ulo::Bound>());
}

/// `Classify::details`, `CallError::with_details`, `Validate::validate` and `ExtractError::Invalid`
/// name these, so a handler's error is written with the transport crate alone.
#[test]
fn the_detail_types_are_ulo_transports() {
    assert!(same::<ulo_rpc::Details, ulo_transport::Details>());
    assert!(same::<ulo_rpc::Detail, ulo_transport::Detail>());
    assert!(same::<ulo_rpc::FieldViolation, ulo_transport::FieldViolation>());
    assert!(same::<ulo_rpc::details::Link, ulo_transport::Link>());
}
