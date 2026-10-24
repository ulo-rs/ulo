//! Handler forms and the replies a handler may return: sync, async and `self: Arc<Self>`; each
//! `IntoReply<Http>` shape's status, headers and body; a return type spelled as an alias of
//! `Result` reaching the error chain as the error it carries; and a reply that cannot be built
//! failing as a 500 before anything is written.

mod o1_support;

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{AnyErrorHandler, BoxError, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::{Bytes, Created, Http, HttpBody, HttpCx, Json, NoContent, Response, WithHeaders};
use ulo_transport::{CallError, Classify, ErrorKind};

/// A domain error classified `conflict`.
#[derive(Debug, Classify)]
#[classify(conflict)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

impl Error for Refusal {}

type Outcome<T> = Result<T, Refusal>;

#[injectable]
struct Replies;

#[routes]
impl Replies {
    #[ulo_http::get("/sync")]
    fn sync_form(&self) -> &'static str {
        "sync"
    }

    #[ulo_http::get("/async")]
    async fn async_form(&self) -> &'static str {
        "async"
    }

    #[ulo_http::get("/arc")]
    fn arc_form(self: Arc<Self>) -> &'static str {
        "arc"
    }

    #[ulo_http::get("/unit")]
    fn unit(&self) {}

    #[ulo_http::get("/no-content")]
    fn no_content(&self) -> NoContent {
        NoContent
    }

    #[ulo_http::get("/created")]
    fn created(&self) -> Created<String> {
        Created::at("/things/9", "made".to_owned())
    }

    #[ulo_http::get("/created-bad-location")]
    fn created_bad_location(&self) -> Created<String> {
        Created::at("/things/\n9", "made".to_owned())
    }

    #[ulo_http::get("/status")]
    fn status(&self) -> (StatusCode, &'static str) {
        (StatusCode::ACCEPTED, "queued")
    }

    #[ulo_http::get("/headers")]
    fn headers(&self) -> WithHeaders<&'static str> {
        WithHeaders::new("body").header("x-a", "1").header("x-a", "2").header("x-b", "3")
    }

    #[ulo_http::get("/bad-header")]
    fn bad_header(&self) -> WithHeaders<&'static str> {
        WithHeaders::new("body").header("not a header name", "1")
    }

    #[ulo_http::get("/bytes")]
    fn bytes(&self) -> Bytes {
        Bytes::from_static(b"\x00\x01")
    }

    #[ulo_http::get("/json")]
    fn json(&self) -> Json<Vec<u8>> {
        Json(vec![1, 2])
    }

    #[ulo_http::get("/raw")]
    fn raw(&self) -> HttpBody {
        HttpBody::from("raw")
    }

    #[ulo_http::get("/string")]
    fn string(&self) -> String {
        "text".to_owned()
    }

    #[ulo_http::get("/built")]
    fn built(&self) -> Response {
        let mut response = Response::new(HttpBody::from("built"));
        *response.status_mut() = StatusCode::IM_A_TEAPOT;
        response
    }

    #[ulo_http::get("/result-unit")]
    fn result_unit(&self) -> Result<(), CallError> {
        Ok(())
    }

    #[ulo_http::get("/alias-ok")]
    fn alias_ok(&self) -> Outcome<&'static str> {
        Ok("fine")
    }

    #[ulo_http::get("/alias-err")]
    fn alias_err(&self) -> Outcome<&'static str> {
        Err(Refusal)
    }

    #[ulo_http::get("/alias-err-async")]
    async fn alias_err_async(&self) -> Outcome<String> {
        Err(Refusal)
    }
}

/// Claims a `Refusal` as 200 and the name of what it received; anything else passes on.
struct ClaimRefusal;

impl ErrorHandler<Http> for ClaimRefusal {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        match call.source_as::<Refusal>() {
            Some(Refusal) => Ok(Response::new(HttpBody::from(format!("claimed {}", call.kind())))),
            None => Err(Box::new(call)),
        }
    }
}

