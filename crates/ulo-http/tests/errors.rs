//! The HTTP error model: every `ErrorKind` renders its status as a problem document, an error's
//! own status from `WithHttpStatus` replaces its kind's, a derived `Classify` maps each variant to
//! its kind, details travel in the document and in `Retry-After`, an error nothing classified is
//! withheld, a panic is a 500, and the error chain lets a handler claim, pass on or reshape an
//! error, reword it keeping everything else, or answer it with the status it would have had.

mod o1_support;

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{AnyErrorHandler, BoxError, Dep, ErrorHandler, ModuleDef, controller, handlers};
use ulo_http::{Http, HttpBody, HttpCx, Path, Response, WithHttpStatus, status_for};
use ulo_transport::{CallError, Classify, Detail, Details, ErrorKind};

/// A domain error whose variants are classified by the derive.
#[derive(Debug, Classify)]
enum Shop {
    #[classify(not_found)]
    Gone,
    #[classify(conflict)]
    Taken,
    #[classify(unavailable)]
    Closed,
}

impl fmt::Display for Shop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Shop::Gone => "gone",
            Shop::Taken => "taken",
            Shop::Closed => "closed",
        })
    }
}

impl Error for Shop {}

/// A hand-written classification carrying details and a public message of its own.
#[derive(Debug)]
struct Throttled;

impl fmt::Display for Throttled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("internal bookkeeping text")
    }
}

impl Error for Throttled {}

impl Classify for Throttled {
    fn classify(&self) -> ErrorKind {
        ErrorKind::TooManyRequests
    }

    fn public_message(&self) -> Cow<'_, str> {
        Cow::Borrowed("slow down")
    }

    fn details(&self) -> Details {
        Details::new().with(Detail::RetryAfter(Duration::from_millis(1500)))
    }
}

/// An error with no classification.
#[derive(Debug)]
struct Boom;

impl fmt::Display for Boom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the secret reason")
    }
}

impl Error for Boom {}

fn kind_named(name: &str) -> ErrorKind {
    match name {
        "bad_request" => ErrorKind::BadRequest,
        "unauthorized" => ErrorKind::Unauthorized,
        "forbidden" => ErrorKind::Forbidden,
        "not_found" => ErrorKind::NotFound,
        "conflict" => ErrorKind::Conflict,
        "unprocessable" => ErrorKind::Unprocessable,
        "too_many_requests" => ErrorKind::TooManyRequests,
        "timeout" => ErrorKind::Timeout,
        "unavailable" => ErrorKind::Unavailable,
        "unimplemented" => ErrorKind::Unimplemented,
        "cancelled" => ErrorKind::Cancelled,
        _ => ErrorKind::Internal,
    }
}

/// Built per call by a constructor that fails with a status of its own.
struct Tenant;

#[controller]
struct Faults;

