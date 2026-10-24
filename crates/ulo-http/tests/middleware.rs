//! The pre-dispatch stage: unscoped entries run in the order written and see misses, may rewrite
//! the path before routing, answer without calling `next` or fail into the error chain; scoped
//! entries run for the routes their pattern covers minus the exclusions, and a declaration the
//! stage cannot build is refused by `listen()` at the call that wrote it.

mod o1_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use http::{HeaderMap, HeaderValue, StatusCode};
use o1_support::{Call, Running, refused, start};
use ulo::{AnyErrorHandler, BoxError, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::middleware::{Middleware, Next};
use ulo_http::{Http, HttpBody, HttpCx, PreDispatch, Request, Response, Routing};
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
async fn a_scoped_entry_runs_for_the_routes_its_pattern_covers_and_not_the_excluded_one() {
    let app = start(scoped_with_exclusion).await;

    assert_eq!(stamped(&app, "/admin/users").await, (StatusCode::OK, true));
    assert_eq!(stamped(&app, "/admin/users/7").await, (StatusCode::OK, true), "`/admin/*` covers a parameter route below it");
    assert_eq!(stamped(&app, "/admin/health").await, (StatusCode::OK, false), "excluded");
    assert_eq!(stamped(&app, "/admin").await, (StatusCode::OK, false), "`/admin/*` needs a segment after `/admin`");
    assert_eq!(stamped(&app, "/public").await, (StatusCode::OK, false), "outside the scope");
    assert_eq!(stamped(&app, "/admin/nowhere").await, (StatusCode::NOT_FOUND, false), "a scoped entry runs after a route matched");
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

/// Whether the refusal's text names the source line `line` of this file.
fn names_line(error: &ulo::StartupError, line: &AtomicU32) -> bool {
    let line = line.load(Ordering::SeqCst);
    assert_ne!(line, 0, "the declaration never ran");
    error.to_string().contains(&format!("{}:{line}:", file!()))
}

#[tokio::test]
async fn a_declaration_the_stage_cannot_build_is_refused_at_the_line_that_wrote_it() {
    let error = refused(exclude_with_no_entry).await;
    assert!(names_line(&error, &STRAY_EXCLUDE), "an `exclude` with no entry before it: {error}");

    let error = refused(scoped_entry_naming_no_pattern).await;
    assert!(names_line(&error, &EMPTY_SCOPE), "a scoped entry with an empty scope: {error}");

    let error = refused(scope_that_is_not_a_pattern).await;
    assert!(names_line(&error, &BAD_SCOPE), "a scope without a leading slash: {error}");
}

#[tokio::test]
async fn a_scoped_entry_covering_no_route_is_refused_with_its_scope_and_the_line_that_wrote_it() {
    let error = refused(scoped_entries_two_of_which_cover_no_route).await;
    let text = error.to_string();

    // Each refusal is one line naming one entry's location beside the scopes that entry wrote.
    let refusal_of = |line: &AtomicU32| -> Vec<&str> {
        let at = format!("{}:{}:", file!(), line.load(Ordering::SeqCst));
        text.lines().filter(|refusal| refusal.contains(&at)).collect()
    };
    assert!(names_line(&error, &GHOST), "the entry scoped to routes nothing declares: {error}");
    let ghost = refusal_of(&GHOST);
    assert!(ghost.len() == 1 && ghost[0].contains("`/ghost`, `/phantom/*`") && !ghost[0].contains("posts"), "every scope the entry wrote, once: {error}");
    assert!(names_line(&error, &TOO_DEEP), "a scope deeper than any route covers none: {error}");
    let deep = refusal_of(&TOO_DEEP);
    assert!(deep.len() == 1 && deep[0].contains("`/admin/users/{id}/posts`") && !deep[0].contains("ghost"), "{error}");
    assert!(!names_line(&error, &COVERING), "the entry whose scope covers routes is not named: {error}");
    assert!(!text.contains("`/admin/*`"), "{error}");
}
