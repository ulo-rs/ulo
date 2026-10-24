//! GraphQL over HTTP, mounted (transports DESIGN §7; SWITCH_INVENTORY G18): the schema served at
//! its path, a request reaching the engine whole, the status each outcome gets under each media
//! type, a GET's refusal of a mutation, the playground, and what `GraphqlModule`'s identity does
//! with two of them.
//!
//! The engine is a stub that answers by what the query names, so these tests pin the binding and
//! not a GraphQL library. Every call runs under a real-clock deadline.

// `StartupError` is large by design; `try_start` returns it as `listen()` does.
#![allow(clippy::result_large_err)]

mod common;

use std::sync::Arc;

use serde_json::json;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, StartupError, WiringError};
use ulo_graphql_http::{GraphqlConfig, GraphqlModule};
use ulo_http::{AppService, StatusCode};

use common::{Answer, EngineModule, Keeper, PATIENCE, Seen, read, request};

struct Root<F>(F);

impl<F: Fn(&mut ModuleDef<'_>) + Send + Sync + 'static> Module for Root<F> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        (self.0)(m);
    }
}

struct Running {
    svc: AppService,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not finish within {PATIENCE:?}"))
}

/// The app wired, connected and listening on a backend that serves nothing itself, or the
/// `StartupError` it stopped with.
async fn try_start<F: Fn(&mut ModuleDef<'_>) + Send + Sync + 'static>(register: F) -> Result<Running, StartupError> {
    let keeper = Keeper::default();
    let app = within("the start", async {
        App::builder(Root(register))
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()?
            .connect()
            .await?
            .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()))
            .listen()
            .await
    })
    .await?;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Ok(Running { svc: keeper.service(), handle, serving })
}

async fn start<F: Fn(&mut ModuleDef<'_>) + Send + Sync + 'static>(register: F) -> Running {
    try_start(register).await.unwrap_or_else(|error| panic!("the app did not start: {error}"))
}

impl Running {
    async fn call(&self, method: &str, uri: &str, headers: &[(&str, &str)], body: &[u8]) -> Answer {
        within("the call", async { read(self.svc.call(request(method, uri, headers, body)).await).await }).await
    }

    async fn post(&self, uri: &str, headers: &[(&str, &str)], body: serde_json::Value) -> Answer {
        let mut all = vec![("content-type", "application/json")];
        all.extend_from_slice(headers);
        self.call("POST", uri, &all, body.to_string().as_bytes()).await
    }

    async fn stop(self) {
        within("the close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

fn one_endpoint(seen: &Arc<Seen>, config: fn() -> GraphqlConfig, page: bool) -> impl Fn(&mut ModuleDef<'_>) + Send + Sync + use<> {
    let seen = Arc::clone(seen);
    move |m: &mut ModuleDef<'_>| {
        m.import(GraphqlModule::for_root(config().engine_from(EngineModule { seen: Arc::clone(&seen), page })));
    }
}

const GRAPHQL_RESPONSE: &str = "application/graphql-response+json; charset=utf-8";
const LEGACY_JSON: &str = "application/json; charset=utf-8";

/// A POST at the configured path reaches the engine with every field of the request, and the
/// engine's answer comes back as the body; another path is not the endpoint.
#[tokio::test]
async fn a_post_reaches_the_engine_whole_and_the_answer_is_served_at_the_path() {
    let seen = Arc::new(Seen::default());
    let app = start(one_endpoint(&seen, || GraphqlConfig::at("/graphql"), false)).await;

    let body = json!({
        "query": "query Q($n: Int) { executed }",
        "operationName": "Q",
        "variables": { "n": 3 },
        "extensions": { "persisted": { "hash": "abc" } },
    });
    let answer = app.post("/graphql", &[], body).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.header("content-type"), Some(GRAPHQL_RESPONSE));
    assert_eq!(answer.json(), json!({ "data": { "ok": true } }));

    let requests = seen.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].query, "query Q($n: Int) { executed }");
    assert_eq!(requests[0].operation_name.as_deref(), Some("Q"));
    assert_eq!(requests[0].variables.as_ref().map(|v| serde_json::Value::Object(v.clone())), Some(json!({ "n": 3 })));
    assert_eq!(requests[0].extensions.as_ref().map(|v| serde_json::Value::Object(v.clone())), Some(json!({ "persisted": { "hash": "abc" } })));

    let elsewhere = app.post("/other", &[], json!({ "query": "{ executed }" })).await;
    assert_eq!(elsewhere.status, StatusCode::NOT_FOUND);
    assert_eq!(seen.requests().len(), 1, "the engine heard nothing of it");
    app.stop().await;
}