#[handlers]
impl Faults {
    #[ulo_http::get("/kind/{name}")]
    fn kind(&self, Path(name): Path<String>) -> Result<&'static str, CallError> {
        Err(CallError::new(kind_named(&name), "the message"))
    }

    /// A `Conflict` sent with the status the path names.
    #[ulo_http::get("/status/{code}")]
    fn status(&self, Path(code): Path<u16>) -> Result<&'static str, CallError> {
        let status = StatusCode::from_u16(code).expect("the test names a valid status");
        Err(CallError::new(ErrorKind::Conflict, "the version changed").with_http_status(status))
    }

    /// A `Throttled`, whose details carry `RetryAfter`, sent as a 503.
    #[ulo_http::get("/status-unavailable")]
    fn status_unavailable(&self) -> Result<&'static str, CallError> {
        Err(CallError::from(Throttled).with_http_status(StatusCode::SERVICE_UNAVAILABLE))
    }

    /// Fails as its `Tenant` parameter's constructor fails, a `Conflict` sent as a 412.
    #[ulo_http::get("/tenant")]
    fn tenant(&self, _tenant: Dep<Tenant>) -> &'static str {
        "built"
    }

    #[ulo_http::get("/challenge")]
    fn challenge(&self) -> Result<&'static str, CallError> {
        Err(CallError::unauthorized("Basic realm=\"shop\""))
    }

    #[ulo_http::get("/shop/{what}")]
    fn shop(&self, Path(what): Path<String>) -> Result<&'static str, Shop> {
        Err(match what.as_str() {
            "gone" => Shop::Gone,
            "taken" => Shop::Taken,
            _ => Shop::Closed,
        })
    }

    #[ulo_http::get("/throttled")]
    fn throttled(&self) -> Result<&'static str, Throttled> {
        Err(Throttled)
    }

    #[ulo_http::get("/unclassified")]
    fn unclassified(&self) -> Result<&'static str, Boom> {
        Err(Boom)
    }

    #[ulo_http::get("/panic")]
    fn panics(&self) -> &'static str {
        panic!("the secret panic text")
    }

    #[ulo_http::get("/method-claims")]
    #[error_handlers(value = MethodClaim)]
    fn method_claims(&self) -> Result<&'static str, Shop> {
        Err(Shop::Taken)
    }

    #[ulo_http::get("/passes-on")]
    #[error_handlers(value = PassOn)]
    fn passes_on(&self) -> Result<&'static str, Shop> {
        Err(Shop::Taken)
    }

    #[ulo_http::get("/reshaped")]
    #[error_handlers(value = Reshape)]
    fn reshaped(&self) -> Result<&'static str, Shop> {
        Err(Shop::Taken)
    }

    /// A `Throttled` sent as a 503, reworded by its error handler.
    #[ulo_http::get("/reworded")]
    #[error_handlers(value = Reword)]
    fn reworded(&self) -> Result<&'static str, CallError> {
        Err(CallError::from(Throttled).with_http_status(StatusCode::SERVICE_UNAVAILABLE))
    }

    /// An unauthorized error whose challenge the rewording keeps.
    #[ulo_http::get("/reworded-challenge")]
    #[error_handlers(value = Reword)]
    fn reworded_challenge(&self) -> Result<&'static str, CallError> {
        Err(CallError::unauthorized("Basic realm=\"shop\""))
    }

    /// A `Conflict` sent as a 412, or with no status of its own for `0`, answered by a handler
    /// writing its own body with the status the error would have had.
    #[ulo_http::get("/own-body/{code}")]
    #[error_handlers(value = OwnBody)]
    fn own_body(&self, Path(code): Path<u16>) -> Result<&'static str, CallError> {
        let error = CallError::new(ErrorKind::Conflict, "the version changed");
        Err(match code {
            0 => error,
            code => error.with_http_status(StatusCode::from_u16(code).expect("the test names a valid status")),
        })
    }
}

/// Rewords the error's public message, then names the source the reworded error still reaches.
struct Reword;

impl ErrorHandler<Http> for Reword {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let reworded = CallError::from_boxed(err).with_message("reworded");
        let source = if reworded.source_as::<Throttled>().is_some() { "throttled" } else { "none" };
        let text = format!("{}, source {source}", reworded.message());
        Err(Box::new(reworded.with_message(text)))
    }
}

/// Answers with a body of its own and the status the error renders with.
struct OwnBody;

impl ErrorHandler<Http> for OwnBody {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        let status = call.http_status().unwrap_or_else(|| status_for(call.kind()));
        let mut response = Response::new(HttpBody::from(format!("own body {}", status.as_u16())));
        *response.status_mut() = status;
        Ok(response)
    }
}

/// Claims every error as 200 `method`.
struct MethodClaim;

impl ErrorHandler<Http> for MethodClaim {
    async fn handle(&self, _err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        Ok(Response::new(HttpBody::from("method")))
    }
}

/// Declines: hands the error on as it came.
struct PassOn;

impl ErrorHandler<Http> for PassOn {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        Err(err)
    }
}

/// Replaces the error with a `Forbidden` one.
struct Reshape;

impl ErrorHandler<Http> for Reshape {
    async fn handle(&self, _err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        Err(Box::new(CallError::new(ErrorKind::Forbidden, "reshaped")))
    }
}

/// The global handler: claims only the kinds it was written for, as 200 `global:<kind>`.
struct GlobalClaim;

impl ErrorHandler<Http> for GlobalClaim {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        if call.kind() == ErrorKind::Conflict || call.kind() == ErrorKind::Forbidden {
            return Ok(Response::new(HttpBody::from(format!("global:{}", call.kind()))));
        }
        Err(Box::new(call))
    }
}

fn plain(m: &mut ModuleDef<'_>) {
    m.controller::<Faults>();
    m.try_execution(|| async {
        Err::<Tenant, CallError>(CallError::new(ErrorKind::Conflict, "the tenant moved").with_http_status(StatusCode::PRECONDITION_FAILED))
    });
}

fn chained(m: &mut ModuleDef<'_>) {
    plain(m);
    let claim: Arc<AnyErrorHandler<Http>> = Arc::new(GlobalClaim);
    m.enhancer::<AnyErrorHandler<Http>>().value(claim);
}

