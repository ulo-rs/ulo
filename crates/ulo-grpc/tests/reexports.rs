//! Every type `ulo-grpc`'s public signatures name from another crate that a handler or a builder
//! takes is reachable through `ulo_grpc`, and is that crate's type rather than a copy: each
//! re-export names the same type as its defining path, and each re-exported trait is the one the
//! crate's extractors implement.

use std::any::TypeId;

use ulo_grpc::Grpc;

fn same<A: 'static, B: 'static>() -> bool {
    TypeId::of::<A>() == TypeId::of::<B>()
}

fn extracts<T: ulo_grpc::FromCall<Grpc>>() {}

fn classifies<T: ulo_grpc::Classify>() {}

fn validates<T: ulo_grpc::Validate>() {}

#[derive(ulo_grpc::Validate)]
struct Form {
    #[validate(length(min = 1))]
    _name: String,
}

#[test]
fn the_transport_neutral_types_are_ulo_transports() {
    assert!(same::<ulo_grpc::CallError, ulo_transport::CallError>());
    assert!(same::<ulo_grpc::ErrorKind, ulo_transport::ErrorKind>());
    assert!(same::<ulo_grpc::ExtractError, ulo_transport::ExtractError>());
    assert!(same::<ulo_grpc::IntoReplyError, ulo_transport::IntoReplyError>());
    assert!(same::<ulo_grpc::Valid<u8>, ulo_transport::Valid<u8>>());
    extracts::<ulo_grpc::GrpcMetadata>();
    classifies::<ulo_transport::ExtractError>();
    validates::<Form>();
}

#[test]
fn the_other_crates_types_are_theirs() {
    assert!(same::<ulo_grpc::http::StatusCode, http::StatusCode>());
    assert!(same::<ulo_grpc::config::Count, ulo_transport::Count>());
    assert!(same::<ulo_grpc::config::Bound, ulo::Bound>());
    assert!(same::<ulo_grpc::config::Tls, ulo_net::Tls>());
    assert!(same::<ulo_grpc::config::Endpoint, ulo_net::Endpoint>());
    assert!(same::<ulo_grpc::config::EndpointSpec, ulo_net::EndpointSpec>());
}

/// `Classify::details`, `CallError::with_details`, `Validate::validate` and `ExtractError::Invalid`
/// name these, so a handler's error is written with the transport crate alone.
#[test]
fn the_detail_types_are_ulo_transports() {
    assert!(same::<ulo_grpc::Details, ulo_transport::Details>());
    assert!(same::<ulo_grpc::Detail, ulo_transport::Detail>());
    assert!(same::<ulo_grpc::FieldViolation, ulo_transport::FieldViolation>());
    assert!(same::<ulo_grpc::details::Link, ulo_transport::Link>());
}
