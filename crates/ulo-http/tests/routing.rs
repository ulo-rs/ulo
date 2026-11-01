//! Routing as every backend sees it: a trailing slash is insignificant, `{name}` and `{*rest}`
//! parameters are decoded but for `%2F` and typed, a path with a `.` or `..` segment is refused
//! before routing, a static segment outranks a parameter, one path carries one
//! handler per verb, and a route table that cannot be built is refused by `listen()` naming the
//! controllers involved.

mod o1_support;

use http::{Method, StatusCode};
use o1_support::{Call, Running, refusal_names, refusals, refused, start};
use ulo::{ModuleDef, TypeName, injectable, routes};
use ulo_http::{HttpCx, Path, Routing};

#[injectable]
struct Pages;

#[routes]
impl Pages {
    #[ulo_http::get("/a")]
    fn a(&self) -> &'static str {
        "a"
    }

    #[ulo_http::get("/")]
    fn index(&self) -> &'static str {
        "index"
    }

    /// The value as `Path` parsed it, as `HttpCx::param` read it, and the pattern that matched.
    #[ulo_http::get("/users/{id}")]
    fn user(&self, cx: HttpCx, Path(id): Path<u64>) -> String {
        format!("{id}|{}|{}", cx.param("id").unwrap_or("-"), cx.route().unwrap_or("-"))
    }

    /// The value as `Path` read it, then as `HttpCx::param` read it.
    #[ulo_http::get("/names/{name}")]
    fn named(&self, cx: HttpCx, Path(name): Path<String>) -> String {
        format!("{name}|{}", cx.param("name").unwrap_or("-"))
    }

    #[ulo_http::get("/files/{name}")]
    fn files_by_name(&self, Path(name): Path<String>) -> String {
        format!("param:{name}")
    }

    #[ulo_http::get("/files/new")]
    fn files_new(&self) -> &'static str {
        "static"
    }

    #[ulo_http::get("/assets/{*rest}")]
    fn assets(&self, Path(rest): Path<String>) -> String {
        rest
    }

    #[ulo_http::get("/shared")]
    fn shared_get(&self) -> &'static str {
        "get"
    }

    #[ulo_http::post("/shared")]
    fn shared_post(&self) -> &'static str {
        "post"
    }
}

#[injectable]
struct Versioned;

#[routes]
impl Versioned {
    #[ulo_http::get("/items/{id}")]
    fn item(&self, cx: HttpCx) -> String {
        cx.route().unwrap_or("-").to_owned()
    }
}

fn pages(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.controller::<Versioned>().at("/v1/");
}

async fn running() -> Running {
    start(pages).await
}

#[tokio::test]
async fn a_trailing_slash_reaches_the_same_handler_as_none() {
    let app = running().await;

    for path in ["/a", "/a/"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, "a".to_owned()), "{path}");
        assert_eq!(reply.matched(), Some(("/a".to_owned(), "a")), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn the_root_path_is_not_emptied_by_the_slash_rule() {
    let app = running().await;

    let reply = app.send(Call::get("/")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "index".to_owned()));
    assert_eq!(reply.matched(), Some(("/".to_owned(), "index")));
    app.stop().await;
}

#[tokio::test]
async fn a_parameter_is_parsed_named_and_matched_to_its_pattern_with_or_without_a_trailing_slash() {
    let app = running().await;

    for path in ["/users/42", "/users/42/"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, "42|42|/users/{id}".to_owned()), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_parameter_that_does_not_parse_is_a_bad_request_and_an_empty_one_matches_no_route() {
    let app = running().await;

    let reply = app.send(Call::get("/users/abc")).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.matched(), Some(("/users/{id}".to_owned(), "user")), "the route matched and its extractor refused");

    // `/users/` is `/users` once the slash is dropped: no segment for `{id}`.
    let reply = app.send(Call::get("/users/")).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.matched(), None);
    app.stop().await;
}

