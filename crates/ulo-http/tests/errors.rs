//! The HTTP error model: every `ErrorKind` renders its status as a problem document, a derived
//! `Classify` maps each variant to its kind, details travel in the document and in `Retry-After`,
//! an error nothing classified is withheld, a panic is a 500, and the error chain lets a handler
//! claim, pass on or reshape an error.

mod o1_support;

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{AnyErrorHandler, BoxError, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::{Http, HttpBody, HttpCx, Path, Response};
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
        _ => ErrorKind::Internal,
    }
}

#[injectable]
struct Faults;

#[routes]
impl Faults {
    #[ulo_http::get("/kind/{name}")]
    fn kind(&self, Path(name): Path<String>) -> Result<&'static str, CallError> {
        Err(CallError::new(kind_named(&name), "the message"))
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
}

fn chained(m: &mut ModuleDef<'_>) {
    plain(m);
    let claim: Arc<AnyErrorHandler<Http>> = Arc::new(GlobalClaim);
    m.enhancer::<AnyErrorHandler<Http>>().value(claim);
}

#[tokio::test]
async fn every_error_kind_renders_its_status_as_a_problem_document_of_that_status() {
    let app = start(plain).await;
    let table = [
        ("bad_request", StatusCode::BAD_REQUEST),
        ("unauthorized", StatusCode::UNAUTHORIZED),
        ("forbidden", StatusCode::FORBIDDEN),
        ("not_found", StatusCode::NOT_FOUND),
        ("conflict", StatusCode::CONFLICT),
        ("unprocessable", StatusCode::UNPROCESSABLE_ENTITY),
        ("too_many_requests", StatusCode::TOO_MANY_REQUESTS),
        ("timeout", StatusCode::GATEWAY_TIMEOUT),
        ("unavailable", StatusCode::SERVICE_UNAVAILABLE),
        ("unimplemented", StatusCode::NOT_IMPLEMENTED),
        ("internal", StatusCode::INTERNAL_SERVER_ERROR),
    ];

    for (name, status) in table {
        assert_eq!(kind_named(name).as_str(), name, "the table names the kind it claims to");
        let reply = app.send(Call::get(&format!("/kind/{name}"))).await;
        assert_eq!(reply.status, status, "{name}");
        assert_eq!(reply.header("content-type"), Some("application/problem+json"), "{name}");
        let problem = reply.json();
        assert_eq!(problem["status"], status.as_u16(), "{name}: the document repeats the status");
        assert_eq!(problem["detail"], "the message", "{name}");
    }
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
