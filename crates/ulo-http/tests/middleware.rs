//! The pre-dispatch stage: unscoped entries run in the order written and see misses, may rewrite
//! the path before routing, answer without calling `next` or fail into the error chain; scoped
//! entries run for the request paths their pattern matches minus the exclusions, routed or not,
//! a path with a dot segment reaches no entry, and a declaration the stage cannot build is
//! refused by `listen()` at the call that wrote it.

mod o1_support;

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use http::{HeaderMap, HeaderValue, StatusCode};
use o1_support::{Call, Reply, Running, refusal_entries, refused, start};
use ulo::{AnyErrorHandler, BoxError, BoxFuture, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::middleware::{Middleware, Next};
use ulo_http::{Cors, Http, HttpBody, HttpCx, PreDispatch, Request, Response, Routing, UpgradeHandler, Upgrades};

const ORIGIN: &str = "https://app.example";
use ulo_transport::{CallError, ErrorKind};

/// Appends its name to the request's `x-trail`, so the handler reads the order entries ran in.
struct Tag(&'static str);

impl Middleware for Tag {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        req.head.headers.append("x-trail", HeaderValue::from_static(self.0));
        next.run(req).await
    }
}

/// Sets `x-stamp: stamped` on the response it passes.
#[injectable]
struct Stamp;

impl Middleware for Stamp {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        let mut response = next.run(req).await?;
        response.headers_mut().insert("x-stamp", HeaderValue::from_static("stamped"));
        Ok(response)
    }
}

/// Rewrites `/legacy` to `/current`.
struct Rewrite;

impl Middleware for Rewrite {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if req.path() == "/legacy" {
            req.set_path("/current")?;
        }
        next.run(req).await
    }
}

/// Answers 418 itself when the request carries `x-gate: shut`.
struct Gate;

impl Middleware for Gate {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if req.headers().get("x-gate").is_some_and(|value| value == "shut") {
            let mut response = Response::new(HttpBody::from("gate answered"));
            *response.status_mut() = StatusCode::IM_A_TEAPOT;
            return Ok(response);
        }
        next.run(req).await
    }
}

/// Fails with the error `x-fail` names.
struct Fail;

impl Middleware for Fail {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        match req.headers().get("x-fail").and_then(|value| value.to_str().ok()) {
            Some("unauthorized") => Err(CallError::unauthorized("Basic realm=\"o1\"").into()),
            Some("forbidden") => Err(CallError::new(ErrorKind::Forbidden, "no").into()),
            Some("unclassified") => Err("a plain error".into()),
            _ => next.run(req).await,
        }
    }
}

/// Answers every error with 200 and the kind it arrived as.
struct Claim;

impl ErrorHandler<Http> for Claim {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let kind = CallError::from_boxed(err).kind();
        Ok(Response::new(HttpBody::from(kind.as_str())))
    }
}

#[injectable]
struct Pages;

#[routes]
impl Pages {
    #[ulo_http::get("/trail")]
    fn trail(&self, headers: HeaderMap) -> String {
        headers.get_all("x-trail").iter().filter_map(|value| value.to_str().ok()).collect::<Vec<_>>().join(",")
    }

    #[ulo_http::get("/trail-quiet")]
    fn trail_quiet(&self, headers: HeaderMap) -> String {
        headers.get_all("x-trail").iter().filter_map(|value| value.to_str().ok()).collect::<Vec<_>>().join(",")
    }

    #[ulo_http::get("/public")]
    fn public(&self) -> &'static str {
        "public"
    }

    #[ulo_http::get("/admin/users")]
    fn users(&self) -> &'static str {
        "users"
    }

    #[ulo_http::get("/admin/users/{id}")]
    fn user(&self) -> &'static str {
        "user"
    }

    #[ulo_http::get("/admin/health")]
    fn health(&self) -> &'static str {
        "health"
    }

    #[ulo_http::get("/admin")]
    fn admin_root(&self) -> &'static str {
        "admin"
    }

    #[ulo_http::get("/current")]
    fn current(&self) -> &'static str {
        "current"
    }
}

fn ordered(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Tag("one")).apply_value(Tag("two")).apply_value(Tag("three"));
}