#[tokio::test]
async fn a_parameter_is_percent_decoded_except_for_an_encoded_slash_which_stays_as_sent() {
    let app = running().await;

    // The handler receives the three characters `%2F`, in the case the client wrote them, and
    // never a `/`, so a value from one segment holds no separator.
    for (sent, value) in [
        ("a%20b%2Fc", "a b%2Fc"),
        ("a%2fb", "a%2fb"),
        ("..%2F..%2Fetc%2Fpasswd", "..%2F..%2Fetc%2Fpasswd"),
        // `%25` decodes once, so `%252F` reaches the handler as the same text as `%2F`.
        ("a%252Fb", "a%2Fb"),
    ] {
        let reply = app.send(Call::get(&format!("/names/{sent}"))).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, format!("{value}|{value}")), "/names/{sent}");
        assert_eq!(reply.matched(), Some(("/names/{name}".to_owned(), "named")), "/names/{sent}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_rest_parameter_keeps_an_encoded_slash_so_each_slash_in_it_is_one_the_client_sent() {
    let app = running().await;

    let reply = app.send(Call::get("/assets/a%2Fb/c%20d")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "a%2Fb/c d".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn a_dot_segment_raw_or_encoded_is_refused_400_before_routing() {
    let app = running().await;

    for path in [
        "/assets/..",
        "/assets/a/../b",
        "/assets/./b",
        "/assets/%2e%2e/b",
        "/assets/%2E%2E/b",
        "/assets/.%2e/b",
        "/assets/%2e./b",
        "/assets/%2e/b",
        "/names/..",
        "/../a",
        "/nowhere/../a",
        "/assets/a/../",
    ] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{path}");
        assert!(reply.header("content-type").is_some_and(|value| value.starts_with("application/problem+json")), "{path}");
        assert_eq!(reply.json()["detail"], "the path holds a `.` or `..` segment", "{path}");
        assert!(matches!(reply.routing, Some(Routing::Unrouted)), "{path}: answered before routing, {:?}", reply.routing);
    }
    app.stop().await;
}

#[tokio::test]
async fn a_segment_that_only_contains_dots_or_a_twice_encoded_dot_is_routed() {
    let app = running().await;

    for (path, value) in [
        ("/assets/.../b", ".../b"),
        ("/assets/a./b", "a./b"),
        ("/assets/.hidden", ".hidden"),
        // Decoded once, `%252e` is the text `%2e`, which names no dot segment at this layer.
        ("/assets/%252e%252e/b", "%2e%2e/b"),
    ] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, value.to_owned()), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_static_segment_outranks_a_parameter_whatever_order_they_were_declared_in() {
    let app = running().await;

    let reply = app.send(Call::get("/files/new")).await;
    assert_eq!(reply.matched(), Some(("/files/new".to_owned(), "files_new")));
    let reply = app.send(Call::get("/files/old")).await;
    assert_eq!(reply.matched(), Some(("/files/{name}".to_owned(), "files_by_name")));
    assert_eq!(reply.text(), "param:old");
    app.stop().await;
}

#[tokio::test]
async fn a_rest_parameter_takes_the_remaining_segments_decoded_and_needs_at_least_one() {
    let app = running().await;

    let reply = app.send(Call::get("/assets/css/site%20v2.css")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "css/site v2.css".to_owned()));
    assert_eq!(reply.matched(), Some(("/assets/{*rest}".to_owned(), "assets")));

    for path in ["/assets", "/assets/"] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}: no segment for `{{*rest}}`");
    }
    app.stop().await;
}

