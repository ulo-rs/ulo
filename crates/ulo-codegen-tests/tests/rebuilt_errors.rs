//! A `CallError` naming an HTTP status, a gRPC code and a close code reads each back as it was
//! set, and keeps all three however it is rebuilt: reworded, given details or a source,
//! summarised, boxed and recognised again, built afresh by an error handler from the read-backs,
//! and failed by an execution-scoped constructor behind a handler's parameter. gRPC sends its code
//! and a refused connection closes with its own, and HTTP's status reads back for the renderer. An
//! error built from a `tonic::Status` reads back the status's code and keeps it the same ways.

use tonic::Code;
use ulo::{App, ExecOptions, Execution, LookupError, Module, ModuleDef, ModuleIdentity};
use ulo_grpc::{WithGrpcCode, from_status, to_status};
use ulo_http::{StatusCode, WithHttpStatus};
use ulo_transport::{CallError, Detail, Details, ErrorKind, ExtractError};
use ulo_ws::{CloseCode, ConnectRefused, WithCloseCode};

const STATUS: StatusCode = StatusCode::PRECONDITION_FAILED;
const CODE: Code = Code::AlreadyExists;
const CLOSE: CloseCode = match CloseCode::new(4409) {
    Ok(code) => code,
    Err(_) => panic!("4409 is an application's code"),
};

/// A `Conflict` whose kind would send 409, `ABORTED` and 1008.
fn named() -> CallError {
    CallError::new(ErrorKind::Conflict, "the order exists").with_http_status(STATUS).with_grpc_code(CODE).with_close_code(CLOSE)
}

#[track_caller]
fn assert_keeps_all_three(error: CallError, how: &str) {
    assert_eq!((error.http_status(), error.grpc_code(), error.close_code()), (Some(STATUS), Some(CODE), Some(CLOSE)), "{how}: read back");
    assert_eq!(to_status(&error).code(), CODE, "{how}: the gRPC status");
    assert_eq!(ConnectRefused::from(error).close_code(), CLOSE, "{how}: the refusal's close code");
}

/// What an error handler writes to answer with an error of its own in place of the one it
/// received: a new message, the old error as the source, the transports' codes carried over.
fn rebuilt(old: CallError) -> CallError {
    let mut new = CallError::new(old.kind(), "la commande existe").with_details(old.details().clone());
    if let Some(status) = old.http_status() {
        new = new.with_http_status(status);
    }
    if let Some(code) = old.grpc_code() {
        new = new.with_grpc_code(code);
    }
    if let Some(code) = old.close_code() {
        new = new.with_close_code(code);
    }
    new.with_source(old)
}

#[test]
fn an_error_with_no_code_of_its_own_reads_back_none_and_renders_its_kind_s() {
    let error = CallError::new(ErrorKind::Conflict, "the order exists");
    assert_eq!((error.http_status(), error.grpc_code(), error.close_code()), (None, None, None));
    assert_eq!(to_status(&error).code(), Code::Aborted);
    assert_eq!(ConnectRefused::from(error).close_code(), CloseCode::POLICY_VIOLATION);
}

#[test]
fn every_rebuild_of_an_error_keeps_its_three_codes() {
    assert_keeps_all_three(named(), "as set");
    assert_keeps_all_three(named().with_message("reworded"), "with_message");
    assert_keeps_all_three(named().with_details(Details::new().with(Detail::Json(serde_json::json!({"id": 7})))), "with_details");
    assert_keeps_all_three(named().with_source(std::fmt::Error), "with_source");
    assert_keeps_all_three(named().summary(), "summary");
    assert_keeps_all_three(CallError::from_boxed(Box::new(named())), "from_boxed");

    let fresh = rebuilt(named());
    assert_eq!(fresh.message(), "la commande existe");
    assert_eq!(fresh.source_as::<CallError>().map(CallError::message), Some("the order exists"), "the original is the source");
    assert_keeps_all_three(fresh, "an error handler's own error");
}

/// A status whose code its kind, `BadRequest`, does not send: the kind's is INVALID_ARGUMENT.
#[test]
fn an_error_from_a_status_reads_back_the_status_s_code_and_keeps_it_through_every_rebuild() {
    let status = || from_status(tonic::Status::failed_precondition("the order is closed"));
    assert_eq!(status().kind(), ErrorKind::BadRequest);
    for (how, error) in [
        ("as built", status()),
        ("with_message", status().with_message("la commande est close")),
        ("with_details", status().with_details(Details::new().with(Detail::Json(serde_json::json!({"id": 7}))))),
        ("summary", status().summary()),
        ("from_boxed", CallError::from_boxed(Box::new(status()))),
        ("an error handler's own error", rebuilt(status())),
    ] {
        assert_eq!(error.grpc_code(), Some(Code::FailedPrecondition), "{how}: read back");
        assert_eq!(to_status(&error).code(), Code::FailedPrecondition, "{how}: the gRPC status");
    }
}

struct Tenant;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.try_execution(|| async { Err::<Tenant, CallError>(named()) });
    }
}

/// An execution-scoped constructor's `CallError` reaches the error handlers as its lookup error,
/// read directly or as the `ExtractError::Dependency` a handler's `Dep<Tenant>` parameter fails
/// with, redacted or not, and is recognised with its kind, message and all three codes.
#[tokio::test]
async fn a_constructor_s_error_behind_a_dependency_keeps_its_three_codes() {
    let app = App::builder(Root).wire().expect("the graph wires").connect().await.expect("the app connects");
    let exec = Execution::open(&app.handle().root(), ExecOptions::new()).expect("the app is running");
    let lookup = async || -> LookupError {
        match exec.get::<Tenant>().await {
            Ok(_) => panic!("the constructor fails"),
            Err(error) => error,
        }
    };

    let direct = CallError::from_boxed(Box::new(lookup().await));
    assert_eq!((direct.kind(), direct.message()), (ErrorKind::Conflict, "the order exists"));
    assert_keeps_all_three(direct, "a lookup error");

    let dependency = CallError::from_boxed(Box::new(ExtractError::Dependency { param: "Tenant", source: lookup().await }));
    assert_eq!((dependency.kind(), dependency.message()), (ErrorKind::Conflict, "the order exists"));
    assert!(dependency.source_as::<ExtractError>().is_some(), "the extraction failure is the source");
    assert_keeps_all_three(dependency, "a dependency parameter's failure");

    let redacted = app.handle().redact(Box::new(ExtractError::Dependency { param: "Tenant", source: lookup().await }));
    assert_keeps_all_three(CallError::from_boxed(Box::new(redacted)), "a redacted dependency parameter's failure");

    drop(exec);
    assert!(app.close(ulo::Signal::new("done")).await.is_ok(), "the app closes");
}
