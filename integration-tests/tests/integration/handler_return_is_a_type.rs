//! What a handler returns is decided by its type, not by how the type is written.
//!
//! A handler that answers `Err` reaches the error chain whether its return type is spelled
//! `Result<T, E>`, `impl IntoOutput<Http>`, or an alias for a `Result`. A check that matched the
//! last path segment against `Result` read the last two as infallible, and the value rendered its
//! own error instead of reaching a `#[catch]` handler.

use std::sync::{Arc, Mutex};

use ulo::dispatch::{Http, IntoOutput};
use ulo::http::{Body, HttpContext, HttpHandlerResult, HttpResponse};
use ulo::{UloFactory, catch, controller, get, module, routes};
use ulo_http_axum::AxumAdapter;

static CLAIMED: Mutex<usize> = Mutex::new(0);

#[derive(Debug)]
pub struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refused")
    }
}
impl std::error::Error for Refused {}
impl ulo::Error for Refused {
    fn kind(&self) -> ulo::ErrorKind {
        ulo::ErrorKind::Conflict
    }
}

#[catch(Refused)]
async fn claim_refused(_e: &Refused, _ctx: &HttpContext) -> HttpHandlerResult {
    *CLAIMED.lock().unwrap() += 1;
    Ok(HttpResponse::builder()
        .status(418)
        .json(serde_json::json!({ "claimed": true }))
        .build())
}

#[controller("/spelling")]
pub struct SpellingController {}

#[routes]
impl SpellingController {
    #[get("/result")]
    async fn as_result(&self) -> Result<Body, Refused> {
        Err(Refused)
    }

    #[get("/impl-trait")]
    async fn as_impl_trait(&self) -> impl IntoOutput<Http> {
        Err::<Body, Refused>(Refused)
    }

    /// A `Result` behind an alias. The last path segment reads `Answered`, not `Result`.
    #[get("/aliased")]
    async fn as_alias(&self) -> Answered {
        Err(Refused)
    }
}

type Answered = Result<Body, Refused>;

#[module(controllers: [SpellingController])]
impl SpellingModule {}

async fn start() -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = UloFactory::new();
        factory.use_global_http_error_handler(Arc::new(claim_refused));
        let mut app = factory.create_with(SpellingModule).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(bound.http.unwrap());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    addr_rx.await.unwrap()
}

#[tokio_localset_test::localset_test]
async fn every_spelling_reaches_the_error_chain() {
    let addr = start().await;
    *CLAIMED.lock().unwrap() = 0;

    for path in ["result", "impl-trait", "aliased"] {
        let resp = reqwest::get(format!("http://{}/spelling/{}", addr, path))
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            418,
            "`/{path}` must be answered by the catcher, not by the error rendering itself"
        );
    }

    assert_eq!(
        *CLAIMED.lock().unwrap(),
        3,
        "the catcher must run for every spelling"
    );
}
