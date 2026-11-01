use std::sync::atomic::Ordering;

use ex_17_http_pre_dispatch::{Hits, builder};
use ex_support::http::{Call, call};
use ex_support::{Served, start};
use ulo::Dep;

const APP: &str = "https://app.example";

async fn app() -> (Served, Dep<Hits>) {
    let server = start(builder()).await;
    let hits: Dep<Hits> = server.handle.get().await.expect("the counter is bound");
    (server, hits)
}

#[tokio::test]
async fn a_preflight_is_answered_before_routing_with_the_allowed_origin_and_methods() {
    let (server, hits) = app().await;

    // `/nowhere` has no route: only an entry that runs before routing can answer it.
    let preflight = call(
        server.addr,
        Call::new("OPTIONS", "/nowhere").header("origin", APP).header("access-control-request-method", "GET"),
    )
    .await;
    assert_eq!(preflight.status, 204);
    assert_eq!(preflight.header("access-control-allow-origin"), Some(APP));
    assert_eq!(preflight.header("access-control-allow-methods"), Some("GET"));
    assert_eq!(hits.count.load(Ordering::SeqCst), 0, "the API-key entry never ran");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_response_varies_on_origin_and_echoes_the_origin_only_when_it_is_allowed() {
    let (server, _) = app().await;

    let allowed = call(server.addr, Call::get("/").header("origin", APP)).await;
    assert_eq!((allowed.status, allowed.body.as_str()), (200, "public"));
    assert_eq!(allowed.header("access-control-allow-origin"), Some(APP));
    assert_eq!(allowed.header("vary"), Some("origin"));

    let stranger = call(server.addr, Call::get("/").header("origin", "https://elsewhere.example")).await;
    assert_eq!(stranger.status, 200, "the request is served, only undecorated");
    assert_eq!(stranger.header("access-control-allow-origin"), None);
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn the_scoped_entry_guards_admin_routes_and_skips_the_excluded_one() {
    let (server, hits) = app().await;

    let refused = call(server.addr, Call::get("/admin/panel")).await;
    assert_eq!(refused.status, 401);
    assert_eq!(refused.header("www-authenticate"), Some("ApiKey"));
    assert_eq!(hits.count.load(Ordering::SeqCst), 1);

    let admitted = call(server.addr, Call::get("/admin/panel").header("x-api-key", "sesame")).await;
    assert_eq!((admitted.status, admitted.body.as_str()), (200, "panel"));
    assert_eq!(hits.count.load(Ordering::SeqCst), 2);

    let health = call(server.addr, Call::get("/admin/health")).await;
    assert_eq!((health.status, health.body.as_str()), (200, "ok"));
    let public = call(server.addr, Call::get("/")).await;
    assert_eq!(public.status, 200);
    assert_eq!(hits.count.load(Ordering::SeqCst), 2, "neither the excluded route nor a route outside `/admin/*` ran the entry");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn the_tower_layer_stamps_every_answer_including_a_refusal_a_miss_and_a_preflight() {
    let (server, _) = app().await;

    for (path, key, status) in [("/", None, 200), ("/admin/panel", Some("sesame"), 200), ("/admin/panel", None, 401), ("/nowhere", None, 404)] {
        let request = key.map_or_else(|| Call::get(path), |key| Call::get(path).header("x-api-key", key));
        let reply = call(server.addr, request).await;
        assert_eq!(reply.status, status, "{path}");
        assert_eq!(reply.header("x-served-by"), Some("ulo"), "{path}");
    }
    let preflight = call(server.addr, Call::new("OPTIONS", "/nowhere").header("origin", APP).header("access-control-request-method", "GET")).await;
    assert_eq!((preflight.status, preflight.header("x-served-by")), (204, Some("ulo")), "the preflight CORS answers");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn an_entrys_value_reaches_the_handler_and_its_absence_is_a_500_that_names_no_type() {
    let (server, _) = app().await;

    let present = call(server.addr, Call::get("/tenant").header("x-tenant", "acme")).await;
    assert_eq!((present.status, present.body.as_str()), (200, "acme"));

    let absent = call(server.addr, Call::get("/tenant")).await;
    assert_eq!(absent.status, 500, "a deployment fault, not a bad request");
    assert!(!absent.body.contains("Tenant"), "the type stays server-side: {}", absent.body);

    let optional = call(server.addr, Call::get("/tenant-or-none")).await;
    assert_eq!((optional.status, optional.body.as_str()), (200, "none"));
    let given = call(server.addr, Call::get("/tenant-or-none").header("x-tenant", "initech")).await;
    assert_eq!(given.body, "initech");
    server.stop().await.expect("closes clean");
}
