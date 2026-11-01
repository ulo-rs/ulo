//! The GraphQL-over-HTTP endpoint: a controller mounted `.at(config.path)`, reading
//! `Dep<dyn Engine, Q>`, answering GET and POST by the specification's rules from the response's
//! `Outcome` and `RequestFault`, and the negotiated media type.
//!
//! The rules are the GraphQL-over-HTTP working draft's (graphql/graphql-over-http,
//! `spec/GraphQLOverHTTP.md` at `d027fa2`, 2026-09-28), read as a shape rather than a contract: the
//! draft is unfinished, and its statuses are SHOULDs. Its status list reads, for a request error:
//! "If the JSON body of the request could not be parsed, status code `400` SHOULD be used." "If the
//! request is not a well-formed _GraphQL-over-HTTP request_, status code `422` SHOULD be used." "If
//! the GraphQL document in the request cannot be parsed, status code `400` SHOULD be used." "If the
//! request does not pass GraphQL validation, status code `422` SHOULD be used." "If the operation to
//! execute cannot be unambiguously determined, status code `422` SHOULD be used." "If the variable
//! values cannot be coerced to match the operation's variable definitions, status code `422` SHOULD
//! be used." Those statuses hold under either response media type; for a legacy client, one that
//! accepts `application/json` and not `application/graphql-response+json`, the request "SHOULD be
//! performed following the requirements of this specification as if the request had specified
//! `Accept: application/graphql-response+json`, except any response that produces a `2xx` status
//! code should replace the `Content-Type` header with `Content-Type: application/json`." An
//! `Accept` naming neither: "If the `Accept` header is present but does not indicate support for
//! any of the server's supported media types or `application/json`, the server SHOULD respond with
//! `406 Not Acceptable`." No `Accept`: "If a client does not supply the `Accept` header then the
//! server may respond with an error, or with any content type it chooses".

use std::marker::PhantomData;

use http::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value};
use ulo::scope::Auto;
use ulo::{BoxError, Construct, ConstructError, Controller, Dep, Dependencies, HandlerSpec, Mount, Resolver};
use ulo_graphql::{Engine, GqlError, GqlRequest, GqlResponse, Outcome, RequestFault};
use ulo_http::{
    Bytes, CallError, ErrorKind, ExtractError, FromCall, Http, HttpBody, HttpCx, HttpHandler, IntoReplyError, MethodNotAllowed,
    NotAcceptable, Query, Response,
};

use crate::playground;

/// What `GraphqlModule` binds beside the controller, read by it alone: the endpoint's path and the
/// subscription path as the playground writes them, and whether the playground is served.
pub(crate) struct EndpointSettings<Q> {
    pub(crate) path: String,
    pub(crate) subscriptions: Option<String>,
    pub(crate) playground: bool,
    pub(crate) _engine: PhantomData<fn() -> Q>,
}

/// The endpoint over the engine bound as `dyn Engine @ Q`. Its two handlers answer at `/`, which
/// the controller's prefix puts at the configured path.
pub(crate) struct GraphqlEndpoint<Q> {
    engine: Dep<dyn Engine, Q>,
    settings: Dep<EndpointSettings<Q>>,
}

impl<Q: Send + Sync + 'static> Construct for GraphqlEndpoint<Q> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<dyn Engine, Q>>("engine").field::<Dep<EndpointSettings<Q>>>("settings");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(GraphqlEndpoint {
            engine: r.dep_qualified::<dyn Engine, Q>().await?,
            settings: r.dep::<EndpointSettings<Q>>().await?,
        })
    }
}

impl<Q: Send + Sync + 'static> Controller for GraphqlEndpoint<Q> {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Http, _>::new("get", HttpHandler::new(Method::GET, get::<Q>)).route("/"));
        m.handler(HandlerSpec::<Http, _>::new("post", HttpHandler::new(Method::POST, post::<Q>)).route("/"));
    }
}

/// The query parameters a GET names; `variables` and `extensions` are JSON text.
#[derive(Deserialize)]
struct GetParams {
    query: Option<String>,
    #[serde(rename = "operationName")]
    operation_name: Option<String>,
    variables: Option<String>,
    extensions: Option<String>,
}