fn ordered_with_an_exclude(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Tag("one")).apply_value(Tag("two")).exclude(["/trail-quiet"]).apply_value(Tag("three"));
}

fn unscoped_by_type(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    m.meta::<PreDispatch>().apply::<Stamp>().exclude(["/public"]);
}

fn scoped_with_exclusion(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    m.meta::<PreDispatch>().apply_for::<Stamp>(["/admin/*"]).exclude(["/admin/health"]);
}

fn rewriting(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Rewrite);
}

fn gated(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Gate);
}

fn failing(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().apply_value(Fail);
}

fn failing_with_a_claiming_handler(m: &mut ModuleDef<'_>) {
    failing(m);
    let claim: Arc<AnyErrorHandler<Http>> = Arc::new(Claim);
    m.enhancer::<AnyErrorHandler<Http>>().value(claim);
}

async fn stamped(app: &Running, path: &str) -> (StatusCode, bool) {
    let reply = app.send(Call::get(path)).await;
    (reply.status, reply.header("x-stamp") == Some("stamped"))
}

#[tokio::test]
async fn unscoped_entries_run_in_the_order_written() {
    let app = start(ordered).await;

    let reply = app.send(Call::get("/trail")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "one,two,three".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn an_exclude_removes_its_paths_from_the_entry_written_just_before_it_only() {
    let app = start(ordered_with_an_exclude).await;

    let quiet = app.send(Call::get("/trail-quiet")).await;
    assert_eq!((quiet.status, quiet.text()), (StatusCode::OK, "one,three".to_owned()));
    let trail = app.send(Call::get("/trail")).await;
    assert_eq!(trail.text(), "one,two,three", "a path the exclude does not name passes every entry");
    app.stop().await;
}

#[tokio::test]
async fn an_unscoped_entry_sees_a_miss_and_a_405_and_skips_a_path_it_excludes() {
    let app = start(unscoped_by_type).await;

    assert_eq!(stamped(&app, "/admin/users").await, (StatusCode::OK, true));
    assert_eq!(stamped(&app, "/nowhere").await, (StatusCode::NOT_FOUND, true), "a miss still passes the entry");
    let refused = app.send(Call::new(http::Method::DELETE, "/admin/users")).await;
    assert_eq!((refused.status, refused.header("x-stamp")), (StatusCode::METHOD_NOT_ALLOWED, Some("stamped")));
    assert_eq!(stamped(&app, "/public").await, (StatusCode::OK, false), "the excluded path skips the entry");
    app.stop().await;
}

#[tokio::test]
async fn a_scoped_entry_runs_for_the_paths_its_pattern_matches_and_not_the_excluded_one() {
    let app = start(scoped_with_exclusion).await;

    assert_eq!(stamped(&app, "/admin/users").await, (StatusCode::OK, true));
    assert_eq!(stamped(&app, "/admin/users/7").await, (StatusCode::OK, true), "`/admin/*` covers a parameter route below it");
    assert_eq!(stamped(&app, "/admin/health").await, (StatusCode::OK, false), "excluded");
    assert_eq!(stamped(&app, "/admin").await, (StatusCode::OK, false), "`/admin/*` needs a segment after `/admin`");
    assert_eq!(stamped(&app, "/public").await, (StatusCode::OK, false), "outside the scope");
    assert_eq!(stamped(&app, "/admin/nowhere").await, (StatusCode::NOT_FOUND, true), "the scope matches the path, routed or not");
    app.stop().await;
}

/// Refuses with 401 unless the request carries `x-key: open`, and marks what it passes with
/// `x-auth: checked`.
#[injectable]
struct ApiKeyAuth;

impl Middleware for ApiKeyAuth {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if req.headers().get("x-key").is_none_or(|key| key != "open") {
            return Err(CallError::unauthorized("ApiKey").into());
        }
        let mut response = next.run(req).await?;
        response.headers_mut().insert("x-auth", HeaderValue::from_static("checked"));
        Ok(response)
    }
}

/// Answers every upgrade request on `/admin/socket` with 200 `taken`.
struct Taker;

impl UpgradeHandler for Taker {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/admin/socket".into()]
    }

    fn upgrade(&self, _req: Request) -> BoxFuture<'static, Response> {
        Box::pin(async { Response::new(HttpBody::from("taken")) })
    }
}

