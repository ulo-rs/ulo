//! Tower layers in the pre-dispatch stage: an unscoped layer wraps routing and sees misses, may
//! rewrite the request before the route is matched, layers nest in declaration order, a scoped
//! layer wraps the request paths its pattern matches, routed or not, an `Err` is rendered by its kind, a response a
//! layer builds is not an error, and a second call of the inner service for one request is a 500.

mod o1_support;

use std::sync::Arc;
use std::task::{Context, Poll};

use http::{HeaderValue, StatusCode};
use o1_support::{Call, start};
use tower::Service as _;
use ulo::{AnyErrorHandler, BoxError, BoxFuture, ErrorHandler, ModuleDef, injectable, routes};
use ulo_http::{Http, HttpBody, HttpCx, PreDispatch, Response};
use ulo_transport::{CallError, ErrorKind};

type Req = http::Request<HttpBody>;
type Res = http::Response<HttpBody>;

/// What a layer in these tests does with a request and the rest of the stage.
type Behaviour = fn(Req, ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>>;

/// A tower layer running `Behaviour` around the stage.
#[derive(Clone)]
struct Around(Behaviour);

impl<S> tower::Layer<S> for Around {
    type Service = Wrapped<S>;

    fn layer(&self, inner: S) -> Wrapped<S> {
        Wrapped { behaviour: self.0, inner }
    }
}

#[derive(Clone)]
struct Wrapped<S> {
    behaviour: Behaviour,
    #[allow(dead_code)]
    inner: S,
}

impl tower::Service<Req> for Wrapped<ulo_http::Service> {
    type Response = Res;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<Res, BoxError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Req) -> Self::Future {
        (self.behaviour)(req, self.inner.clone())
    }
}

fn set(response: &mut Res, name: &'static str, value: &str) {
    response.headers_mut().append(name, HeaderValue::from_str(value).expect("a header value"));
}

/// Marks the response `x-layer: seen` and `x-path` with the path the layer saw.
fn mark(req: Req, mut inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move {
        let path = req.uri().path().to_owned();
        let mut response = inner.call(req).await?;
        set(&mut response, "x-layer", "seen");
        set(&mut response, "x-path", &path);
        Ok(response)
    })
}

fn order_a(req: Req, mut inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move {
        let mut response = inner.call(req).await?;
        set(&mut response, "x-order", "a");
        Ok(response)
    })
}

fn order_b(req: Req, mut inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move {
        let mut response = inner.call(req).await?;
        set(&mut response, "x-order", "b");
        Ok(response)
    })
}

/// Rewrites `/old` to `/new` before the rest of the stage.
fn rewrite(mut req: Req, mut inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move {
        if req.uri().path() == "/old" {
            *req.uri_mut() = "/new".parse()?;
        }
        Ok(inner.call(req).await?)
    })
}

/// Calls the inner service twice, the second time with a request built from the first's
/// extensions, and answers the second call's response, noting the first's status.
fn twice(req: Req, mut inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move {
        let (parts, body) = req.into_parts();
        let mut again = http::Request::new(HttpBody::empty());
        *again.uri_mut() = parts.uri.clone();
        *again.extensions_mut() = parts.extensions.clone();
        let first = inner.call(Req::from_parts(parts, body)).await?;
        let first_status = first.status();
        let mut second = inner.call(again).await?;
        set(&mut second, "x-first-status", first_status.as_str());
        Ok(second)
    })
}

/// Fails with an `Unauthorized` carrying its own challenge.
fn refuse(_req: Req, _inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move { Err(CallError::unauthorized("Bearer realm=\"tower\"").into()) })
}

/// Builds its own 401 without calling the rest of the stage.
fn answer(_req: Req, _inner: ulo_http::Service) -> BoxFuture<'static, Result<Res, BoxError>> {
    Box::pin(async move {
        let mut response = Response::new(HttpBody::from("the layer's own answer"));
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        Ok::<Res, BoxError>(response)
    })
}

/// Answers every error with 200 and the kind it arrived as.
struct Claim;

impl ErrorHandler<Http> for Claim {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        Ok(Response::new(HttpBody::from(CallError::from_boxed(err).kind().as_str())))
    }
}

#[injectable]
struct Pages;