fn unhandled(m: &mut ModuleDef<'_>) {
    m.controller::<Replies>();
}

fn handled(m: &mut ModuleDef<'_>) {
    unhandled(m);
    let claim: Arc<AnyErrorHandler<Http>> = Arc::new(ClaimRefusal);
    m.enhancer::<AnyErrorHandler<Http>>().value(claim);
}

#[tokio::test]
async fn sync_async_and_arc_receivers_all_route() {
    let app = start(unhandled).await;

    for (path, body) in [("/sync", "sync"), ("/async", "async"), ("/arc", "arc")] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, body.to_owned()), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn text_bytes_json_and_raw_bodies_carry_their_content_types() {
    let app = start(unhandled).await;

    let string = app.send(Call::get("/string")).await;
    assert_eq!((string.header("content-type"), string.text()), (Some("text/plain; charset=utf-8"), "text".to_owned()));
    let literal = app.send(Call::get("/sync")).await;
    assert_eq!(literal.header("content-type"), Some("text/plain; charset=utf-8"));
    let bytes = app.send(Call::get("/bytes")).await;
    assert_eq!((bytes.header("content-type"), bytes.body.as_slice()), (Some("application/octet-stream"), [0u8, 1].as_slice()));
    let json = app.send(Call::get("/json")).await;
    assert_eq!((json.header("content-type"), json.text()), (Some("application/json"), "[1,2]".to_owned()));
    let raw = app.send(Call::get("/raw")).await;
    assert_eq!((raw.status, raw.header("content-type"), raw.text()), (StatusCode::OK, None, "raw".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn unit_and_no_content_answer_204_with_no_body_and_so_does_an_ok_unit() {
    let app = start(unhandled).await;

    for path in ["/unit", "/no-content", "/result-unit"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT, "{path}");
        assert!(reply.body.is_empty(), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn created_status_and_header_wrappers_set_what_they_name_and_a_built_response_passes_as_it_is() {
    let app = start(unhandled).await;

    let created = app.send(Call::get("/created")).await;
    assert_eq!((created.status, created.header("location"), created.text()), (StatusCode::CREATED, Some("/things/9"), "made".to_owned()));
    let status = app.send(Call::get("/status")).await;
    assert_eq!((status.status, status.text()), (StatusCode::ACCEPTED, "queued".to_owned()));
    let headers = app.send(Call::get("/headers")).await;
    assert_eq!(headers.values("x-a"), ["1", "2"], "a repeated name is appended, not replaced");
    assert_eq!(headers.header("x-b"), Some("3"));
    let built = app.send(Call::get("/built")).await;
    assert_eq!((built.status, built.text()), (StatusCode::IM_A_TEAPOT, "built".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn a_reply_that_cannot_be_built_is_a_500_and_leaks_nothing_of_the_bad_value() {
    let app = start(unhandled).await;

    for path in ["/bad-header", "/created-bad-location"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert!(!reply.text().contains("made") && !reply.text().contains("not a header name"), "{path}: {}", reply.text());
    }
    app.stop().await;
}

#[tokio::test]
async fn an_alias_of_result_answers_its_ok_value_and_renders_its_error_by_the_errors_kind() {
    let app = start(unhandled).await;

    let ok = app.send(Call::get("/alias-ok")).await;
    assert_eq!((ok.status, ok.text()), (StatusCode::OK, "fine".to_owned()));
    for path in ["/alias-err", "/alias-err-async"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "{path}: the error was classified `conflict`");
    }
    app.stop().await;
}

#[tokio::test]
async fn an_alias_of_result_hands_its_error_to_the_chain_as_the_domain_error() {
    let app = start(handled).await;

    for path in ["/alias-err", "/alias-err-async"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, format!("claimed {}", ErrorKind::Conflict)), "{path}");
    }
    let ok = app.send(Call::get("/alias-ok")).await;
    assert_eq!(ok.text(), "fine", "an `Ok` never reaches the chain");
    app.stop().await;
}
