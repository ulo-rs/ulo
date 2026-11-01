//! `Cors` beyond the single conformance preflight: the allow-list echoes the matching origin,
//! credentials ride with it, a preflight carries the configured lists and `Max-Age`, and every
//! response, a miss included, is decorated and varies on `Origin`.

mod o1_support;

use std::time::Duration;

use http::{HeaderName, Method, StatusCode};
use o1_support::{Call, Running, refused, refusals, start};
use ulo::{ModuleDef, injectable, routes};
use ulo_http::{Cors, PreDispatch, Routing, WithHeaders};

const FIRST: &str = "https://first.example";
const SECOND: &str = "https://second.example";
const STRANGER: &str = "https://stranger.example";
/// `FIRST` in another case, and `FIRST` with more after it: an origin matches byte for byte.
const FIRST_UPPER: &str = "HTTPS://FIRST.EXAMPLE";
const FIRST_EXTENDED: &str = "https://first.example.evil";

#[injectable]
struct Pages;

#[routes]
impl Pages {
    #[ulo_http::get("/hit")]
    fn hit(&self) -> &'static str {
        "hit"
    }

    #[ulo_http::get("/varied")]
    fn varied(&self) -> WithHeaders<&'static str> {
        WithHeaders::new("varied").header("vary", "accept-encoding")
    }
}

fn list_config(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(
        Cors::new()
            .allow_origin(FIRST)
            .allow_origin(SECOND)
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([HeaderName::from_static("x-token"), HeaderName::from_static("content-type")])
            .expose_headers([HeaderName::from_static("x-total")])
            .max_age(Duration::from_secs(600)),
    );
}

fn credentials_config(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Cors::new().allow_origin(FIRST).allow_methods([Method::GET]).allow_credentials(true));
}

fn any_origin_config(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Cors::new().allow_any_origin().allow_methods([Method::GET]).allow_any_header());
}

fn any_origin_with_credentials(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Cors::new().allow_any_origin().allow_credentials(true));
}

fn unconfigured(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Cors::new());
}

fn preflight(path: &str, origin: &str) -> Call {
    Call::new(Method::OPTIONS, path).header("origin", origin).header("access-control-request-method", "GET")
}

#[tokio::test]
async fn the_allow_list_echoes_the_origin_that_matched_and_not_the_first_entry() {
    let app = start(list_config).await;

    for origin in [FIRST, SECOND] {
        let reply = app.send(Call::get("/hit").header("origin", origin)).await;
        assert_eq!(reply.status, StatusCode::OK, "{origin}");
        assert_eq!(reply.header("access-control-allow-origin"), Some(origin));
    }
    app.stop().await;
}

#[tokio::test]
async fn an_origin_outside_the_list_or_none_at_all_gets_no_allow_origin_but_still_varies_on_origin() {
    let app = start(list_config).await;

    let stranger = app.send(Call::get("/hit").header("origin", STRANGER)).await;
    let bare = app.send(Call::get("/hit")).await;
    let upper = app.send(Call::get("/hit").header("origin", FIRST_UPPER)).await;
    let extended = app.send(Call::get("/hit").header("origin", FIRST_EXTENDED)).await;
    for (what, reply) in [("a stranger", &stranger), ("no origin", &bare), ("another case", &upper), ("a longer origin", &extended)] {
        assert_eq!(reply.status, StatusCode::OK, "{what}: the request is answered, only undecorated");
        assert_eq!(reply.header("access-control-allow-origin"), None, "{what}");
        assert_eq!(reply.header("access-control-expose-headers"), None, "{what}");
        assert!(reply.tokens("vary").iter().any(|token| token.eq_ignore_ascii_case("origin")), "{what}: {:?}", reply.headers);
    }
    app.stop().await;
}

#[tokio::test]
async fn an_unconfigured_cors_admits_no_origin() {
    let app = start(unconfigured).await;

    let reply = app.send(Call::get("/hit").header("origin", FIRST)).await;
    assert_eq!(reply.header("access-control-allow-origin"), None);
    let pre = app.send(preflight("/hit", FIRST)).await;
    assert_eq!((pre.status, pre.header("access-control-allow-origin")), (StatusCode::NO_CONTENT, None));
    app.stop().await;
}

#[tokio::test]
async fn credentials_ride_with_the_echoed_origin_on_the_response_and_the_preflight_and_only_when_configured() {
    let with = start(credentials_config).await;
    let reply = with.send(Call::get("/hit").header("origin", FIRST)).await;
    assert_eq!(reply.header("access-control-allow-origin"), Some(FIRST));
    assert_eq!(reply.header("access-control-allow-credentials"), Some("true"));
    let pre = with.send(preflight("/hit", FIRST)).await;
    assert_eq!(pre.header("access-control-allow-origin"), Some(FIRST));
    assert_eq!(pre.header("access-control-allow-credentials"), Some("true"));
    let stranger = with.send(Call::get("/hit").header("origin", STRANGER)).await;
    assert_eq!(stranger.header("access-control-allow-credentials"), None, "credentials are not granted to an origin that is not allowed");
    with.stop().await;

    let without = start(list_config).await;
    let reply = without.send(Call::get("/hit").header("origin", FIRST)).await;
    assert_eq!(reply.header("access-control-allow-credentials"), None);
    without.stop().await;
}

