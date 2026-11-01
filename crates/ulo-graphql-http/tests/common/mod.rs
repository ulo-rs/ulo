//! A backend that serves nothing itself, so a test calls `AppService` as a backend would, and
//! the stub engine every mounting test serves.

#![allow(dead_code)]

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use http_body::Body as _;
use tokio::sync::Notify;
use ulo::{BoxError, BoxFuture, ExecutionRef, Module, ModuleDef, ModuleIdentity};
use ulo_graphql::{BoxStream, Engine, GqlError, GqlRequest, GqlResponse, RequestFault};
use ulo_http::{AppService, Backend, BackendLimits, ConnInfo, HttpBody, HttpConfig, Request, Response, StatusCode};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;

pub const PATIENCE: Duration = Duration::from_secs(10);

/// Keeps the `AppService` it is handed at `bind`.
#[derive(Clone, Default)]
pub struct Keeper {
    svc: Arc<OnceLock<AppService>>,
    drained: Arc<Notify>,
}

impl Keeper {
    pub fn service(&self) -> AppService {
        self.svc.get().cloned().expect("the backend was handed the service")
    }
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

/// The response's head and its body read to the end.
pub struct Answer {
    pub status: StatusCode,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| panic!("the body is JSON: {error}: {:?}", String::from_utf8_lossy(&self.body)))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub async fn read(response: Response) -> Answer {
    let (parts, mut body) = response.into_parts();
    let mut data = Vec::new();
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        if let Ok(bytes) = frame.expect("the body yields no error").into_data() {
            data.extend_from_slice(&bytes);
        }
        if body.is_end_stream() {
            break;
        }
    }
    Answer { status: parts.status, headers: parts.headers, body: data }
}

pub fn request(method: &str, uri: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
    let mut builder = http::Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let (head, ()) = builder.body(()).expect("the request builds").into_parts();
    Request { head, body: HttpBody::from_bytes(body.to_vec()), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None }
}

/// What the stub engine saw.
#[derive(Default)]
pub struct Seen {
    pub requests: Mutex<Vec<GqlRequest>>,
    pub playground: Mutex<Vec<(String, Option<String>)>>,
}

impl Seen {
    pub fn requests(&self) -> Vec<GqlRequest> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn playground(&self) -> Vec<(String, Option<String>)> {
        self.playground.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Answers by what the query names: `executed`, `fieldError`, `requestError` (a request error
/// naming no fault), `syntaxError`, `validationError`, `operationError`, `coercionError` or
/// `failed`; every request it executes is recorded.
pub struct Stub {
    pub seen: Arc<Seen>,
    pub page: bool,
}

impl Engine for Stub {
    fn execute(&self, req: GqlRequest, _exec: ExecutionRef) -> BoxFuture<'static, GqlResponse> {
        let query = req.query.clone();
        self.seen.requests.lock().unwrap_or_else(PoisonError::into_inner).push(req);
        Box::pin(async move {
            let fault = [
                ("syntaxError", RequestFault::Syntax),
                ("validationError", RequestFault::Validation),
                ("operationError", RequestFault::Operation),
                ("coercionError", RequestFault::Coercion),
            ]
            .into_iter()
            .find_map(|(name, fault)| query.contains(name).then_some(fault));
            if let Some(fault) = fault {
                GqlResponse::request_fault(fault, vec![GqlError::new("the request is at fault")])
            } else if query.contains("requestError") {
                GqlResponse::request_error(vec![GqlError::new("the document is invalid")])
            } else if query.contains("failed") {
                GqlResponse::failed(vec![GqlError::new("the server failed")])
            } else if query.contains("fieldError") {
                GqlResponse::executed(serde_json::Value::Null, vec![GqlError::new("a field failed")])
            } else {
                GqlResponse::executed(serde_json::json!({ "ok": true }), Vec::new())
            }
        })
    }

    fn subscribe(&self, _req: GqlRequest, _exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        Box::pin(futures_util::stream::empty())
    }

    fn sdl(&self) -> String {
        String::from("type Query { ok: Boolean }")
    }

    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String> {
        self.seen.playground.lock().unwrap_or_else(PoisonError::into_inner).push((endpoint.to_owned(), subscriptions.map(str::to_owned)));
        self.page.then(|| String::from("<html>stub playground</html>"))
    }
}

/// Binds the stub and exports it as `dyn Engine`, the module `GraphqlConfig::engine_from` names.
pub struct EngineModule {
    pub seen: Arc<Seen>,
    pub page: bool,
}

impl Module for EngineModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Stub { seen: Arc::clone(&self.seen), page: self.page }).also_as::<dyn Engine>(|engine| engine);
        m.export::<dyn Engine>();
    }
}