#[tokio::test]
async fn every_error_kind_renders_its_status_as_a_problem_document_of_that_status() {
    let app = start(plain).await;
    // The title is RFC 9110's phrase for the status, as RFC 9457 has an `about:blank` problem's.
    let table = [
        ("bad_request", StatusCode::BAD_REQUEST, "Bad Request"),
        ("unauthorized", StatusCode::UNAUTHORIZED, "Unauthorized"),
        ("forbidden", StatusCode::FORBIDDEN, "Forbidden"),
        ("not_found", StatusCode::NOT_FOUND, "Not Found"),
        ("conflict", StatusCode::CONFLICT, "Conflict"),
        ("unprocessable", StatusCode::UNPROCESSABLE_ENTITY, "Unprocessable Content"),
        ("too_many_requests", StatusCode::TOO_MANY_REQUESTS, "Too Many Requests"),
        ("timeout", StatusCode::GATEWAY_TIMEOUT, "Gateway Timeout"),
        ("unavailable", StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable"),
        ("unimplemented", StatusCode::NOT_IMPLEMENTED, "Not Implemented"),
        ("internal", StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"),
    ];

    for (name, status, title) in table {
        assert_eq!(kind_named(name).as_str(), name, "the table names the kind it claims to");
        assert_eq!(status_for(kind_named(name)), status, "{name}: `status_for` is the table the renderer reads");
        let reply = app.send(Call::get(&format!("/kind/{name}"))).await;
        assert_eq!(reply.status, status, "{name}");
        assert_eq!(reply.header("content-type"), Some("application/problem+json"), "{name}");
        let problem = reply.json();
        assert_eq!((&problem["type"], &problem["title"]), (&serde_json::json!("about:blank"), &serde_json::json!(title)), "{name}");
        assert_eq!(problem["status"], status.as_u16(), "{name}: the document repeats the status");
        assert_eq!(problem["detail"], "the message", "{name}");
    }

    // 499 has no registered phrase, so its document carries no `title`.
    let cancelled = StatusCode::from_u16(499).expect("499 is a status code");
    assert_eq!(status_for(ErrorKind::Cancelled), cancelled, "`status_for` is the table the renderer reads");
    let reply = app.send(Call::get("/kind/cancelled")).await;
    assert_eq!(reply.status, cancelled);
    let problem = reply.json();
    assert_eq!((&problem["type"], problem.get("title")), (&serde_json::json!("about:blank"), None));
    assert_eq!((&problem["status"], &problem["detail"]), (&serde_json::json!(499), &serde_json::json!("the message")));
    app.stop().await;
}

/// The status `WithHttpStatus` names replaces the kind's in the response and in the problem
/// document, and the headers follow the status: a 401 challenges and a 503 carries `Retry-After`.
/// A status outside 400 to 599 is ignored and the kind's 409 sent.
#[tokio::test]
async fn an_error_s_own_status_replaces_its_kind_s_and_the_headers_follow_the_status() {
    let app = start(plain).await;
    for (code, title) in [(412, "Precondition Failed"), (418, "I'm a teapot"), (507, "Insufficient Storage")] {
        let reply = app.send(Call::get(&format!("/status/{code}"))).await;
        assert_eq!(reply.status.as_u16(), code, "with_http_status({code})");
        let problem = reply.json();
        assert_eq!((&problem["status"], &problem["title"]), (&serde_json::json!(code), &serde_json::json!(title)), "{code}");
        assert_eq!(problem["detail"], "the version changed", "{code}");
    }
    let challenged = app.send(Call::get("/status/401")).await;
    assert_eq!((challenged.status, challenged.header("www-authenticate")), (StatusCode::UNAUTHORIZED, Some("Bearer")));
    let unavailable = app.send(Call::get("/status-unavailable")).await;
    assert_eq!((unavailable.status, unavailable.header("retry-after")), (StatusCode::SERVICE_UNAVAILABLE, Some("2")));
    for code in [200, 302] {
        let reply = app.send(Call::get(&format!("/status/{code}"))).await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "with_http_status({code}) is not an error's status");
    }
    app.stop().await;
}

/// A constructor's error behind a handler's `Dep` parameter is sent with the status it names, as
/// the same error returned by the handler is.
#[tokio::test]
async fn a_dependency_s_constructor_error_is_sent_with_its_own_status() {
    let app = start(plain).await;
    let reply = app.send(Call::get("/tenant")).await;
    assert_eq!(reply.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(reply.json()["detail"], "the tenant moved");
    app.stop().await;
}

#[tokio::test]
async fn an_unauthorized_answer_challenges_with_the_errors_own_scheme_or_the_servers_default() {
    let app = start(plain).await;

    let own = app.send(Call::get("/challenge")).await;
    assert_eq!((own.status, own.header("www-authenticate")), (StatusCode::UNAUTHORIZED, Some("Basic realm=\"shop\"")));
    let default = app.send(Call::get("/kind/unauthorized")).await;
    assert_eq!((default.status, default.header("www-authenticate")), (StatusCode::UNAUTHORIZED, Some("Bearer")));
    let other = app.send(Call::get("/kind/forbidden")).await;
    assert_eq!(other.header("www-authenticate"), None, "only a 401 challenges");
    app.stop().await;
}

#[tokio::test]
async fn a_derived_classification_maps_each_variant_to_its_own_kind() {
    let app = start(plain).await;

    for (what, status) in [("gone", StatusCode::NOT_FOUND), ("taken", StatusCode::CONFLICT), ("closed", StatusCode::SERVICE_UNAVAILABLE)] {
        let reply = app.send(Call::get(&format!("/shop/{what}"))).await;
        assert_eq!(reply.status, status, "{what}");
        assert_eq!(reply.json()["detail"], what, "the derive's default public message is the error's text");
    }
    app.stop().await;
}

#[tokio::test]
async fn details_and_a_public_message_travel_and_retry_after_rounds_up_to_whole_seconds() {
    let app = start(plain).await;

    let reply = app.send(Call::get("/throttled")).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reply.header("retry-after"), Some("2"));
    let problem = reply.json();
    assert_eq!(problem["detail"], "slow down", "the public message, not the error's text");
    assert_eq!(problem["details"][0]["type"], "retry_after");
    assert_eq!(problem["details"][0]["seconds"], 2);
    app.stop().await;
}

#[tokio::test]
async fn an_error_nothing_classified_is_a_500_that_withholds_its_text() {
    let app = start(plain).await;

    let reply = app.send(Call::get("/unclassified")).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!reply.text().contains("secret"), "{}", reply.text());
    app.stop().await;
}

