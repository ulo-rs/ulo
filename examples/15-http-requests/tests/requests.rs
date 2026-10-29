use ex_support::http::{Call, call};
use ex_support::{Served, start};
use serde_json::{Value, json};

async fn app() -> Served {
    start(ex_15_http_requests::builder()).await
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[tokio::test]
async fn a_valid_post_answers_201_with_a_location_and_the_note_and_a_get_reads_it_back() {
    let server = app().await;

    let created = call(server.addr, Call::post("/notes").json(r#"{"title":"milk"}"#)).await;
    assert_eq!(created.status, 201);
    assert_eq!(created.header("location"), Some("/notes/1"));
    assert_eq!(created.header("content-type"), Some("application/json"));
    assert_eq!(json_of(&created.body), json!({"id": 1, "title": "milk"}));

    let found = call(server.addr, Call::get("/notes/1")).await;
    assert_eq!(found.status, 200);
    assert_eq!(json_of(&found.body), json!({"id": 1, "title": "milk"}));
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_title_that_breaks_its_rule_is_refused_before_the_handler_and_the_problem_names_the_field() {
    let server = app().await;

    let refused = call(server.addr, Call::post("/notes").json(r#"{"title":""}"#)).await;
    assert_eq!(refused.status, 422);
    assert_eq!(refused.header("content-type"), Some("application/problem+json"));
    let problem = json_of(&refused.body);
    assert_eq!(problem["status"], 422);
    assert_eq!(problem["detail"], "`Json` failed 1 validation rule");
    assert_eq!(problem["details"][0]["type"], "field_violations");
    assert_eq!(problem["details"][0]["violations"][0]["field"], "title");

    let long = call(server.addr, Call::post("/notes").json(&format!(r#"{{"title":"{}"}}"#, "x".repeat(21)))).await;
    assert_eq!(long.status, 422, "21 characters is one over the rule's maximum");
    assert_eq!(json_of(&long.body)["details"][0]["violations"][0]["field"], "title");

    let none = call(server.addr, Call::get("/notes")).await;
    assert_eq!(none.body, "[]", "the refused note was never created");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_body_that_is_not_json_is_a_400_and_another_media_type_a_415() {
    let server = app().await;

    let broken = call(server.addr, Call::post("/notes").json("{not json")).await;
    assert_eq!(broken.status, 400);
    assert_eq!(json_of(&broken.body)["detail"].as_str().map(|d| d.starts_with("`body` could not be decoded")), Some(true), "{}", broken.body);

    let wrong = call(server.addr, Call::post("/notes").header("content-type", "text/plain").body(r#"{"title":"x"}"#)).await;
    assert_eq!(wrong.status, 415);
    assert_eq!(wrong.header("accept"), Some("application/json"));
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_missing_note_is_a_404_problem_naming_the_id_the_error_carried() {
    let server = app().await;

    let missing = call(server.addr, Call::get("/notes/7")).await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.header("content-type"), Some("application/problem+json"));
    let problem = json_of(&missing.body);
    assert_eq!((problem["status"].as_u64(), problem["title"].as_str(), problem["detail"].as_str()), (Some(404), Some("Not Found"), Some("no note 7")));

    let malformed = call(server.addr, Call::get("/notes/seven")).await;
    assert_eq!(malformed.status, 400, "a path segment that is no u32 never reaches the handler");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn query_limits_the_list_and_the_reply_carries_the_total_in_a_header() {
    let server = app().await;
    for title in ["a", "b", "c"] {
        call(server.addr, Call::post("/notes").json(&format!(r#"{{"title":"{title}"}}"#))).await;
    }

    let limited = call(server.addr, Call::get("/notes?limit=2")).await;
    assert_eq!(limited.status, 200);
    assert_eq!(limited.header("x-total-count"), Some("3"));
    assert_eq!(json_of(&limited.body), json!([{"id": 1, "title": "a"}, {"id": 2, "title": "b"}]));

    let all = call(server.addr, Call::get("/notes")).await;
    assert_eq!(json_of(&all.body).as_array().map(Vec::len), Some(3));

    let bad = call(server.addr, Call::get("/notes?limit=lots")).await;
    assert_eq!(bad.status, 400);
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_typed_header_is_read_and_its_absence_names_the_header() {
    let server = app().await;

    let seen = call(server.addr, Call::get("/agent").header("user-agent", "ex-15/1")).await;
    assert_eq!((seen.status, seen.body.as_str()), (200, "ex-15/1"));

    let absent = call(server.addr, Call::get("/agent")).await;
    assert_eq!(absent.status, 400);
    assert_eq!(json_of(&absent.body)["detail"], "`user-agent` is missing");
    server.stop().await.expect("closes clean");
}