/// The status of each outcome under each negotiated media type: executed 200 whatever its
/// errors, a request error 400 under `application/graphql-response+json` and 200 under
/// `application/json`, a failed request 500 under both. No `Accept` reads as the new type.
#[tokio::test]
async fn the_status_follows_the_outcome_and_the_media_type_negotiated_from_accept() {
    let seen = Arc::new(Seen::default());
    let app = start(one_endpoint(&seen, || GraphqlConfig::at("/graphql"), false)).await;

    let table: [(&str, Option<&str>, StatusCode, &str); 12] = [
        ("{ executed }", None, StatusCode::OK, GRAPHQL_RESPONSE),
        ("{ fieldError }", None, StatusCode::OK, GRAPHQL_RESPONSE),
        ("{ requestError }", None, StatusCode::BAD_REQUEST, GRAPHQL_RESPONSE),
        ("{ failed }", None, StatusCode::INTERNAL_SERVER_ERROR, GRAPHQL_RESPONSE),
        ("{ requestError }", Some("application/graphql-response+json"), StatusCode::BAD_REQUEST, GRAPHQL_RESPONSE),
        ("{ requestError }", Some("application/json"), StatusCode::OK, LEGACY_JSON),
        ("{ fieldError }", Some("application/json"), StatusCode::OK, LEGACY_JSON),
        ("{ failed }", Some("application/json"), StatusCode::INTERNAL_SERVER_ERROR, LEGACY_JSON),
        ("{ requestError }", Some("application/json, application/graphql-response+json"), StatusCode::BAD_REQUEST, GRAPHQL_RESPONSE),
        ("{ requestError }", Some("application/graphql-response+json;q=0.5, application/json"), StatusCode::OK, LEGACY_JSON),
        ("{ requestError }", Some("*/*"), StatusCode::BAD_REQUEST, GRAPHQL_RESPONSE),
        ("{ requestError }", Some("text/plain"), StatusCode::OK, LEGACY_JSON),
    ];
    for (query, accept, status, media) in table {
        let headers: Vec<(&str, &str)> = accept.map(|accept| ("accept", accept)).into_iter().collect();
        let answer = app.post("/graphql", &headers, json!({ "query": query })).await;
        assert_eq!((answer.status, answer.header("content-type")), (status, Some(media)), "{query} accepting {accept:?}");
    }
    app.stop().await;
}

/// A GET carries the request in its query string, `variables` and `extensions` as JSON text; a
/// document whose operation is a mutation is refused 405 with `Allow` and never executed.
#[tokio::test]
async fn a_get_reads_the_query_string_and_refuses_a_mutation_with_405_and_allow() {
    let seen = Arc::new(Seen::default());
    let app = start(one_endpoint(&seen, || GraphqlConfig::at("/graphql"), false)).await;

    let uri = "/graphql?query=query%20Q(%24n%3A%20Int)%20%7B%20executed%20%7D&operationName=Q&variables=%7B%22n%22%3A3%7D";
    let answer = app.call("GET", uri, &[], b"").await;
    assert_eq!(answer.status, StatusCode::OK);
    let requests = seen.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].query, "query Q($n: Int) { executed }");
    assert_eq!(requests[0].operation_name.as_deref(), Some("Q"));
    assert_eq!(requests[0].variables.as_ref().map(|v| serde_json::Value::Object(v.clone())), Some(json!({ "n": 3 })));

    let mutation = app.call("GET", "/graphql?query=mutation%20%7B%20executed%20%7D", &[], b"").await;
    assert_eq!(mutation.status, StatusCode::METHOD_NOT_ALLOWED);
    let allow = mutation.header("allow").expect("a 405 carries `Allow`");
    let allowed: Vec<&str> = allow.split(',').map(str::trim).collect();
    assert!(allowed.contains(&"GET") && allowed.contains(&"POST"), "`Allow` names the methods the path answers: {allow}");
    assert_eq!(seen.requests().len(), 1, "the mutation was not executed");

    let named = app.call("GET", "/graphql?query=query%20A%20%7B%20executed%20%7D%20mutation%20B%20%7B%20executed%20%7D&operationName=A", &[], b"").await;
    assert_eq!(named.status, StatusCode::OK, "the operation the request names decides, not the other in the document");

    let bad = app.call("GET", "/graphql?query=%7B%20executed%20%7D&variables=%5B1%5D", &[], b"").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "`variables` that is not a JSON object is a request error");
    app.stop().await;
}

