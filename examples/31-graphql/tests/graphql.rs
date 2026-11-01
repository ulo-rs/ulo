use ex_31_graphql::builder;
use ex_support::{Reply, send, start};
use serde_json::{Value, json};

const GRAPHQL_RESPONSE: &str = "application/graphql-response+json; charset=utf-8";

async fn post(addr: std::net::SocketAddr, accept: Option<&str>, body: Value) -> Reply {
    let body = body.to_string();
    let accept = accept.map(|a| format!("accept: {a}\r\n")).unwrap_or_default();
    send(
        addr,
        &format!(
            "POST /graphql HTTP/1.1\r\nhost: example\r\nconnection: close\r\ncontent-type: application/json\r\n{accept}content-length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

fn json_of(reply: &Reply) -> Value {
    serde_json::from_str(&reply.body).unwrap_or_else(|e| panic!("a JSON body, got {:?}: {e}", reply.body))
}

#[tokio::test]
async fn a_query_answers_its_data_with_the_graphql_media_type() {
    let server = start(builder()).await;
    let reply = post(server.addr, None, json!({ "query": "query Hi($n: String!) { greeting(name: $n) }", "variables": { "n": "ulo" } })).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some(GRAPHQL_RESPONSE));
    assert_eq!(json_of(&reply), json!({ "data": { "greeting": "hello ulo" } }));
    server.stop().await.unwrap();
}

#[tokio::test]
async fn a_mutation_changes_what_the_next_query_reads() {
    let server = start(builder()).await;
    let added = post(server.addr, None, json!({ "query": r#"mutation { first: addNote(text: "one") }"# })).await;
    assert_eq!((added.status, json_of(&added)), (200, json!({ "data": { "first": 1 } })));
    let added = post(server.addr, None, json!({ "query": r#"mutation { second: addNote(text: "two") }"# })).await;
    assert_eq!(json_of(&added), json!({ "data": { "second": 2 } }));

    let read = post(server.addr, None, json!({ "query": "{ notes }" })).await;
    assert_eq!(json_of(&read), json!({ "data": { "notes": ["one", "two"] } }));
    server.stop().await.unwrap();
}

/// Two fields of one request read one context; the next request reads a new one.
#[tokio::test]
async fn the_context_is_built_once_per_request() {
    let server = start(builder()).await;
    let first = post(server.addr, None, json!({ "query": "{ a: execution b: execution }" })).await;
    assert_eq!(json_of(&first), json!({ "data": { "a": 1, "b": 1 } }));
    let second = post(server.addr, None, json!({ "query": "{ a: execution b: execution }" })).await;
    assert_eq!(json_of(&second), json!({ "data": { "a": 2, "b": 2 } }));
    // The engine reads the context once per request whatever its scope; a second lookup inside the
    // same execution is what tells execution scope from transient.
    let shared = post(server.addr, None, json!({ "query": "{ sameContext }" })).await;
    assert_eq!(json_of(&shared), json!({ "data": { "sameContext": true } }));
    server.stop().await.unwrap();
}

/// A required variable left out is a fault in the request, variable coercion's: 422, no `data`
/// key, and an error that names the variable and where it is declared but no field `path`. A
/// client accepting only the legacy `application/json` gets the same status and body, under
/// `application/graphql-response+json`, which the draft keeps on every response but a 2xx.
#[tokio::test]
async fn a_missing_required_variable_is_a_request_error() {
    let server = start(builder()).await;
    let query = "query Hi($n: String!) { greeting(name: $n) }";
    let expected = json!({
        "errors": [{
            "message": "Variable \"$n\" of required type \"String!\" was not provided.",
            "locations": [{ "line": 1, "column": 10 }],
        }],
    });

    let reply = post(server.addr, None, json!({ "query": query })).await;
    assert_eq!(reply.status, 422);
    assert_eq!(reply.header("content-type"), Some(GRAPHQL_RESPONSE));
    assert_eq!(json_of(&reply), expected);

    let legacy = post(server.addr, Some("application/json"), json!({ "query": query })).await;
    assert_eq!(legacy.status, 422);
    assert_eq!(legacy.header("content-type"), Some(GRAPHQL_RESPONSE));
    assert_eq!(json_of(&legacy), expected);
    server.stop().await.unwrap();
}
