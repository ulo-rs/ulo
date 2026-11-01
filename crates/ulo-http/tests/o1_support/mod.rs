//! An HTTP app served through a backend that serves nothing, called in process through
//! `AppService` the way a backend calls it, with every wait under a real-clock deadline.

#![allow(dead_code)]

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use http_body_util::BodyExt;
use tokio::sync::Notify;
use ulo::{App, AppHandle, BoxError, Module, ModuleDef, ModuleIdentity, Signal, StartupError};
use ulo::{PrepareEntry, PrepareError, TypeName};
use ulo_http::{AppService, Backend, BackendLimits, ConnInfo, HeaderMap, HttpBody, HttpConfig, Request, Response, Routing, StatusCode};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;

/// The longest any one wait in a test may take, on the real clock.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// Keeps the `AppService` it is handed at `bind`; serves nothing.
#[derive(Clone, Default)]
pub struct Keeper {
    svc: Arc<OnceLock<AppService>>,
    drained: Arc<Notify>,
}

impl Backend for Keeper {
    const NAME: &'static str = "keeper";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(&mut self, _listeners: Vec<BoundListener>, _tls: Option<Arc<ServerConfig>>, svc: AppService, _cfg: &HttpConfig) -> Result<(), BoxError> {
        let _ = self.svc.set(svc);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.drained.notified().await;
        Ok(())
    }

    async fn drain(&self) {
        self.drained.notify_one();
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// A root module whose one `register` is a plain function.
pub struct Root(pub fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        (self.0)(m);
    }
}

/// A started app and the service its backend was handed.
pub struct Running {
    svc: AppService,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

/// Starts the app, or answers what `listen()` refused it with: the route table and the
/// pre-dispatch stage are built there.
#[allow(clippy::result_large_err)]
async fn launch(root: Root, drain: Option<Duration>) -> Result<Running, StartupError> {
    let keeper = Keeper::default();
    let builder = App::builder(root).runtime(ulo_runtime_tokio::Tokio::current());
    let builder = match drain {
        Some(window) => builder.drain_timeout(window),
        None => builder,
    };
    let app = builder
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()))
        .listen();
    let app = tokio::time::timeout(PATIENCE, app).await.expect("listen finished in time")?;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let svc = keeper.svc.get().cloned().expect("the backend was handed the service");
    Ok(Running { svc, handle, serving })
}