/// A POST body that is not `application/json` is refused 415 before the engine; one that is JSON
/// and not a GraphQL request is a request error.
#[tokio::test]
async fn a_post_that_is_not_json_is_415_and_a_body_that_is_no_request_is_a_request_error() {
    let seen = Arc::new(Seen::default());
    let app = start(one_endpoint(&seen, || GraphqlConfig::at("/graphql"), false)).await;

    let form = app.call("POST", "/graphql", &[("content-type", "application/x-www-form-urlencoded")], b"query=%7B%20executed%20%7D").await;
    assert_eq!(form.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let bare = app.call("POST", "/graphql", &[], b"{ \"query\": \"{ executed }\" }").await;
    assert_eq!(bare.status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "no `Content-Type` is not JSON");
    assert!(seen.requests().is_empty(), "the engine heard nothing of either");

    let no_query = app.post("/graphql", &[], json!({ "operationName": "Q" })).await;
    assert_eq!(no_query.status, StatusCode::BAD_REQUEST);
    assert_eq!(no_query.header("content-type"), Some(GRAPHQL_RESPONSE));
    assert!(no_query.json()["errors"].as_array().is_some_and(|errors| errors.len() == 1), "one error in the response format");
    assert!(seen.requests().is_empty());
    app.stop().await;
}

/// The engine's playground answers a GET that names `text/html` and no `query`, given the
/// endpoint's path and the subscription path; `*/*` is not a browser, a `query` is a query, and
/// `.playground(false)` or an engine with no page serves none.
#[tokio::test]
async fn the_playground_is_html_for_a_browser_with_no_query_and_only_when_enabled() {
    let seen = Arc::new(Seen::default());
    let on = |seen: &Arc<Seen>, page: bool| {
        let seen = Arc::clone(seen);
        move |m: &mut ModuleDef<'_>| {
            m.import(ulo_ws::WsModule::for_root());
            m.import(GraphqlModule::for_root(
                GraphqlConfig::at("/graphql").subscriptions("/graphql/ws").playground(true).engine_from(EngineModule { seen: Arc::clone(&seen), page }),
            ));
        }
    };
    let app = start(on(&seen, true)).await;

    let html = [("accept", "text/html,application/xhtml+xml,*/*;q=0.8")];
    let page = app.call("GET", "/graphql", &html, b"").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.header("content-type"), Some("text/html; charset=utf-8"));
    assert_eq!(page.text(), "<html>stub playground</html>");
    assert_eq!(seen.playground(), [("/graphql".to_owned(), Some("/graphql/ws".to_owned()))], "the page is told both paths as a browser reaches them");

    let curl = app.call("GET", "/graphql", &[("accept", "*/*")], b"").await;
    assert_eq!(curl.status, StatusCode::BAD_REQUEST, "`*/*` is no browser: a GET with no query is a request error");
    let queried = app.call("GET", "/graphql?query=%7B%20executed%20%7D", &html, b"").await;
    assert_eq!(queried.header("content-type"), Some(GRAPHQL_RESPONSE), "a `query` is answered as a query whatever the browser accepts");
    assert_eq!(seen.playground().len(), 1, "the engine was asked for the page once");
    app.stop().await;

    let seen = Arc::new(Seen::default());
    let off = {
        let seen = Arc::clone(&seen);
        move |m: &mut ModuleDef<'_>| {
            m.import(GraphqlModule::for_root(GraphqlConfig::at("/graphql").playground(false).engine_from(EngineModule { seen: Arc::clone(&seen), page: true })));
        }
    };
    let app = start(off).await;
    assert_eq!(app.call("GET", "/graphql", &html, b"").await.status, StatusCode::BAD_REQUEST, "disabled: no page");
    app.stop().await;

    let seen = Arc::new(Seen::default());
    let none = {
        let seen = Arc::clone(&seen);
        move |m: &mut ModuleDef<'_>| {
            m.import(GraphqlModule::for_root(GraphqlConfig::at("/graphql").playground(true).engine_from(EngineModule { seen: Arc::clone(&seen), page: false })));
        }
    };
    let app = start(none).await;
    assert_eq!(app.call("GET", "/graphql", &html, b"").await.status, StatusCode::BAD_REQUEST, "an engine with no page serves none");
    assert_eq!(seen.playground().len(), 1, "the engine was asked and answered `None`");
    app.stop().await;
}