fn admin_behind_a_key(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    m.provide::<ApiKeyAuth>();
    m.meta::<Upgrades>().register(Taker);
    m.meta::<PreDispatch>()
        .apply_value(Cors::new().allow_origin(ORIGIN))
        .apply::<Stamp>()
        .apply_for::<ApiKeyAuth>(["/admin/*"])
        .exclude(["/admin/health"]);
}

const GET: &str = "GET";
const DELETE: &str = "DELETE";
const OPTIONS: &str = "OPTIONS";
/// A `GET` carrying `Upgrade: websocket`.
const UPGRADE: &str = "UPGRADE";

fn call(method: &str, path: &str) -> Call {
    match method {
        UPGRADE => Call::get(path).header("upgrade", "websocket"),
        _ => Call::new(method.parse().expect("a method name"), path),
    }
}

async fn as_anonymous(app: &Running, call: Call) -> Reply {
    app.send(call.header("origin", ORIGIN)).await
}

#[tokio::test]
async fn a_scoped_entry_answers_an_unrouted_path_under_its_scope_as_it_answers_a_routed_one() {
    let app = start(admin_behind_a_key).await;

    // Anonymous: the routed path, a path no route matches, a method no handler takes, an
    // `OPTIONS` with no handler and an upgrade all answer the same 401, so the status tells no
    // route from none.
    for (method, path) in [
        (GET, "/admin/users"),
        (GET, "/admin/nowhere"),
        (GET, "/admin/users/7/deeper"),
        (GET, "/admin/healthz"),
        (DELETE, "/admin/users"),
        (OPTIONS, "/admin/users"),
        (UPGRADE, "/admin/socket"),
    ] {
        let what = format!("{method} {path}");
        let reply = as_anonymous(&app, call(method, path)).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{what}");
        assert_eq!(reply.header("www-authenticate"), Some("ApiKey"), "{what}");
        assert_eq!(reply.header("x-auth"), None, "{what}");
        assert_eq!(reply.header("access-control-allow-origin"), Some(ORIGIN), "{what}: the outer CORS entry decorates the 401");
        assert_eq!(reply.header("x-stamp"), Some("stamped"), "{what}: the unscoped entry before it ran");
    }

    // With the key, each is answered as routing decides, the scoped entry having passed it.
    for (method, path, status) in [
        (GET, "/admin/users", StatusCode::OK),
        (GET, "/admin/nowhere", StatusCode::NOT_FOUND),
        (DELETE, "/admin/users", StatusCode::METHOD_NOT_ALLOWED),
        (OPTIONS, "/admin/users", StatusCode::NO_CONTENT),
        (UPGRADE, "/admin/socket", StatusCode::OK),
    ] {
        let what = format!("{method} {path}");
        let reply = app.send(call(method, path).header("x-key", "open")).await;
        assert_eq!((reply.status, reply.header("x-auth")), (status, Some("checked")), "{what}");
    }

    // The exclusion and the paths outside the scope pass without a key.
    for (path, status) in [("/admin/health", StatusCode::OK), ("/admin", StatusCode::OK), ("/nowhere", StatusCode::NOT_FOUND)] {
        let reply = as_anonymous(&app, Call::get(path)).await;
        assert_eq!((reply.status, reply.header("x-auth")), (status, None), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_dot_segment_is_refused_before_any_entry_so_it_cannot_step_into_or_out_of_a_scope() {
    let app = start(admin_behind_a_key).await;

    // Read as written, `/public/../admin/users` is outside `/admin/*`; normalized, it is
    // `/admin/users`. Refused before the stage, it reaches neither the router nor any entry, and
    // `/admin/../public` cannot step out of the scope either.
    for path in ["/public/../admin/users", "/public/%2e%2e/admin/users", "/admin/../public", "/admin/%2E%2E/public", "/admin/./users"] {
        for key in [None, Some("open")] {
            let mut call = Call::get(path).header("origin", ORIGIN);
            if let Some(key) = key {
                call = call.header("x-key", key);
            }
            let reply = app.send(call).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{path} with key {key:?}");
            assert_eq!(reply.json()["detail"], "the path holds a `.` or `..` segment", "{path}");
            assert_eq!(
                (reply.header("x-stamp"), reply.header("x-auth"), reply.header("access-control-allow-origin")),
                (None, None, None),
                "{path}: no entry, scoped or unscoped, ran"
            );
            assert!(matches!(reply.routing, Some(Routing::Unrouted)), "{path}: {:?}", reply.routing);
        }
    }
    app.stop().await;
}

/// Rewrites `/into-admin` to `/public/../admin/users`.
struct DotRewrite;

impl Middleware for DotRewrite {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        if req.path() == "/into-admin" {
            req.set_path("/public/../admin/users")?;
        }
        next.run(req).await
    }
}

fn rewriting_into_a_dot_segment(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<ApiKeyAuth>();
    m.meta::<PreDispatch>().apply_value(DotRewrite).apply_for::<ApiKeyAuth>(["/admin/*"]);
}

#[tokio::test]
async fn a_path_an_unscoped_entry_rewrites_into_a_dot_segment_is_refused_at_routing() {
    let app = start(rewriting_into_a_dot_segment).await;

    let reply = app.send(Call::get("/into-admin")).await;
    assert_eq!((reply.status, reply.header("x-auth")), (StatusCode::BAD_REQUEST, None));
    assert_eq!(reply.json()["detail"], "the path holds a `.` or `..` segment");
    app.stop().await;
}

fn one_user_behind_a_key(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<ApiKeyAuth>();
    m.meta::<PreDispatch>()
        .apply_for::<ApiKeyAuth>(["/admin/users/me"])
        .apply_for::<ApiKeyAuth>(["/admin/users/*"])
        .exclude(["/admin/users/7"]);
}

#[tokio::test]
async fn a_scope_narrower_than_the_route_it_shares_paths_with_decides_per_request() {
    // `/admin/users/me` and the exclusion `/admin/users/7` each match some paths of
    // `/admin/users/{id}` and not others: the app starts, and each request's path decides.
    let app = start(one_user_behind_a_key).await;

    for (path, status) in [
        ("/admin/users/me", StatusCode::UNAUTHORIZED),
        ("/admin/users/7", StatusCode::OK),
        ("/admin/users/8", StatusCode::UNAUTHORIZED),
        ("/admin/users/7/x", StatusCode::UNAUTHORIZED),
        ("/admin/users", StatusCode::OK),
    ] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, status, "{path}");
    }
    let reply = app.send(Call::get("/admin/users/me").header("x-key", "open")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "user".to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn an_unscoped_rewrite_changes_which_route_matches() {
    let app = start(rewriting).await;

    let reply = app.send(Call::get("/legacy")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "current".to_owned()));
    assert_eq!(reply.matched(), Some(("/current".to_owned(), "current")), "the route that matched is the rewritten one");
    app.stop().await;
}

#[tokio::test]
async fn an_entry_that_answers_without_calling_next_reaches_no_handler() {
    let app = start(gated).await;

    let shut = app.send(Call::get("/public").header("x-gate", "shut")).await;
    assert_eq!((shut.status, shut.text()), (StatusCode::IM_A_TEAPOT, "gate answered".to_owned()));
    assert!(matches!(shut.routing, Some(Routing::Unrouted)), "{:?}", shut.routing);
    let open = app.send(Call::get("/public")).await;
    assert_eq!((open.status, open.matched()), (StatusCode::OK, Some(("/public".to_owned(), "public"))));
    app.stop().await;
}

#[tokio::test]
async fn an_entry_that_fails_is_rendered_by_the_kind_it_failed_with() {
    let app = start(failing).await;

    let unauthorized = app.send(Call::get("/public").header("x-fail", "unauthorized")).await;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthorized.header("www-authenticate"), Some("Basic realm=\"o1\""), "the error's own challenge");

    let forbidden = app.send(Call::get("/public").header("x-fail", "forbidden")).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    let unclassified = app.send(Call::get("/public").header("x-fail", "unclassified")).await;
    assert_eq!(unclassified.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!unclassified.text().contains("plain error"), "an unclassified error's text is withheld: {}", unclassified.text());
    app.stop().await;
}

