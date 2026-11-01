use ex_support::http::{Call, call};
use ex_support::{Served, start};
use serde_json::Value;

async fn app() -> Served {
    start(ex_15_http_requests::builder()).await
}

fn detail(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap_or_else(|e| panic!("{e}: {body}"))["detail"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn a_form_is_decoded_from_urlencoded_pairs_and_json_is_not_a_form() {
    let server = app().await;

    let form = Call::post("/login").header("content-type", "application/x-www-form-urlencoded");
    let ok = call(server.addr, form.body("user=ann%20lee&pw=s3cret")).await;
    assert_eq!((ok.status, ok.body.as_str()), (200, "ann lee (6 characters of password)"));

    let wrong = call(server.addr, Call::post("/login").json(r#"{"user":"ann","pw":"x"}"#)).await;
    assert_eq!(wrong.status, 415);

    let partial = Call::post("/login").header("content-type", "application/x-www-form-urlencoded");
    assert_eq!(call(server.addr, partial.body("user=ann")).await.status, 422, "`pw` is missing: the form parsed and does not fit");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_body_stream_reads_the_body_without_the_handler_holding_it_whole() {
    let server = app().await;

    let counted = call(server.addr, Call::post("/count").body("hello world")).await;
    assert_eq!(counted.status, 200);
    assert!(counted.body.starts_with("11 bytes in "), "{}", counted.body);
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_hand_written_extractor_reads_the_call_and_fails_with_the_extract_error_it_chose() {
    let server = app().await;

    let named = call(server.addr, Call::get("/whoami").header("authorization", "Bearer ann")).await;
    assert_eq!((named.status, named.body.as_str()), (200, "ann"));

    let missing = call(server.addr, Call::get("/whoami")).await;
    assert_eq!(missing.status, 400);
    assert_eq!(detail(&missing.body), "`authorization` is missing");

    let basic = call(server.addr, Call::get("/whoami").header("authorization", "Basic YW5u")).await;
    assert_eq!(basic.status, 400);
    assert!(detail(&basic.body).contains("authorization"), "{}", basic.body);
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn option_turns_only_a_missing_value_into_none_and_a_malformed_one_still_fails() {
    let server = app().await;

    let guest = call(server.addr, Call::get("/whoami-or-guest")).await;
    assert_eq!((guest.status, guest.body.as_str()), (200, "guest"));
    let known = call(server.addr, Call::get("/whoami-or-guest").header("authorization", "Bearer bo")).await;
    assert_eq!(known.body, "bo");
    let basic = call(server.addr, Call::get("/whoami-or-guest").header("authorization", "Basic x")).await;
    assert_eq!(basic.status, 400, "present and wrong is not absent");
    server.stop().await.expect("closes clean");
}
