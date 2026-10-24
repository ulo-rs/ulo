//! The HTTP extractors: each one's success and the `ExtractError` variant and parameter it fails
//! with, and the status that variant renders. Every case runs against two apps: one that renders
//! the failure as the framework does, and one whose global error handler reports the variant and
//! the parameter it saw, so a failure naming the wrong parameter shows.

mod o1_support;

use std::sync::Arc;

use futures_util::StreamExt;
use http::{Method, StatusCode};
use o1_support::{Call, Running, start};
use serde::Deserialize;
use ulo::{AnyErrorHandler, BoxError, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::{BodyLimit, BodyStream, Bytes, Form, Header, Host, Http, HttpBody, HttpCx, Json, Multipart, Path, Query, Response};
use ulo_transport::{CallError, ErrorKind, ExtractError, Valid, Validate};

#[derive(Deserialize)]
struct Search {
    page: u32,
    q: Option<String>,
}

#[derive(Deserialize)]
struct Ids {
    id: u32,
}

#[derive(Deserialize)]
struct Login {
    user: String,
    pw: String,
}

#[derive(Deserialize, Validate)]
struct Item {
    #[validate(length(min = 1, max = 8))]
    name: String,
    #[validate(range(min = 1))]
    qty: u32,
}

/// What a host put in the request's extensions.
#[derive(Clone)]
struct Who(&'static str);

#[injectable]
struct Inputs;

#[routes]
impl Inputs {
    #[ulo_http::get("/scalar/{id}")]
    fn scalar(&self, Path(id): Path<u32>) -> String {
        id.to_string()
    }

    #[ulo_http::get("/record/{id}")]
    fn record(&self, Path(ids): Path<Ids>) -> String {
        ids.id.to_string()
    }

    #[ulo_http::get("/query")]
    fn query(&self, Query(search): Query<Search>) -> String {
        format!("{}|{}", search.page, search.q.as_deref().unwrap_or("-"))
    }

    #[ulo_http::post("/json")]
    fn json(&self, Json(value): Json<serde_json::Value>) -> String {
        value["n"].to_string()
    }

    #[ulo_http::post("/form")]
    fn form(&self, Form(login): Form<Login>) -> String {
        format!("{}|{}", login.user, login.pw)
    }

    #[ulo_http::post("/upload")]
    #[meta(BodyLimit(512))]
    async fn upload(&self, mut parts: Multipart) -> Result<String, CallError> {
        let refuse = |error: multer::Error| CallError::new(ErrorKind::BadRequest, error.to_string());
        let mut seen = Vec::new();
        while let Some(field) = parts.next_field().await.map_err(refuse)? {
            let name = field.name().unwrap_or("-").to_owned();
            let file = field.file_name().map(str::to_owned);
            let text = field.text().await.map_err(refuse)?;
            seen.push(match file {
                Some(file) => format!("{name}[{file}]={text}"),
                None => format!("{name}={text}"),
            });
        }
        Ok(seen.join(";"))
    }

    /// Reads the parts and answers what a read failure was, as its source reported it.
    #[ulo_http::post("/upload-small")]
    #[meta(BodyLimit(16))]
    async fn upload_small(&self, mut parts: Multipart) -> String {
        loop {
            match parts.next_field().await {
                Ok(Some(field)) => drop(field.bytes().await),
                Ok(None) => return "ok".to_owned(),
                Err(multer::Error::StreamReadFailed(source)) => return describe_boxed(&source),
                Err(_) => return "other multipart error".to_owned(),
            }
        }
    }

    #[ulo_http::post("/stream")]
    #[meta(BodyLimit(8))]
    async fn stream(&self, mut body: BodyStream) -> String {
        let mut read = 0;
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => read += bytes.len(),
                Err(error) => return format!("{read}|{}", describe_boxed(&error)),
            }
        }
        format!("{read}|ok")
    }

    #[ulo_http::post("/bytes")]
    #[meta(BodyLimit(8))]
    fn bytes(&self, bytes: Bytes) -> String {
        bytes.len().to_string()
    }

    #[ulo_http::get("/host")]
    fn host(&self, who: Host<Who>) -> String {
        who.0.0.to_owned()
    }

    #[ulo_http::get("/host-optional")]
    fn host_optional(&self, who: Option<Host<Who>>) -> String {
        who.map_or("none", |who| who.0.0).to_owned()
    }

    #[ulo_http::get("/agent")]
    fn agent(&self, agent: Header<headers::UserAgent>) -> String {
        agent.as_str().to_owned()
    }

    #[ulo_http::get("/agent-optional")]
    fn agent_optional(&self, agent: Option<Header<headers::UserAgent>>) -> String {
        agent.map_or("none".to_owned(), |agent| agent.as_str().to_owned())
    }

    #[ulo_http::post("/valid")]
    fn valid(&self, item: Valid<Json<Item>>) -> String {
        format!("{}|{}", item.0.0.name, item.0.0.qty)
    }
}