#[tokio::test]
async fn an_entrys_failure_reaches_the_global_error_handler_with_its_kind() {
    let app = start(failing_with_a_claiming_handler).await;

    let forbidden = app.send(Call::get("/public").header("x-fail", "forbidden")).await;
    assert_eq!((forbidden.status, forbidden.text()), (StatusCode::OK, "forbidden".to_owned()));
    let unclassified = app.send(Call::get("/public").header("x-fail", "unclassified")).await;
    assert_eq!((unclassified.status, unclassified.text()), (StatusCode::OK, "internal".to_owned()));
    app.stop().await;
}

static STRAY_EXCLUDE: AtomicU32 = AtomicU32::new(0);
static EMPTY_SCOPE: AtomicU32 = AtomicU32::new(0);
static BAD_SCOPE: AtomicU32 = AtomicU32::new(0);
static STRAY_SUPPLIES: AtomicU32 = AtomicU32::new(0);
static ADOPT_SUPPLIES: AtomicU32 = AtomicU32::new(0);

/// A value a `supplies` declares.
#[derive(Clone)]
struct Supplied;

/// `declaration` with the line it is written on stored in `line`; `track_caller` reports that line.
macro_rules! declared_at {
    ($line:expr, $declaration:expr) => {{
        $line.store(line!(), Ordering::SeqCst);
        $declaration;
    }};
}

