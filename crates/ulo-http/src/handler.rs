use std::future::Future;
use std::sync::Arc;

use http::Method;
use serde::de::DeserializeOwned;
use ulo::{BoxError, BoxFuture, TypeName};

use crate::cx::HttpCx;
use crate::extract::PathCheck;
use crate::response::Response;

/// A route's call: extraction, the controller, the handler and the reply, run by `dispatch` after
/// every guard admits.
pub(crate) type HandlerFn = Arc<dyn Fn(HttpCx) -> BoxFuture<'static, Result<Response, BoxError>> + Send + Sync>;

/// One HTTP handler: the value a controller's [`mount`](ulo::Controller::mount) passes to
/// [`HandlerSpec::new`](ulo::HandlerSpec::new), which the server reads back when it prepares. The
/// method attributes (`#[ulo_http::get("/users/{id}")]` and the rest) build this value, and a
/// controller written without them builds it by hand.
///
/// The route is the spec's: [`HandlerSpec::route`](ulo::HandlerSpec::route) is the pattern the
/// router matches and the one [`HandlerInfo::route`](ulo::HandlerInfo::route) and
/// [`Routing::Matched`](crate::Routing::Matched) report, the controller's `.at(..)` prefix joined to
/// it. A handler mounted with no route is refused when the server prepares. Enhancers, declared
/// metadata (a [`BodyLimit`](crate::BodyLimit) or [`Timeout`](crate::Timeout) among it) and what
/// the call reads from the container go on the spec as well, as for every transport.
///
/// The call runs once per request, after every guard admits, and inside the interceptors. It reads
/// its parameters through [`FromCall`](crate::FromCall), its controller through
/// `cx.exec().get::<C>()`, and answers a [`Response`], which any
/// [`IntoReply<Http>`](crate::IntoReply) value converts into. An error it returns reaches the error
/// handlers as it stands; an error that classifies itself is converted with
/// [`CallError::from`](crate::CallError) first, as the attributes do, since a `BoxError` carries no
/// kind of its own and renders as an internal error unclaimed.
///
/// ```
/// use ulo::scope::Auto;
/// use ulo::{Construct, ConstructError, Controller, Dependencies, HandlerSpec, Mount, Resolver};
/// use ulo_http::{FromCall, Http, HttpCx, HttpHandler, IntoReply, Method, Path, Response};
///
/// struct Greeter;
///
/// impl Construct for Greeter {
///     type Scope = Auto;
///
///     fn dependencies(_: &mut Dependencies) {}
///
///     async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
///         Ok(Greeter)
///     }
/// }
///
/// impl Greeter {
///     fn greet(&self, name: &str) -> String {
///         format!("hello, {name}")
///     }
/// }
///
/// async fn greet(cx: HttpCx) -> Result<Response, ulo::BoxError> {
///     let Path(name) = <Path<String> as FromCall<Http>>::from_call(&cx).await?;
///     let greeter = cx.exec().get::<Greeter>().await?;
///     Ok(greeter.greet(&name).into_reply(&cx)?)
/// }
///
/// impl Controller for Greeter {
///     fn mount(m: &mut Mount<'_>) {
///         let handler = HttpHandler::new(Method::GET, greet).reads_path::<String>();
///         m.handler(HandlerSpec::<Http, _>::new("greet", handler).route("/greet/{name}"));
///     }
/// }
/// ```
pub struct HttpHandler {
    pub(crate) method: Method,
    pub(crate) call: HandlerFn,
    pub(crate) path_checks: Vec<PathCheck>,
    pub(crate) host_reads: Vec<HostRead>,
}

impl HttpHandler {
    /// The handler answering `method` with `call`. A `multer::Error` the call returns reaches the
    /// error handlers as the `ExtractError` it is: `TooLarge` when the read passed the route's
    /// body limit, `Malformed` otherwise.
    pub fn new<F, Fut>(method: Method, call: F) -> Self
    where
        F: Fn(HttpCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response, BoxError>> + Send + 'static,
    {
        let call: HandlerFn = Arc::new(move |cx| {
            let app = cx.app().clone();
            let run = call(cx);
            Box::pin(async move { run.await.map_err(|err| crate::render::from_multipart(err, &app)) })
        });
        HttpHandler { method, call, path_checks: Vec::new(), host_reads: Vec::new() }
    }

    /// Declares that the call reads [`Path<T>`](crate::Path), so `T` is checked against the route
    /// when the server prepares, as an attribute checks each `Path<T>` parameter: a struct field
    /// the route does not name, a tuple of the wrong length or a scalar on a route with other than
    /// one parameter is a startup error rather than the first request's.
    pub fn reads_path<T: DeserializeOwned + 'static>(mut self) -> Self {
        self.path_checks.push(PathCheck::of::<T>());
        self
    }

    /// Declares that the call reads [`Host<T>`](crate::Host) or `Option<Host<T>>`, so an
    /// embedding whose host supplies no extensions refuses the route at `listen()` unless a
    /// pre-dispatch entry covering it supplies `T`.
    pub fn reads_host<T: 'static>(mut self) -> Self {
        self.host_reads.push(HostRead { ty: TypeName::of::<T>() });
        self
    }

    pub(crate) fn method(&self) -> &Method {
        &self.method
    }
}

/// The value type a `Host<T>` read names, for the embedding's `host_extensions` check.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HostRead {
    pub(crate) ty: TypeName,
}