#[tokio::test]
async fn a_panic_in_a_handler_is_a_500_that_withholds_the_message_and_the_app_keeps_serving() {
    let app = start(plain).await;

    let reply = app.send(Call::get("/panic")).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!reply.text().contains("secret"), "{}", reply.text());
    let after = app.send(Call::get("/kind/conflict")).await;
    assert_eq!(after.status, StatusCode::CONFLICT, "the next request is served");
    app.stop().await;
}

#[tokio::test]
async fn the_most_specific_handler_that_claims_an_error_wins_and_the_global_one_never_sees_it() {
    let app = start(chained).await;

    let reply = app.send(Call::get("/method-claims")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "method".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn a_handler_that_declines_hands_the_same_error_to_the_next_tier() {
    let app = start(chained).await;

    let reply = app.send(Call::get("/passes-on")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, format!("global:{}", ErrorKind::Conflict)));
    app.stop().await;
}

#[tokio::test]
async fn a_handler_that_reshapes_the_error_hands_the_new_one_to_the_next_tier() {
    let app = start(chained).await;

    let reply = app.send(Call::get("/reshaped")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, format!("global:{}", ErrorKind::Forbidden)));
    app.stop().await;
}

#[tokio::test]
async fn an_error_no_handler_claims_falls_through_to_its_own_rendering() {
    let app = start(chained).await;

    // `Gone` is `not_found`; the global handler only claims conflict and forbidden.
    let reply = app.send(Call::get("/shop/gone")).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.header("content-type"), Some("application/problem+json"));
    app.stop().await;
}

/// `with_message` changes the text alone: the status override, the details with the
/// `Retry-After` they produce, the challenge and the source all survive it.
#[tokio::test]
async fn a_reworded_error_keeps_its_status_details_challenge_and_source() {
    let app = start(plain).await;

    let reply = app.send(Call::get("/reworded")).await;
    assert_eq!((reply.status, reply.header("retry-after")), (StatusCode::SERVICE_UNAVAILABLE, Some("2")));
    let problem = reply.json();
    assert_eq!(problem["detail"], "reworded, source throttled");
    assert_eq!(problem["details"][0]["type"], "retry_after");

    let challenged = app.send(Call::get("/reworded-challenge")).await;
    assert_eq!((challenged.status, challenged.header("www-authenticate")), (StatusCode::UNAUTHORIZED, Some("Basic realm=\"shop\"")));
    assert_eq!(challenged.json()["detail"], "reworded, source none");
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_reads_the_status_an_error_would_render_with() {
    let app = start(plain).await;

    let own = app.send(Call::get("/own-body/412")).await;
    assert_eq!((own.status, own.text()), (StatusCode::PRECONDITION_FAILED, "own body 412".to_owned()), "the error's own status");
    let kind = app.send(Call::get("/own-body/0")).await;
    assert_eq!((kind.status, kind.text()), (StatusCode::CONFLICT, "own body 409".to_owned()), "its kind's, with none of its own");
    app.stop().await;
}