fn exclude_with_no_entry(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    declared_at!(STRAY_EXCLUDE, m.meta::<PreDispatch>().exclude(["/x"]));
}

fn scoped_entry_naming_no_pattern(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    declared_at!(EMPTY_SCOPE, m.meta::<PreDispatch>().apply_for::<Stamp>(Vec::<&str>::new()));
}

fn supplies_with_no_entry(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    declared_at!(STRAY_SUPPLIES, m.meta::<PreDispatch>().supplies::<Supplied>());
}

fn supplies_after_an_adopt(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    declared_at!(ADOPT_SUPPLIES, m.meta::<PreDispatch>().adopt::<Supplied>().supplies::<Supplied>());
}

fn scope_that_is_not_a_pattern(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    declared_at!(BAD_SCOPE, m.meta::<PreDispatch>().apply_for::<Stamp>(["admin/*"]));
}

static COVERING: AtomicU32 = AtomicU32::new(0);
static GHOST: AtomicU32 = AtomicU32::new(0);
static TOO_DEEP: AtomicU32 = AtomicU32::new(0);

fn scoped_entries_two_of_which_cover_no_route(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    declared_at!(COVERING, m.meta::<PreDispatch>().apply_for::<Stamp>(["/admin/*"]));
    declared_at!(GHOST, m.meta::<PreDispatch>().apply_for::<Stamp>(["/ghost", "/phantom/*"]));
    declared_at!(TOO_DEEP, m.meta::<PreDispatch>().apply_for::<Stamp>(["/admin/users/{id}/posts"]));
}

/// Whether `entry` was written at `line` of this file, about a pre-dispatch entry, concerning
/// exactly `scopes`, and names no type.
fn is_refusal_at(entry: &ulo::PrepareEntry, line: &AtomicU32, scopes: &[&str]) -> bool {
    let line = line.load(Ordering::SeqCst);
    assert_ne!(line, 0, "the declaration never ran");
    entry.location().is_some_and(|at| at.file() == file!() && at.line() == line)
        && entry.subject() == Some("pre-dispatch entry")
        && entry.scopes() == scopes
        && entry.names().is_empty()
}

#[tokio::test]
async fn a_declaration_the_stage_cannot_build_is_refused_at_the_line_that_wrote_it() {
    let error = refused(exclude_with_no_entry).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "an `exclude` with no entry before it: {error}");
    assert!(is_refusal_at(&entries[0], &STRAY_EXCLUDE, &[]), "{entries:?}");

    let error = refused(scoped_entry_naming_no_pattern).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "a scoped entry with an empty scope: {error}");
    assert!(is_refusal_at(&entries[0], &EMPTY_SCOPE, &[]), "{entries:?}");

    let error = refused(scope_that_is_not_a_pattern).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "a scope without a leading slash: {error}");
    assert!(is_refusal_at(&entries[0], &BAD_SCOPE, &["admin/*"]), "{entries:?}");
}