fn describe(error: &ExtractError) -> String {
    match error {
        ExtractError::Missing { param } => format!("Missing|{param}"),
        ExtractError::Malformed { param, .. } => format!("Malformed|{param}"),
        ExtractError::UnsupportedMediaType { param, expected } => format!("UnsupportedMediaType|{param}|{expected}"),
        ExtractError::TooLarge { param, limit } => format!("TooLarge|{param}|{limit}"),
        ExtractError::Invalid { param, violations } => format!("Invalid|{param}|{}", violations.len()),
        ExtractError::HostMissing { param, type_name } => format!("HostMissing|{param}|{type_name}"),
        _ => "other extract error".to_owned(),
    }
}

fn describe_boxed(error: &BoxError) -> String {
    error.downcast_ref::<ExtractError>().map_or_else(|| "not an extract error".to_owned(), describe)
}

/// Answers every error with 200 and what the extractor reported.
struct Report;

impl ErrorHandler<Http> for Report {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        let text = call.source_as::<ExtractError>().map_or_else(|| format!("other|{}", call.kind()), describe);
        Ok(Response::new(HttpBody::from(text)))
    }
}

fn plain(m: &mut ModuleDef<'_>) {
    m.controller::<Inputs>();
}

fn reporting(m: &mut ModuleDef<'_>) {
    plain(m);
    let report: Arc<AnyErrorHandler<Http>> = Arc::new(Report);
    m.enhancer::<AnyErrorHandler<Http>>().value(report);
}

/// The framework's plain app and the reporting one.
struct Apps {
    plain: Running,
    reporting: Running,
}

impl Apps {
    async fn start() -> Apps {
        Apps { plain: start(plain).await, reporting: start(reporting).await }
    }

    /// `call` answers `status` on the plain app and the reporting app sees `report`.
    async fn fails(&self, call: impl Fn() -> Call, status: StatusCode, report: &str) -> o1_support::Reply {
        let reply = self.plain.send(call()).await;
        assert_eq!(reply.status, status, "{report}: the status the plain app renders");
        let seen = self.reporting.send(call()).await;
        assert_eq!(seen.text(), report, "what the extractor reported");
        reply
    }

    /// `call` succeeds on both apps with body `expected`.
    async fn succeeds(&self, call: impl Fn() -> Call, expected: &str) {
        for app in [&self.plain, &self.reporting] {
            let reply = app.send(call()).await;
            assert_eq!((reply.status, reply.text()), (StatusCode::OK, expected.to_owned()));
        }
    }

    async fn stop(self) {
        self.plain.stop().await;
        self.reporting.stop().await;
    }
}

const BAD: StatusCode = StatusCode::BAD_REQUEST;