#[tokio::test]
async fn any_origin_answers_a_star_and_with_credentials_listen_refuses_the_app() {
    let app: Running = start(any_origin_config).await;
    let reply = app.send(Call::get("/hit").header("origin", STRANGER)).await;
    assert_eq!(reply.header("access-control-allow-origin"), Some("*"));
    app.stop().await;

    let error = refused(any_origin_with_credentials).await;
    assert_eq!(refusals(&error), 1, "{error}");
}

#[tokio::test]
async fn a_preflight_carries_the_configured_lists_and_max_age_and_is_answered_before_routing() {
    let app = start(list_config).await;

    // `/nowhere` has no route: the preflight is answered ahead of routing.
    for path in ["/hit", "/nowhere"] {
        let reply = app.send(preflight(path, SECOND).header("access-control-request-headers", "x-token")).await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT, "{path}");
        assert!(reply.body.is_empty(), "{path}");
        assert_eq!(reply.header("access-control-allow-origin"), Some(SECOND), "{path}");
        assert_eq!(reply.tokens("access-control-allow-methods"), ["GET", "POST"], "{path}");
        assert_eq!(reply.tokens("access-control-allow-headers"), ["content-type", "x-token"], "{path}");
        assert_eq!(reply.header("access-control-max-age"), Some("600"), "{path}");
        assert_eq!(reply.header("access-control-expose-headers"), None, "{path}: expose lists belong to the actual response");
        assert!(matches!(reply.routing, Some(Routing::Unrouted)), "{path}: {:?}", reply.routing);
        let vary = reply.tokens("vary").into_iter().map(|token| token.to_ascii_lowercase()).collect::<Vec<_>>();
        for name in ["origin", "access-control-request-method", "access-control-request-headers"] {
            assert!(vary.contains(&name.to_owned()), "{path}: Vary lacks {name}: {vary:?}");
        }
    }
    app.stop().await;
}

#[tokio::test]
async fn a_preflight_from_an_origin_outside_the_list_is_answered_204_with_nothing_granted() {
    let app = start(list_config).await;

    let reply = app.send(preflight("/hit", STRANGER)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    for name in ["access-control-allow-origin", "access-control-allow-methods", "access-control-allow-headers", "access-control-max-age"] {
        assert_eq!(reply.header(name), None, "{name}");
    }
    let vary: Vec<String> = reply.tokens("vary").into_iter().map(|token| token.to_ascii_lowercase()).collect();
    assert_eq!(vary, ["access-control-request-headers", "access-control-request-method", "origin"], "a refusal varies as a grant does");
    app.stop().await;
}

#[tokio::test]
async fn allow_any_header_reflects_what_the_preflight_asked_for() {
    let app = start(any_origin_config).await;

    let asked = "x-one, x-two";
    let reply = app.send(preflight("/hit", FIRST).header("access-control-request-headers", asked)).await;
    assert_eq!(reply.header("access-control-allow-headers"), Some(asked));
    let none = app.send(preflight("/hit", FIRST)).await;
    assert_eq!(none.header("access-control-allow-headers"), None, "nothing asked, nothing reflected");
    app.stop().await;
}

#[tokio::test]
async fn an_options_with_an_origin_and_no_requested_method_is_not_a_preflight() {
    let app = start(list_config).await;

    let reply = app.send(Call::new(Method::OPTIONS, "/hit").header("origin", FIRST)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(matches!(reply.routing, Some(Routing::Options { .. })), "the router answered it: {:?}", reply.routing);
    assert!(reply.header("allow").is_some(), "the router's Allow");
    assert_eq!(reply.header("access-control-allow-methods"), None, "no preflight answer");
    assert_eq!(reply.header("access-control-allow-origin"), Some(FIRST), "decorated like any response");
    app.stop().await;
}

#[tokio::test]
async fn an_actual_response_is_decorated_with_exposed_headers_and_so_are_a_miss_and_a_405() {
    let app = start(list_config).await;

    let hit = app.send(Call::get("/hit").header("origin", FIRST)).await;
    assert_eq!(hit.tokens("access-control-expose-headers"), ["x-total"]);
    assert_eq!(hit.header("access-control-max-age"), None, "max-age belongs to the preflight");
    let varied = app.send(Call::get("/varied").header("origin", FIRST)).await;
    assert_eq!(varied.tokens("vary"), ["accept-encoding", "origin"], "the handler's `Vary` is kept and `Origin` added to it");

    let miss = app.send(Call::get("/nowhere").header("origin", FIRST)).await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);
    assert_eq!(miss.header("access-control-allow-origin"), Some(FIRST));

    let refused = app.send(Call::new(Method::DELETE, "/hit").header("origin", FIRST)).await;
    assert_eq!(refused.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(refused.header("access-control-allow-origin"), Some(FIRST));
    app.stop().await;
}