/// One engine at two paths mounts both, each answering; the paths are normalised, so a trailing
/// or missing slash in the configuration is the same path.
#[tokio::test]
async fn the_same_schema_at_two_paths_mounts_both() {
    let seen = Arc::new(Seen::default());
    let app = start({
        let seen = Arc::clone(&seen);
        move |m: &mut ModuleDef<'_>| {
            for path in ["/graphql", "v2/graphql/"] {
                m.import(GraphqlModule::for_root(GraphqlConfig::at(path).engine_from(EngineModule { seen: Arc::clone(&seen), page: false })));
            }
        }
    })
    .await;
    for path in ["/graphql", "/v2/graphql"] {
        let answer = app.post(path, &[], json!({ "query": "{ executed }" })).await;
        assert_eq!((path, answer.status), (path, StatusCode::OK));
    }
    assert_eq!(seen.requests().len(), 2, "one engine instance served both paths");
    app.stop().await;
}

/// The configuration is the module's identity: the same configuration imported twice is one
/// module and mounts once, so it wires where two paths would be two modules.
#[tokio::test]
async fn an_equal_configuration_imported_twice_is_one_module() {
    let seen = Arc::new(Seen::default());
    let app = start({
        let seen = Arc::clone(&seen);
        move |m: &mut ModuleDef<'_>| {
            for _ in 0..2 {
                m.import(GraphqlModule::for_root(GraphqlConfig::at("/graphql").engine_from(EngineModule { seen: Arc::clone(&seen), page: false })));
            }
        }
    })
    .await;
    assert_eq!(app.post("/graphql", &[], json!({ "query": "{ executed }" })).await.status, StatusCode::OK);
    app.stop().await;
}

/// A zero `connection_init_timeout` would close every subscription connection before its
/// `connection_init` could arrive, and is refused at `wire()` naming the setting.
#[tokio::test]
async fn a_zero_connection_init_timeout_is_refused_at_wiring() {
    let seen = Arc::new(Seen::default());
    let started = try_start({
        let seen = Arc::clone(&seen);
        move |m: &mut ModuleDef<'_>| {
            m.import(ulo_ws::WsModule::for_root());
            m.import(GraphqlModule::for_root(
                GraphqlConfig::at("/graphql")
                    .subscriptions("/graphql/ws")
                    .connection_init_timeout(ulo::Bound::After(std::time::Duration::ZERO))
                    .engine_from(EngineModule { seen: Arc::clone(&seen), page: false }),
            ));
        }
    })
    .await;
    let Err(StartupError::Wiring(errors)) = started else { panic!("a zero `connection_init_timeout` was not refused at `wire()`") };
    let failed = errors.iter().filter(|entry| matches!(entry, WiringError::ValueFailed { .. })).count();
    assert_eq!(failed, 1, "one refused value: {errors}");
}

/// Two different configurations on one path are two modules, so the path is answered twice: the
/// HTTP transport refuses it when the app listens, as `StartupError::Configure` with one entry
/// for the transport naming both routes, and the app never serves.
#[tokio::test]
async fn two_configurations_on_one_path_are_refused_when_the_app_listens() {
    let seen = Arc::new(Seen::default());
    let started = try_start({
        let seen = Arc::clone(&seen);
        move |m: &mut ModuleDef<'_>| {
            m.import(GraphqlModule::for_root(GraphqlConfig::at("/graphql").playground(true).engine_from(EngineModule { seen: Arc::clone(&seen), page: false })));
            m.import(GraphqlModule::for_root(GraphqlConfig::at("/graphql").playground(false).engine_from(EngineModule { seen: Arc::clone(&seen), page: false })));
        }
    })
    .await;
    let Err(StartupError::Configure(errors)) = started else { panic!("two modules on one path were not refused at `listen()`") };
    let entries: Vec<_> = errors.iter().collect();
    assert_eq!(entries.len(), 1, "one entry, for the HTTP transport: {errors}");
    let report = entries[0].source.to_string();
    assert!(report.contains("GET /graphql") && report.contains("POST /graphql"), "both routes the second module repeats: {report}");
}