#[routes]
impl Pages {
    #[ulo_http::get("/hit")]
    fn hit(&self) -> &'static str {
        "hit"
    }

    #[ulo_http::get("/new")]
    fn rewritten(&self) -> &'static str {
        "new"
    }

    #[ulo_http::get("/files/a")]
    fn file(&self) -> &'static str {
        "file"
    }

    #[ulo_http::get("/other")]
    fn other(&self) -> &'static str {
        "other"
    }
}

fn marking(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer(Around(mark));
}

fn rewriting(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer(Around(rewrite));
}

fn nested(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer(Around(order_a)).layer(Around(order_b));
}

fn scoped(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer_for(["/files/*"], Around(mark));
}

fn calling_twice(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer(Around(twice));
}

fn refusing(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer(Around(refuse));
}

fn refusing_with_a_claiming_handler(m: &mut ModuleDef<'_>) {
    refusing(m);
    let claim: Arc<AnyErrorHandler<Http>> = Arc::new(Claim);
    m.enhancer::<AnyErrorHandler<Http>>().value(claim);
}

fn answering_with_a_claiming_handler(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.meta::<PreDispatch>().layer(Around(answer));
    let claim: Arc<AnyErrorHandler<Http>> = Arc::new(Claim);
    m.enhancer::<AnyErrorHandler<Http>>().value(claim);
}

#[tokio::test]
async fn an_unscoped_layer_wraps_routing_so_it_sees_the_path_as_sent_and_a_miss() {
    let app = start(marking).await;

    for (path, status) in [("/hit", StatusCode::OK), ("/nowhere", StatusCode::NOT_FOUND)] {
        let reply = app.send(Call::get(path)).await;
        assert_eq!(reply.status, status, "{path}");
        assert_eq!(reply.header("x-layer"), Some("seen"), "{path}");
        assert_eq!(reply.header("x-path"), Some(path), "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_layer_that_rewrites_the_uri_changes_which_route_matches() {
    let app = start(rewriting).await;

    let reply = app.send(Call::get("/old")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "new".to_owned()));
    assert_eq!(reply.matched(), Some(("/new".to_owned(), "rewritten")));
    app.stop().await;
}

#[tokio::test]
async fn layers_nest_in_declaration_order_the_first_declared_outermost() {
    let app = start(nested).await;

    let reply = app.send(Call::get("/hit")).await;
    // The inner layer writes its header as its call returns, so it is first on the way out.
    assert_eq!(reply.values("x-order"), ["b", "a"]);
    app.stop().await;
}

#[tokio::test]
async fn a_scoped_layer_wraps_the_paths_its_pattern_matches_only_routed_or_not() {
    let app = start(scoped).await;

    let covered = app.send(Call::get("/files/a")).await;
    assert_eq!((covered.status, covered.header("x-layer")), (StatusCode::OK, Some("seen")));
    let outside = app.send(Call::get("/other")).await;
    assert_eq!((outside.status, outside.header("x-layer")), (StatusCode::OK, None));
    let miss = app.send(Call::get("/files/zzz")).await;
    assert_eq!((miss.status, miss.header("x-layer")), (StatusCode::NOT_FOUND, Some("seen")), "a miss under the scope passes the layer");
    let bare = app.send(Call::get("/files")).await;
    assert_eq!((bare.status, bare.header("x-layer")), (StatusCode::NOT_FOUND, None), "`/files/*` needs a segment after `/files`");
    app.stop().await;
}

#[tokio::test]
async fn a_second_call_of_the_inner_service_for_one_request_answers_500() {
    let app = start(calling_twice).await;

    let reply = app.send(Call::get("/hit")).await;
    assert_eq!(reply.header("x-first-status"), Some("200"), "the first call reached the handler");
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "the second had no continuation left");
    app.stop().await;
}

#[tokio::test]
async fn an_err_from_a_layer_is_rendered_by_its_kind_and_reaches_the_error_handler() {
    let app = start(refusing).await;
    let reply = app.send(Call::get("/hit")).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.header("www-authenticate"), Some("Bearer realm=\"tower\""));
    app.stop().await;

    let app = start(refusing_with_a_claiming_handler).await;
    let reply = app.send(Call::get("/hit")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, ErrorKind::Unauthorized.as_str().to_owned()));
    app.stop().await;
}

#[tokio::test]
async fn a_response_a_layer_builds_is_a_response_and_the_error_handlers_never_see_it() {
    let app = start(answering_with_a_claiming_handler).await;

    let reply = app.send(Call::get("/hit")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::UNAUTHORIZED, "the layer's own answer".to_owned()));
    app.stop().await;
}