#[tokio::test]
async fn path_parses_a_scalar_and_a_record_and_a_failure_names_the_field_when_serde_does() {
    let apps = Apps::start().await;

    apps.succeeds(|| Call::get("/scalar/7"), "7").await;
    apps.succeeds(|| Call::get("/record/8"), "8").await;
    apps.fails(|| Call::get("/scalar/abc"), BAD, "Malformed|path").await;
    apps.fails(|| Call::get("/record/abc"), BAD, "Malformed|id").await;
    apps.stop().await;
}

#[tokio::test]
async fn query_decodes_values_and_a_failure_names_the_field() {
    let apps = Apps::start().await;

    apps.succeeds(|| Call::get("/query?page=2"), "2|-").await;
    apps.succeeds(|| Call::get("/query?page=2&q=a%20b+c"), "2|a b c").await;
    apps.fails(|| Call::get("/query?q=x"), BAD, "Missing|page").await;
    apps.fails(|| Call::get("/query?page=many"), BAD, "Malformed|page").await;
    apps.stop().await;
}

#[tokio::test]
async fn json_needs_a_json_media_type_and_a_body_and_names_what_failed() {
    let apps = Apps::start().await;
    let json = |content_type: &'static str, body: &'static str| move || Call::new(Method::POST, "/json").header("content-type", content_type).body(body);

    apps.succeeds(json("application/json", r#"{"n":5}"#), "5").await;
    apps.succeeds(json("application/json; charset=utf-8", r#"{"n":6}"#), "6").await;
    apps.succeeds(json("application/vnd.api+json", r#"{"n":7}"#), "7").await;

    let wrong = apps.fails(json("text/plain", r#"{"n":1}"#), StatusCode::UNSUPPORTED_MEDIA_TYPE, "UnsupportedMediaType|body|application/json").await;
    assert_eq!(wrong.header("accept"), Some("application/json"), "a 415 names what is accepted");
    apps.fails(json("application/json", "{not json"), BAD, "Malformed|body").await;
    apps.fails(|| Call::new(Method::POST, "/json").header("content-type", "application/json"), BAD, "Missing|body").await;
    apps.stop().await;
}

#[tokio::test]
async fn form_needs_the_urlencoded_media_type_and_names_the_field_that_failed() {
    let apps = Apps::start().await;
    let form = |content_type: &'static str, body: &'static str| move || Call::new(Method::POST, "/form").header("content-type", content_type).body(body);
    let urlencoded = "application/x-www-form-urlencoded";

    apps.succeeds(form(urlencoded, "user=ann&pw=x%21"), "ann|x!").await;
    let wrong = apps.fails(form("application/json", "user=ann&pw=x"), StatusCode::UNSUPPORTED_MEDIA_TYPE, &format!("UnsupportedMediaType|body|{urlencoded}")).await;
    assert_eq!(wrong.header("accept"), Some(urlencoded));
    apps.fails(form(urlencoded, "user=ann"), BAD, "Missing|pw").await;
    apps.stop().await;
}

const UPLOAD: &str = "--XB\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nHello\r\n\
--XB\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\ncontents\r\n--XB--\r\n";

#[tokio::test]
async fn multipart_yields_text_and_file_fields_in_order() {
    let apps = Apps::start().await;
    let upload = || Call::new(Method::POST, "/upload").header("content-type", "multipart/form-data; boundary=XB").body(UPLOAD);

    apps.succeeds(upload, "title=Hello;doc[a.txt]=contents").await;
    apps.stop().await;
}

#[tokio::test]
async fn a_multipart_body_that_outgrows_the_limit_mid_stream_fails_the_read_with_too_large() {
    let apps = Apps::start().await;
    let upload = || Call::new(Method::POST, "/upload-small").header("content-type", "multipart/form-data; boundary=XB").body(UPLOAD);

    apps.succeeds(upload, "TooLarge|body|16").await;
    apps.stop().await;
}

#[tokio::test]
async fn an_empty_multipart_body_is_missing() {
    let apps = Apps::start().await;

    apps.fails(|| Call::new(Method::POST, "/upload").header("content-type", "multipart/form-data; boundary=XB"), BAD, "Missing|body").await;
    apps.stop().await;
}

#[tokio::test]
async fn multipart_without_a_boundary_is_unsupported_and_a_declared_length_over_the_limit_is_too_large() {
    let apps = Apps::start().await;

    apps.fails(
        || Call::new(Method::POST, "/upload").header("content-type", "multipart/form-data").body(UPLOAD),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UnsupportedMediaType|body|multipart/form-data",
    )
    .await;
    apps.fails(
        || Call::new(Method::POST, "/upload").header("content-type", "multipart/form-data; boundary=XB").header("content-length", "999").body(UPLOAD),
        StatusCode::PAYLOAD_TOO_LARGE,
        "TooLarge|body|512",
    )
    .await;
    apps.stop().await;
}

#[tokio::test]
async fn a_body_stream_and_bytes_stop_at_the_routes_body_limit_and_report_the_limit() {
    let apps = Apps::start().await;

    apps.succeeds(|| Call::new(Method::POST, "/stream").body("12345"), "5|ok").await;
    apps.succeeds(|| Call::new(Method::POST, "/stream").body("123456789012345678"), "0|TooLarge|body|8").await;
    apps.succeeds(|| Call::new(Method::POST, "/bytes").body("12345678"), "8").await;
    apps.fails(|| Call::new(Method::POST, "/bytes").body("123456789"), StatusCode::PAYLOAD_TOO_LARGE, "TooLarge|body|8").await;
    apps.stop().await;
}

#[tokio::test]
async fn a_host_value_is_read_from_the_request_and_its_absence_is_a_withheld_500_unless_optional() {
    let apps = Apps::start().await;

    apps.succeeds(|| Call::get("/host").ext(Who("ann")), "ann").await;
    apps.succeeds(|| Call::get("/host-optional").ext(Who("bob")), "bob").await;
    apps.succeeds(|| Call::get("/host-optional"), "none").await;

    let report = format!("HostMissing|host|{}", std::any::type_name::<Who>());
    let missing = apps.fails(|| Call::get("/host"), StatusCode::INTERNAL_SERVER_ERROR, &report).await;
    assert!(!missing.text().contains("Who"), "the deployment fault names no type to the client: {}", missing.text());
    apps.stop().await;
}

#[tokio::test]
async fn a_typed_header_is_read_and_its_absence_names_the_header_unless_optional() {
    let apps = Apps::start().await;

    apps.succeeds(|| Call::get("/agent").header("user-agent", "o1/1"), "o1/1").await;
    apps.succeeds(|| Call::get("/agent-optional"), "none").await;
    apps.succeeds(|| Call::get("/agent-optional").header("user-agent", "o1/2"), "o1/2").await;
    apps.fails(|| Call::get("/agent"), BAD, "Missing|user-agent").await;
    apps.stop().await;
}

#[tokio::test]
async fn valid_runs_after_extraction_and_answers_422_with_every_violated_field() {
    let apps = Apps::start().await;
    let item = |body: &'static str| move || Call::new(Method::POST, "/valid").header("content-type", "application/json").body(body);

    apps.succeeds(item(r#"{"name":"ok","qty":2}"#), "ok|2").await;
    let reply = apps.fails(item(r#"{"name":"","qty":0}"#), StatusCode::UNPROCESSABLE_ENTITY, "Invalid|Json|2").await;

    let problem = reply.json();
    let details = problem["details"].as_array().expect("details");
    assert_eq!(details[0]["type"], "field_violations");
    let mut fields: Vec<&str> = details[0]["violations"].as_array().expect("violations").iter().filter_map(|violation| violation["field"].as_str()).collect();
    fields.sort_unstable();
    assert_eq!(fields, ["name", "qty"]);

    // A body that does not extract fails as extraction, before validation runs.
    apps.fails(item("{}"), BAD, "Malformed|body").await;
    apps.stop().await;
}