/// A GET: the playground for a browser asking for HTML with no `query`, otherwise a query read from
/// the query string. An `Accept` naming neither JSON type is refused with 406, and a mutation with
/// 405, before anything executes.
async fn get<Q: Send + Sync + 'static>(cx: HttpCx) -> Result<Response, BoxError> {
    let endpoint = cx.exec().get::<GraphqlEndpoint<Q>>().await?;
    let Query(params) = <Query<GetParams> as FromCall<Http>>::from_call(&cx).await?;
    if params.query.is_none()
        && endpoint.settings.playground
        && playground::accepts_html(cx.headers())
        && let Some(page) = playground::page(&*endpoint.engine, &endpoint.settings, cx.mount_prefix())
    {
        return Ok(page);
    }
    let media = Media::negotiate(cx.headers())?;
    let Some(query) = params.query else {
        return render(&request_error(RequestFault::NotWellFormed, "the request names no `query`"), media);
    };
    let variables = match json_object(params.variables.as_deref(), "variables") {
        Ok(variables) => variables,
        Err(response) => return render(&response, media),
    };
    let extensions = match json_object(params.extensions.as_deref(), "extensions") {
        Ok(extensions) => extensions,
        Err(response) => return render(&response, media),
    };
    let request = GqlRequest { query, operation_name: params.operation_name, variables, extensions };
    if operation_kind(&request.query, request.operation_name.as_deref()) == Some(OperationKind::Mutation) {
        return Err(mutation_over_get());
    }
    let response = endpoint.engine.execute(request, cx.exec().clone()).await;
    render(&response, media)
}

/// A POST: an `application/json` body holding the request. Another media type is refused with 415
/// and an `Accept` naming neither JSON type with 406, both through the error handlers, and a body
/// over the route's limit with 413. A body that is not JSON is a request error answered 400, and
/// JSON that is not a GraphQL request, `{"qeury": ..}` or `variables` that is not a map, 422.
async fn post<Q: Send + Sync + 'static>(cx: HttpCx) -> Result<Response, BoxError> {
    let endpoint = cx.exec().get::<GraphqlEndpoint<Q>>().await?;
    if !is_json(cx.headers()) {
        return Err(ExtractError::UnsupportedMediaType { param: "body", expected: "application/json" }.into());
    }
    let media = Media::negotiate(cx.headers())?;
    let body = <Bytes as FromCall<Http>>::from_call(&cx).await?;
    let request = match serde_json::from_slice::<GqlRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            let fault = match error.classify() {
                serde_json::error::Category::Data => RequestFault::NotWellFormed,
                _ => RequestFault::Encoding,
            };
            return render(&request_error(fault, &format!("the body is not a GraphQL request: {error}")), media);
        }
    };
    let response = endpoint.engine.execute(request, cx.exec().clone()).await;
    render(&response, media)
}

/// The response media type, negotiated from `Accept`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Media {
    GraphqlResponse,
    /// The client accepts both and prefers `application/json`.
    Json,
    /// The client accepts `application/json` and not `application/graphql-response+json`.
    Legacy,
}

const GRAPHQL_RESPONSE: &str = "application/graphql-response+json; charset=utf-8";
const JSON: &str = "application/json; charset=utf-8";

impl Media {
    /// `application/graphql-response+json` when the client accepts it at least as much as
    /// `application/json`, and when it sends no `Accept`, which the draft leaves to the server;
    /// `application/json` when it prefers that; the legacy rule when it accepts `application/json`
    /// alone. An `Accept` naming neither is the 406 of [`not_acceptable`].
    fn negotiate(headers: &HeaderMap) -> Result<Media, BoxError> {
        let ranges = accept_ranges(headers);
        if ranges.is_empty() {
            return Ok(Media::GraphqlResponse);
        }
        let graphql = quality(&ranges, "application", "graphql-response+json");
        let json = quality(&ranges, "application", "json");
        match (graphql, json) {
            (0, 0) => Err(not_acceptable()),
            (0, _) => Ok(Media::Legacy),
            (graphql, json) if graphql >= json => Ok(Media::GraphqlResponse),
            _ => Ok(Media::Json),
        }
    }

    /// The `Content-Type` for a response of `status`: the legacy rule's `application/json` for a
    /// `2xx` only, every other legacy response keeping `application/graphql-response+json`, by
    /// which a client tells a GraphQL error from an intermediary's.
    fn content_type(self, status: StatusCode) -> HeaderValue {
        match self {
            Media::GraphqlResponse => HeaderValue::from_static(GRAPHQL_RESPONSE),
            Media::Json => HeaderValue::from_static(JSON),
            Media::Legacy if status.is_success() => HeaderValue::from_static(JSON),
            Media::Legacy => HeaderValue::from_static(GRAPHQL_RESPONSE),
        }
    }
}

