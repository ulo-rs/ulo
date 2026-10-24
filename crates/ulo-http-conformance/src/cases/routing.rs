//! Scenarios: routing.

use http::Method;

use crate::wire::{Exchange, is_reference, same_as_reference, start};
use crate::{Host, Mode};

/// A request a route matches, answered by its handler.
pub async fn hit<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/hit")).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.text(), "hit");
    host.stop().await;
}

/// A path nothing matches: 404 with `NoRoute`, `Routing::NotFound`.
pub async fn not_found<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/nowhere")).await;
    assert_eq!(reply.status, 404);
    assert!(reply.header("content-type").is_some_and(|value| value.starts_with("application/problem+json")));
    if !is_reference::<H>() {
        assert_eq!(reply.routing(), Some("not-found"));
    }
    host.stop().await;
}

/// A matching path with the wrong method: 405 with `Allow`.
pub async fn method_not_allowed<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::new(Method::DELETE, "/hit")).await;
    assert_eq!(reply.status, 405);
    assert!(reply.header("allow").is_some_and(|allow| allow.contains("GET")), "405 without `Allow: GET`");
    if !is_reference::<H>() {
        assert_eq!(reply.routing(), Some("method-not-allowed"));
    }
    host.stop().await;
}

/// `OPTIONS` without a handler: 204 with `Allow`, `Routing::Options`.
pub async fn options<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::new(Method::OPTIONS, "/hit")).await;
    assert_eq!(reply.status, 204);
    assert!(reply.header("allow").is_some_and(|allow| allow.contains("GET")), "204 without `Allow: GET`");
    if !is_reference::<H>() {
        assert_eq!(reply.routing(), Some(format!("options {}", host.route("/hit")).as_str()));
    }
    host.stop().await;
}

/// `HEAD` answered from the `GET` handler, body omitted.
pub async fn head<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::new(Method::HEAD, "/hit")).await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.is_empty(), "a HEAD answer carries no body");
    host.stop().await;
}

/// A path parameter is percent-decoded once, after the path is split: `a%20b%2Fc%2520` is
/// `a b/c%20` in one segment, `%2F` not a separator and `%25` not decoded a second time, as the
/// reference host reads it.
pub async fn percent_decoded<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let reply = same_as_reference(&host, Exchange::get("/echo/a%20b%2Fc%2520")).await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.text(), "a b/c%20", "the parameter is decoded once, and an encoded slash stays inside its segment");
    if !is_reference::<H>() {
        assert_eq!(reply.routing(), Some(format!("matched {}", host.route("/echo/{name}")).as_str()));
    }
    host.stop().await;
}

/// A trailing slash is insignificant: `/hit/` reaches the handler `/hit` does, answered the same
/// and recorded as the same route.
pub async fn trailing_slash<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    for path in ["/hit", "/hit/"] {
        let reply = same_as_reference(&host, Exchange::get(path)).await;
        assert_eq!((reply.status, reply.text()), (200, "hit".to_owned()), "GET {path}");
        if !is_reference::<H>() {
            assert_eq!(reply.routing(), Some(format!("matched {}", host.route("/hit")).as_str()), "GET {path}");
        }
    }
    host.stop().await;
}
