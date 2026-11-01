//! A handler refusing an `Accept` it has no representation for: a `BadRequest` whose source is
//! `NotAcceptable` renders 406 as a problem document naming what the handler answers, and an error
//! handler that reshapes it to another kind is rendered by that kind, the source kept.

mod o1_support;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{BoxError, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::{CallError, ErrorKind, Http, HttpCx, NotAcceptable, Response};

fn refusal() -> BoxError {
    let refused = NotAcceptable::new(["text/csv", "application/json"]);
    Box::new(CallError::new(ErrorKind::BadRequest, refused.to_string()).with_source(refused))
}

/// Reshapes every error to `Conflict`, keeping it as the source.
struct Reshape;

impl ErrorHandler<Http> for Reshape {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        let refused = call.source_as::<NotAcceptable>().cloned().expect("the refusal reaches the error handler");
        Err(Box::new(CallError::new(ErrorKind::Conflict, "reshaped").with_source(refused)))
    }
}

#[injectable]
struct Report;

#[routes]
impl Report {
    #[ulo_http::get("/report")]
    fn report(&self) -> Result<String, BoxError> {
        Err(refusal())
    }

    #[ulo_http::get("/reshaped")]
    #[error_handlers(value = Reshape)]
    fn reshaped(&self) -> Result<String, BoxError> {
        Err(refusal())
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Report>();
}

#[tokio::test]
async fn a_not_acceptable_renders_406_naming_what_the_handler_answers_until_reshaped() {
    let app = start(register).await;

    let refused = app.send(Call::get("/report").header("accept", "text/html")).await;
    assert_eq!(refused.status, StatusCode::NOT_ACCEPTABLE, "{}", refused.text());
    assert_eq!(refused.header("content-type"), Some("application/problem+json"));
    let problem = refused.json();
    assert_eq!((problem["status"].as_u64(), problem["title"].as_str()), (Some(406), Some("Not Acceptable")));
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("text/csv") && detail.contains("application/json"), "{detail}");

    let reshaped = app.send(Call::get("/reshaped").header("accept", "text/html")).await;
    assert_eq!(reshaped.status, StatusCode::CONFLICT, "a reshaped refusal renders by its kind: {}", reshaped.text());
    app.stop().await;
}