#[tokio::test]
async fn one_path_answers_each_verb_with_its_own_handler_and_lists_them_in_allow_for_the_rest() {
    let app = running().await;

    let get = app.send(Call::get("/shared")).await;
    assert_eq!((get.text(), get.matched()), ("get".to_owned(), Some(("/shared".to_owned(), "shared_get"))));
    let post = app.send(Call::new(Method::POST, "/shared")).await;
    assert_eq!((post.text(), post.matched()), ("post".to_owned(), Some(("/shared".to_owned(), "shared_post"))));

    // One value in declaration order, `HEAD` beside the `GET` that answers it, `OPTIONS` last.
    let expected = ["GET, HEAD, POST, OPTIONS"];
    let put = app.send(Call::new(Method::PUT, "/shared")).await;
    assert_eq!(put.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(put.values("allow"), expected);
    let options = app.send(Call::new(Method::OPTIONS, "/shared")).await;
    assert_eq!(options.status, StatusCode::NO_CONTENT);
    assert_eq!(options.values("allow"), expected);
    app.stop().await;
}

#[tokio::test]
async fn a_controller_prefix_joins_the_route_whatever_slashes_either_side_carries() {
    let app = running().await;

    let reply = app.send(Call::get("/v1/items/9")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "/v1/items/{id}".to_owned()));
    let unprefixed = app.send(Call::get("/items/9")).await;
    assert_eq!(unprefixed.status, StatusCode::NOT_FOUND);
    app.stop().await;
}

#[injectable]
struct SlashFirst;

#[routes]
impl SlashFirst {
    #[ulo_http::get("/dup")]
    fn dup(&self) -> &'static str {
        "first"
    }
}

#[injectable]
struct SlashLast;

#[routes]
impl SlashLast {
    #[ulo_http::get("/dup/")]
    fn dup(&self) -> &'static str {
        "second"
    }
}

fn same_route_up_to_a_slash(m: &mut ModuleDef<'_>) {
    m.controller::<SlashFirst>();
    m.controller::<SlashLast>();
}

#[tokio::test]
async fn two_routes_equal_up_to_a_trailing_slash_are_refused_naming_both_controllers() {
    let error = refused(same_route_up_to_a_slash).await;

    assert_eq!(refusals(&error), 1, "one transport refused: {error}");
    let names = refusal_names(&error);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&TypeName::of::<SlashFirst>()), "{names:?}");
    assert!(names.contains(&TypeName::of::<SlashLast>()), "{names:?}");
}

#[injectable]
struct ParamA;

#[routes]
impl ParamA {
    #[ulo_http::get("/k/{a}")]
    fn by_a(&self) -> &'static str {
        "a"
    }
}

#[injectable]
struct ParamB;

#[routes]
impl ParamB {
    #[ulo_http::get("/k/{b}")]
    fn by_b(&self) -> &'static str {
        "b"
    }
}

fn same_route_other_names(m: &mut ModuleDef<'_>) {
    m.controller::<ParamA>();
    m.controller::<ParamB>();
}

#[tokio::test]
async fn two_routes_differing_only_in_a_parameter_name_are_refused_naming_both_controllers() {
    let error = refused(same_route_other_names).await;

    assert_eq!(refusals(&error), 1, "{error}");
    let names = refusal_names(&error);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&TypeName::of::<ParamA>()), "{names:?}");
    assert!(names.contains(&TypeName::of::<ParamB>()), "{names:?}");
}

#[injectable]
struct Mismatched;

#[routes]
impl Mismatched {
    #[ulo_http::get("/one/{id}")]
    fn pair(&self, Path((_first, _second)): Path<(u64, u64)>) -> &'static str {
        "never"
    }
}

fn path_type_does_not_fit_the_route(m: &mut ModuleDef<'_>) {
    m.controller::<Mismatched>();
}

#[tokio::test]
async fn a_path_extractor_that_does_not_fit_its_route_is_refused_naming_the_controller_and_the_type() {
    let error = refused(path_type_does_not_fit_the_route).await;

    assert_eq!(refusals(&error), 1, "{error}");
    let names = refusal_names(&error);
    assert!(names.contains(&TypeName::of::<Mismatched>()), "{names:?}");
    assert!(names.contains(&TypeName::of::<(u64, u64)>()), "{names:?}");
}
