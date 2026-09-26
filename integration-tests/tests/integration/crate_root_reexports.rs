//! The crates ulo's public signatures name are reachable through `ulo`, so an application can
//! match on or hold their types without declaring the dependency itself.
//!
//! `Multipart` yields `multer::Field` and fails with `multer::Error`; `Validated` reports
//! `validator::ValidationErrors` and bounds on `Validate`; `BodyStream` yields `bytes::Bytes`.

#![allow(dead_code)]

use ulo::validator::Validate;

fn names_the_extraction_crates(
    _field: ulo::multer::Field<'static>,
    _error: ulo::multer::Error,
    _errors: ulo::validator::ValidationErrors,
    _chunk: ulo::bytes::Bytes,
) {
}

fn validates<T: Validate>(_value: &T) {}

/// Compiles only if `BodyStream` yields the re-exported `Bytes`.
async fn a_body_stream_collects_to_the_re_export(
    stream: ulo::http::extract::BodyStream,
) -> ulo::bytes::Bytes {
    stream.collect().await.unwrap()
}

/// Compiles only if `Multipart`'s field is the re-exported `multer::Field`.
fn a_multipart_field_is_the_re_export(
    field: ulo::http::extract::Field<'static>,
) -> ulo::multer::Field<'static> {
    field
}

#[test]
fn the_re_exported_bytes_type_is_usable() {
    let chunk = ulo::bytes::Bytes::from_static(b"ok");
    assert_eq!(&chunk[..], b"ok");
}