/// Starts the app; panics when `listen()` refuses it.
pub async fn start(register: fn(&mut ModuleDef<'_>)) -> Running {
    launch(Root(register), None).await.unwrap_or_else(|error| panic!("the app listens: {error}"))
}

/// Starts the app with a drain window of `window`, after which a call still running is abandoned.
pub async fn start_draining(register: fn(&mut ModuleDef<'_>), window: Duration) -> Running {
    launch(Root(register), Some(window)).await.unwrap_or_else(|error| panic!("the app listens: {error}"))
}

/// What `listen()` refuses the app with.
pub async fn refused(register: fn(&mut ModuleDef<'_>)) -> StartupError {
    match launch(Root(register), None).await {
        Ok(running) => {
            running.stop().await;
            panic!("listen() accepted an app it should refuse")
        }
        Err(error) => error,
    }
}

/// Puts one value in a request's extensions.
type Insert = Box<dyn FnOnce(&mut http::Extensions) + Send>;

/// One request.
pub struct Call {
    method: http::Method,
    path: String,
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
    /// Sent as one frame each of a body of no known length, the way chunked encoding delivers one.
    chunks: Option<Vec<Vec<u8>>>,
    ext: Vec<Insert>,
}

impl Call {
    pub fn get(path: &str) -> Call {
        Call::new(http::Method::GET, path)
    }

    pub fn new(method: http::Method, path: &str) -> Call {
        Call { method, path: path.to_owned(), headers: Vec::new(), body: Vec::new(), chunks: None, ext: Vec::new() }
    }

    pub fn header(mut self, name: &str, value: &str) -> Call {
        self.headers.push((name.to_owned(), value.as_bytes().to_vec()));
        self
    }

    pub fn header_bytes(mut self, name: &str, value: &[u8]) -> Call {
        self.headers.push((name.to_owned(), value.to_vec()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Call {
        self.body = body.into();
        self
    }

    /// The body as a stream of these frames: no `Content-Length`, no size hint, so a limit is met
    /// as the frames arrive, as it is under chunked encoding.
    pub fn chunks<I, C>(mut self, chunks: I) -> Call
    where
        I: IntoIterator<Item = C>,
        C: Into<Vec<u8>>,
    {
        self.chunks = Some(chunks.into_iter().map(Into::into).collect());
        self
    }

    /// A value in the request's `http::Extensions`, as a host's middleware leaves it.
    pub fn ext<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Call {
        self.ext.push(Box::new(move |extensions| {
            extensions.insert(value);
        }));
        self
    }

    pub fn into_request(self) -> Request {
        let mut builder = http::Request::builder().method(self.method).uri(self.path);
        for (name, value) in self.headers {
            builder = builder.header(name, http::HeaderValue::from_bytes(&value).expect("a header value"));
        }
        let (mut head, ()) = builder.body(()).expect("the request builds").into_parts();
        for insert in self.ext {
            insert(&mut head.extensions);
        }
        let body = match self.chunks {
            Some(chunks) => HttpBody::stream(futures_util::stream::iter(chunks.into_iter().map(|chunk| Ok::<_, std::convert::Infallible>(bytes::Bytes::from(chunk))))),
            None if self.body.is_empty() => HttpBody::empty(),
            None => HttpBody::from_bytes(self.body),
        };
        Request { head, body, conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None }
    }
}

/// A response whose body was read to its end.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    /// What the router recorded on the response.
    pub routing: Option<Routing>,
}

impl Reply {
    /// The pattern and the handler's name when a handler answered, `None` otherwise.
    pub fn matched(&self) -> Option<(String, &'static str)> {
        match &self.routing {
            Some(Routing::Matched { route, handler }) => Some((route.to_string(), handler.name())),
            _ => None,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| panic!("a JSON body, got {:?}: {error}", self.text()))
    }

    /// Every value of `name`, in the order the headers were written.
    pub fn values(&self, name: &str) -> Vec<String> {
        self.headers.get_all(name).iter().filter_map(|value| value.to_str().ok()).map(str::to_owned).collect()
    }

    /// The values of `name` as a set of comma-separated tokens, across repeated headers.
    pub fn tokens(&self, name: &str) -> Vec<String> {
        let mut tokens: Vec<String> = self
            .headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .collect();
        tokens.sort();
        tokens
    }
}

impl Running {
    pub fn app(&self) -> &AppHandle {
        &self.handle
    }

    /// The service the backend was handed, to call from a task of the test's own.
    pub fn service(&self) -> AppService {
        self.svc.clone()
    }

    /// Starts the app's `close` on a task of its own.
    pub fn begin_close(&self) -> tokio::task::JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>> {
        let handle = self.handle.clone();
        tokio::spawn(async move { handle.close(Signal::new("test")).await })
    }

    /// The response with its body not yet read.
    pub async fn call(&self, call: Call) -> Response {
        tokio::time::timeout(PATIENCE, self.svc.call(call.into_request())).await.expect("the service answered in time")
    }

    /// The response with its body read to its end.
    pub async fn send(&self, call: Call) -> Reply {
        let response = self.call(call).await;
        read(response).await
    }

    pub async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

/// `response`'s body read to its end.
pub async fn read(response: Response) -> Reply {
    let (parts, body) = response.into_parts();
    let collected = tokio::time::timeout(PATIENCE, body.collect()).await.expect("the body ended in time").expect("the body yields no error");
    let routing = parts.extensions.get::<Routing>().cloned();
    Reply { status: parts.status, headers: parts.headers, body: collected.to_bytes().to_vec(), routing }
}

/// Every type the refusal's `PrepareError`s name, across the transports that refused.
pub fn refusal_names(error: &StartupError) -> Vec<TypeName> {
    let StartupError::Configure(errors) = error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    errors.iter().flat_map(|refused| refused.source.downcast_ref::<PrepareError>().expect("a PrepareError").names().to_vec()).collect()
}

/// Every entry the refusal's `PrepareError`s list, in the order the transports answered.
pub fn refusal_entries(error: &StartupError) -> Vec<PrepareEntry> {
    let StartupError::Configure(errors) = error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    errors.iter().flat_map(|refused| refused.source.downcast_ref::<PrepareError>().expect("a PrepareError").entries().to_vec()).collect()
}

/// How many transports refused.
pub fn refusals(error: &StartupError) -> usize {
    let StartupError::Configure(errors) = error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    errors.len()
}