/// GraphQL-over-HTTP's status, the same under every media type: 200 for a request that executed,
/// errors included; for a request error 400 when its body or document does not parse and 422
/// otherwise, a request error whose fault the engine did not name included; 500 for a request the
/// server failed, the fault being the server's and no document's.
fn status(response: &GqlResponse) -> StatusCode {
    match response.outcome() {
        Outcome::Executed => StatusCode::OK,
        Outcome::Failed => StatusCode::INTERNAL_SERVER_ERROR,
        Outcome::RequestError => match response.fault() {
            Some(RequestFault::Encoding | RequestFault::Syntax) => StatusCode::BAD_REQUEST,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        },
    }
}

/// One `Accept` media range: type, subtype, lowercased, and its quality in thousandths.
pub(crate) struct Range {
    pub(crate) kind: String,
    pub(crate) subtype: String,
    pub(crate) q: u16,
}

/// Every media range of every `Accept` header; empty when there is none, or only empty ones.
pub(crate) fn accept_ranges(headers: &HeaderMap) -> Vec<Range> {
    let mut ranges = Vec::new();
    for value in headers.get_all(ACCEPT) {
        let Ok(text) = value.to_str() else { continue };
        for item in text.split(',') {
            let mut parts = item.split(';');
            let Some((kind, subtype)) = parts.next().and_then(|range| range.trim().split_once('/')) else { continue };
            let mut q = 1000;
            for parameter in parts {
                if let Some((name, value)) = parameter.split_once('=') {
                    if name.trim().eq_ignore_ascii_case("q") {
                        q = parse_quality(value.trim());
                    }
                }
            }
            ranges.push(Range { kind: kind.trim().to_ascii_lowercase(), subtype: subtype.trim().to_ascii_lowercase(), q });
        }
    }
    ranges
}

/// RFC 9110 §12.4.2's qvalue, `0` to `1` with at most three decimals; anything else reads as 0.
fn parse_quality(text: &str) -> u16 {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    let thousandths = format!("{fraction:0<3}").parse::<u16>().unwrap_or(0);
    match whole {
        "0" => thousandths,
        "1" if thousandths == 0 => 1000,
        _ => 0,
    }
}

/// The quality `ranges` give `kind/subtype`: that of the most specific range matching it, exact
/// before `kind/*` before `*/*` (RFC 9110 §12.5.1); 0 when none matches.
pub(crate) fn quality(ranges: &[Range], kind: &str, subtype: &str) -> u16 {
    let mut best: Option<(u8, u16)> = None;
    for range in ranges {
        let specificity = match (range.kind.as_str(), range.subtype.as_str()) {
            (k, s) if k == kind && s == subtype => 3,
            (k, "*") if k == kind => 2,
            ("*", "*") => 1,
            _ => continue,
        };
        if best.is_none_or(|(seen, _)| specificity > seen) {
            best = Some((specificity, range.q));
        }
    }
    best.map_or(0, |(_, q)| q)
}

/// Whether the request's `Content-Type` is `application/json`, parameters aside.
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"))
}

/// A query parameter holding JSON text, as an object; absent and `null` alike are `None`. Text
/// that is not a JSON object is a request that is not well-formed: the draft asks for `variables`
/// and `extensions` as maps, and a GET carries no body whose JSON could fail to parse.
fn json_object(text: Option<&str>, name: &str) -> Result<Option<Map<String, Value>>, GqlResponse> {
    let Some(text) = text else { return Ok(None) };
    serde_json::from_str::<Option<Map<String, Value>>>(text)
        .map_err(|error| request_error(RequestFault::NotWellFormed, &format!("`{name}` is not a JSON object: {error}")))
}

fn request_error(fault: RequestFault, message: &str) -> GqlResponse {
    GqlResponse::request_fault(fault, vec![GqlError::new(message)])
}

/// `response` in `media`, with the status GraphQL-over-HTTP gives its outcome.
fn render(response: &GqlResponse, media: Media) -> Result<Response, BoxError> {
    let body = serde_json::to_vec(response).map_err(IntoReplyError::new)?;
    let mut reply = Response::new(HttpBody::from_bytes(body));
    let status = status(response);
    *reply.status_mut() = status;
    reply.headers_mut().insert(CONTENT_TYPE, media.content_type(status));
    Ok(reply)
}