#[tokio::test]
async fn a_supplies_with_no_entry_to_follow_is_refused_at_the_line_that_wrote_it() {
    let error = refused(supplies_with_no_entry).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "a `supplies` with no entry before it: {error}");
    assert!(is_refusal_at(&entries[0], &STRAY_SUPPLIES, &[]), "{entries:?}");

    let error = refused(supplies_after_an_adopt).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "a `supplies` after an `adopt`: {error}");
    assert!(is_refusal_at(&entries[0], &ADOPT_SUPPLIES, &[]), "{entries:?}");
}

/// Whether `entry` is the refusal of one unreached scope of the pre-dispatch entry written at
/// `line`: that line of this file, the subject, exactly that one scope, and no type named.
fn is_unreached(entry: &ulo::PrepareEntry, line: &AtomicU32, scope: &str) -> bool {
    let line = line.load(Ordering::SeqCst);
    assert_ne!(line, 0, "the declaration never ran");
    entry.location().is_some_and(|at| at.file() == file!() && at.line() == line)
        && entry.subject() == Some("pre-dispatch entry")
        && entry.scopes() == [scope.to_owned()]
        && entry.names().is_empty()
}

#[tokio::test]
async fn a_scoped_entry_covering_no_route_is_refused_with_its_scope_and_the_line_that_wrote_it() {
    let error = refused(scoped_entries_two_of_which_cover_no_route).await;
    let entries = refusal_entries(&error);

    // One refusal per unreached scope, so the entry that wrote two is named twice, once for each;
    // the entry whose scope covers routes is named not at all.
    assert_eq!(entries.len(), 3, "{error}");
    assert!(is_unreached(&entries[0], &GHOST, "/ghost"), "{entries:?}");
    assert!(is_unreached(&entries[1], &GHOST, "/phantom/*"), "{entries:?}");
    assert!(is_unreached(&entries[2], &TOO_DEEP, "/admin/users/{id}/posts"), "{entries:?}");
    let text = error.to_string();
    assert!(text.contains("`/ghost`") && text.contains("`/phantom/*`") && !text.contains("`/admin/*`"), "{error}");
}

static MIXED: AtomicU32 = AtomicU32::new(0);
static EXCLUDED: AtomicU32 = AtomicU32::new(0);

fn entry_with_one_scope_that_covers_and_one_that_does_not(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    declared_at!(MIXED, m.meta::<PreDispatch>().apply_for::<Stamp>(["/admin/*", "/typo"]));
}

fn entry_whose_every_covered_route_is_excluded(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    declared_at!(EXCLUDED, m.meta::<PreDispatch>().apply_for::<Stamp>(["/public"]).exclude(["/public"]));
}

#[tokio::test]
async fn a_scope_beside_one_that_covers_a_route_is_still_refused() {
    let error = refused(entry_with_one_scope_that_covers_and_one_that_does_not).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "{error}");
    assert!(is_unreached(&entries[0], &MIXED, "/typo"), "{entries:?}");
}

#[tokio::test]
async fn a_scope_whose_covered_routes_are_all_excluded_is_refused() {
    let error = refused(entry_whose_every_covered_route_is_excluded).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "{error}");
    assert!(is_unreached(&entries[0], &EXCLUDED, "/public"), "{entries:?}");
}

fn admin_behind_a_key_but_one_user(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<ApiKeyAuth>();
    m.meta::<PreDispatch>().apply_for::<ApiKeyAuth>(["/admin/*"]).exclude(["/admin/users/7"]);
}

#[tokio::test]
async fn a_route_checked_per_request_keeps_its_check_beside_a_route_the_same_entry_covers_whole() {
    // `/admin/users`, declared first, runs the entry unchecked; `/admin/users/{id}` holds the
    // same entry with a per-request check for the exclusion, so the two cannot share a stage.
    let app = start(admin_behind_a_key_but_one_user).await;
    for (path, status) in [("/admin/users", StatusCode::UNAUTHORIZED), ("/admin/users/7", StatusCode::OK), ("/admin/users/8", StatusCode::UNAUTHORIZED)] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, status, "{path}");
    }
    app.stop().await;
}

#[injectable]
struct Tree;

#[routes]
impl Tree {
    #[ulo_http::get("/tree/{*rest}")]
    fn rest(&self) -> &'static str {
        "rest"
    }
}

