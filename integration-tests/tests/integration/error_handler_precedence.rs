//! Which of several error handlers claims, when more than one could.
//!
//! Handlers run method, then controller, then global, and the first `Some`
//! claims. That order is the reverse of every other enhancer's: guards and
//! interceptors run global first, widening inward, and `global_enhancers.rs`
//! asserts that exact sequence. The chain runs outward instead, so the most
//! specific handler answers.
//!
//! Nothing pinned the reversal. Five files register handlers at two scopes, but
//! always on different transports, so no test ever put two candidates on one
//! error and made the framework choose — which is the only way the order is
//! observable.

use std::sync::{Arc, Mutex, OnceLock};

use crate::common::TestServer;
use serial_test::serial;
use ulo::async_trait;
use ulo::enhancer::{ChainError, ErrorHandler, Guard};
use ulo::errors::GuardRejection;
use ulo::http::Body;
use ulo::http::HttpContext;
use ulo::http::HttpHandlerResult;
use ulo::http::HttpResponse;
use ulo::{
    UloFactory, controller, get, injectable, module, routes, use_error_handlers, use_guards,
};

/// Every handler that runs records itself, so a claim by one does not hide
/// whether an earlier one was consulted at all.
static RAN: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();

fn ran() -> &'static Mutex<Vec<&'static str>> {
    RAN.get_or_init(|| Mutex::new(Vec::new()))
}

#[injectable]
pub struct Reject {}

#[async_trait]
impl Guard<HttpContext> for Reject {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        false
    }
}

/// Records that it ran, then answers with its own scope's name.
fn claim(scope: &'static str) -> Option<HttpHandlerResult> {
    ran().lock().unwrap().push(scope);
    let mut resp = HttpResponse::new();
    resp.status = 403;
    resp.body = Some(Body::text(scope));
    Some(Ok(resp))
}

#[injectable]
pub struct MethodHandler {}

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for MethodHandler {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        error.downcast_ref::<GuardRejection>()?;
        claim("method")
    }
}

#[injectable]
pub struct ControllerHandler {}

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for ControllerHandler {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        error.downcast_ref::<GuardRejection>()?;
        claim("controller")
    }
}

/// Runs, records, and declines — the observe-without-shaping path.
#[injectable]
pub struct ObservingHandler {}

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for ObservingHandler {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        error.downcast_ref::<GuardRejection>()?;
        ran().lock().unwrap().push("method");
        None
    }
}

/// The global scope, registered on the factory rather than by attribute.
struct GlobalHandler;

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for GlobalHandler {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        error.downcast_ref::<GuardRejection>()?;
        claim("global")
    }
}

// ── every scope populated ──────────────────────────────────────────────────

#[controller("/all")]
pub struct AllScopes {}

#[routes]
#[use_error_handlers(ControllerHandler)]
impl AllScopes {
    #[get("/x")]
    #[use_guards(Reject)]
    #[use_error_handlers(MethodHandler)]
    fn x(&self) -> Body {
        Body::text("unreachable")
    }
}

#[module(controllers: [AllScopes], providers: [Reject, MethodHandler, ControllerHandler])]
impl AllModule {}

/// The most specific handler claims, and the wider ones are never consulted.
#[serial]
#[tokio_localset_test::localset_test]
async fn the_method_handler_claims_before_the_controllers() {
    ran().lock().unwrap().clear();

    let mut factory = UloFactory::new();
    factory.use_global_http_error_handler(Arc::new(GlobalHandler));
    let server = TestServer::start_with(factory, AllModule).await;

    let resp = server
        .client()
        .get(server.url("/all/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    assert_eq!(
        resp.text().await.unwrap(),
        "method",
        "the method-scoped handler is the most specific and claims first"
    );
    assert_eq!(
        *ran().lock().unwrap(),
        vec!["method"],
        "a claim stops the chain, so the wider handlers never run"
    );
}

// ── the method scope declines ──────────────────────────────────────────────

#[controller("/declines")]
pub struct MethodDeclines {}

#[routes]
#[use_error_handlers(ControllerHandler)]
impl MethodDeclines {
    #[get("/x")]
    #[use_guards(Reject)]
    #[use_error_handlers(ObservingHandler)]
    fn x(&self) -> Body {
        Body::text("unreachable")
    }
}

#[module(controllers: [MethodDeclines], providers: [Reject, ObservingHandler, ControllerHandler])]
impl DeclineModule {}

/// A handler that returns `None` has observed without shaping: it ran, and the
/// next one out answered. Without this, a chain that stopped at the first
/// handler regardless of its return would still pass the test above.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_that_declines_observes_and_the_next_one_answers() {
    ran().lock().unwrap().clear();

    let server = TestServer::start(DeclineModule).await;

    let resp = server
        .client()
        .get(server.url("/declines/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.text().await.unwrap(),
        "controller",
        "the method handler declined, so the controller's answered"
    );
    assert_eq!(
        *ran().lock().unwrap(),
        vec!["method", "controller"],
        "the declining handler still ran, which is what observing means"
    );
}