/// The 406 for an `Accept` naming neither JSON type: a `BadRequest` whose source is
/// [`NotAcceptable`], offered to the error handlers and rendered 406 as a problem document when
/// none claims it. The draft rules out `application/graphql-response+json` for it, a response
/// that no GraphQL result is part of.
fn not_acceptable() -> BoxError {
    let refused = NotAcceptable::new(["application/graphql-response+json", "application/json"]);
    BoxError::from(CallError::new(ErrorKind::BadRequest, refused.to_string()).with_source(refused))
}

/// The 405 for a GET whose operation is a mutation, which the specification forbids executing:
/// a `BadRequest` whose source is [`MethodNotAllowed`] with `Allow` as the router computes it for
/// this path, offered to the error handlers as the router's 405 is and rendered 405 with `Allow`
/// when none claims it.
fn mutation_over_get() -> BoxError {
    let refused = MethodNotAllowed::new(Method::GET, HeaderValue::from_static("GET, HEAD, POST, OPTIONS"));
    BoxError::from(CallError::new(ErrorKind::BadRequest, "a mutation is not executed over GET; send it as a POST").with_source(refused))
}

/// An operation's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

/// The type of the operation `document` selects: the one named `operation_name`, or its only
/// operation. `None` when that cannot be told, a document that does not lex or selects no single
/// operation, which the engine then reports as a request error.
///
/// A scan of the document's top level, not a parse: comments, strings and block strings are
/// skipped, and what sits inside parentheses (variable defaults, directive arguments) never opens
/// a selection set.
fn operation_kind(document: &str, operation_name: Option<&str>) -> Option<OperationKind> {
    #[derive(PartialEq)]
    enum State {
        /// Between definitions.
        Start,
        /// After `query`, `mutation` or `subscription`: the next name is the operation's.
        Keyword,
        /// In an operation's or a fragment's header, before its selection set.
        Header,
        /// Inside a selection set.
        Body,
    }

    let mut operations: Vec<(OperationKind, Option<String>)> = Vec::new();
    let mut state = State::Start;
    let mut pending: Option<(OperationKind, Option<String>)> = None;
    let mut braces = 0usize;
    let mut parens = 0usize;
    let bytes = document.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                    i += 1;
                }
                continue;
            }
            b'"' => {
                i = skip_string(bytes, i)?;
                continue;
            }
            b'(' => parens += 1,
            b')' => parens = parens.checked_sub(1)?,
            b'{' if parens == 0 => {
                if braces == 0 {
                    match state {
                        State::Start => operations.push((OperationKind::Query, None)),
                        State::Keyword | State::Header => operations.extend(pending.take()),
                        State::Body => return None,
                    }
                    state = State::Body;
                }
                braces += 1;
            }
            b'}' if parens == 0 => {
                braces = braces.checked_sub(1)?;
                if braces == 0 {
                    state = State::Start;
                }
            }
            b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                if braces == 0 && parens == 0 {
                    let name = &document[start..i];
                    match state {
                        State::Start => {
                            let kind = match name {
                                "query" => Some(OperationKind::Query),
                                "mutation" => Some(OperationKind::Mutation),
                                "subscription" => Some(OperationKind::Subscription),
                                "fragment" => None,
                                _ => return None,
                            };
                            pending = kind.map(|kind| (kind, None));
                            state = if pending.is_some() { State::Keyword } else { State::Header };
                        }
                        State::Keyword => {
                            if let Some((_, slot)) = pending.as_mut() {
                                *slot = Some(name.to_owned());
                            }
                            state = State::Header;
                        }
                        State::Header | State::Body => {}
                    }
                }
                continue;
            }
            b'@' | b'$' if braces == 0 && parens == 0 && state == State::Keyword => state = State::Header,
            _ => {}
        }
        if b == b'(' && braces == 0 && state == State::Keyword {
            state = State::Header;
        }
        i += 1;
    }
    if braces != 0 || parens != 0 {
        return None;
    }
    let selected = match operation_name {
        Some(wanted) => operations.into_iter().find(|(_, name)| name.as_deref() == Some(wanted)),
        None if operations.len() == 1 => operations.pop(),
        None => None,
    };
    selected.map(|(kind, _)| kind)
}

/// The index just past the string starting at `start`, a `"..."` string with escapes or a
/// `"""..."""` block string; `None` for one left open.
fn skip_string(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes[start..].starts_with(b"\"\"\"") {
        let mut i = start + 3;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"\\\"\"\"") {
                i += 4;
            } else if bytes[i..].starts_with(b"\"\"\"") {
                return Some(i + 3);
            } else {
                i += 1;
            }
        }
        return None;
    }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            b'\n' | b'\r' => return None,
            _ => i += 1,
        }
    }
    None
}