static EXACT_OVER_REST: AtomicU32 = AtomicU32::new(0);

fn exact_scope_over_a_rest_route(m: &mut ModuleDef<'_>) {
    m.controller::<Tree>();
    m.provide::<Stamp>();
    declared_at!(EXACT_OVER_REST, m.meta::<PreDispatch>().apply_for::<Stamp>(["/tree"]));
}

#[tokio::test]
async fn an_exact_scope_over_the_prefix_of_a_rest_route_is_refused() {
    // `{*rest}` takes one segment at least, so `/tree/{*rest}` matches no path `/tree` matches.
    let error = refused(exact_scope_over_a_rest_route).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "{error}");
    assert!(is_unreached(&entries[0], &EXACT_OVER_REST, "/tree"), "{entries:?}");
}

fn exact_scope_beside_a_wildcard_exclusion(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    m.meta::<PreDispatch>().apply_for::<Stamp>(["/admin"]).exclude(["/admin/*"]);
}

#[tokio::test]
async fn a_wildcard_exclusion_leaves_the_exact_scope_it_extends() {
    // `/admin/*` needs a segment after `/admin`, so it takes nothing of the scope `/admin`.
    let app = start(exact_scope_beside_a_wildcard_exclusion).await;
    assert_eq!(stamped(&app, "/admin").await, (StatusCode::OK, true));
    assert_eq!(stamped(&app, "/admin/users").await, (StatusCode::OK, false));
    app.stop().await;
}

static SWALLOWED: AtomicU32 = AtomicU32::new(0);

fn entry_whose_scope_its_exclusion_matches_whole(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.provide::<Stamp>();
    declared_at!(SWALLOWED, m.meta::<PreDispatch>().apply_for::<Stamp>(["/admin/users/me"]).exclude(["/admin/users/me"]));
}

#[tokio::test]
async fn a_scope_its_exclusion_matches_whole_is_refused_though_it_shares_paths_with_a_route() {
    // `/admin/users/me` shares a path with `/admin/users/{id}`, and the exclusion matches only
    // some of that route's paths, but every path the scope matches, so the entry never runs.
    let error = refused(entry_whose_scope_its_exclusion_matches_whole).await;
    let entries = refusal_entries(&error);
    assert_eq!(entries.len(), 1, "{error}");
    assert!(is_unreached(&entries[0], &SWALLOWED, "/admin/users/me"), "{entries:?}");
}

#[injectable]
struct Encoded;

#[routes]
impl Encoded {
    #[ulo_http::get("/caf%C3%A9/x")]
    fn written_encoded(&self) -> &'static str {
        "encoded"
    }

    #[ulo_http::get("/café/y")]
    fn written_plain(&self) -> &'static str {
        "plain"
    }
}

fn plain_scope_over_an_encoded_route(m: &mut ModuleDef<'_>) {
    m.controller::<Encoded>();
    m.provide::<ApiKeyAuth>();
    m.meta::<PreDispatch>().apply_for::<ApiKeyAuth>(["/café/*"]);
}

fn encoded_scope_over_a_plain_route(m: &mut ModuleDef<'_>) {
    m.controller::<Encoded>();
    m.provide::<ApiKeyAuth>();
    m.meta::<PreDispatch>().apply_for::<ApiKeyAuth>(["/caf%C3%A9/*"]);
}

#[tokio::test]
async fn a_scope_and_a_route_written_in_different_percent_encodings_read_one_path() {
    // The router matches `/caf%C3%A9/x` to the route written encoded and `/caf%C3%A9/y` to the
    // one written `é`; each scope matches both paths at runtime, so the routed request runs it.
    for register in [plain_scope_over_an_encoded_route, encoded_scope_over_a_plain_route] {
        let app = start(register).await;
        for path in ["/caf%C3%A9/x", "/caf%c3%a9/x", "/caf%C3%A9/y", "/caf%c3%a9/y"] {
            let reply = app.send(Call::get(path)).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path}: {:?}", reply.matched());
        }
        let reply = app.send(Call::get("/caf%C3%A9/x").header("x-key", "open")).await;
        assert_eq!((reply.status, reply.text()), (StatusCode::OK, "encoded".to_owned()));
        app.stop().await;
    }
}
