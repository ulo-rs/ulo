//! An error's HTTP status is HTTP's alone: `to_status` answers a `CallError` that names a status
//! through `ulo_http::WithHttpStatus` with its kind's code and its message, as it answers the same
//! error without the status.

use tonic::Code;
use ulo_grpc::{CallError, ErrorKind, to_status};
use ulo_http::{StatusCode, WithHttpStatus};

#[test]
fn an_error_s_http_status_changes_no_grpc_status() {
    let error = CallError::new(ErrorKind::Conflict, "the pot is taken");
    let plain = to_status(&error);
    let with_status = to_status(&error.with_http_status(StatusCode::IM_A_TEAPOT));
    assert_eq!((with_status.code(), with_status.message()), (Code::Aborted, "the pot is taken"));
    assert_eq!((with_status.code(), with_status.message()), (plain.code(), plain.message()));
}
