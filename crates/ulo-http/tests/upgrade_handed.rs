//! A request's upgrade future yields the connection only when the response the service hands the
//! backend is an `UpgradeHandler`'s 101 (transports DESIGN decision 280). hyper hands the
//! connection to whoever holds the future once its last exchange ends, whatever status the
//! request was answered with, so a pre-dispatch entry holding the future, which reads the request
//! and its public `upgrade`, would otherwise take over the connection after the route's 200 or
//! the 500 that replaces a handler's 101. Each request here carries a future
//! that resolves when the test says the backend has handed the connection over, which is after
//! the service has answered.

mod o1_support;

use std::borrow::Cow;
use std::sync::{Mutex, PoisonError};

use futures::io::Cursor;
use http::StatusCode;
use o1_support::{Call, start};
use tokio::sync::oneshot;
use ulo::{BoxError, BoxFuture, ModuleDef, controller, handlers, injectable};
use ulo_http::middleware::{Middleware, Next};
use ulo_http::{HttpBody, OnUpgrade, PreDispatch, Request, Response, UpgradeHandler, Upgrades, Upgraded};

/// The upgrade futures the handlers took, newest last.
static TAKEN: Mutex<Vec<OnUpgrade>> = Mutex::new(Vec::new());

fn keep(pending: Option<OnUpgrade>) {
    TAKEN.lock().unwrap_or_else(PoisonError::into_inner).push(pending.expect("the request carries an upgrade future"));
}

fn with_status(status: StatusCode) -> Response {
    let mut response = Response::new(HttpBody::empty());
    *response.status_mut() = status;
    response
}

#[controller]
struct Routes;

#[handlers]
impl Routes {
    #[ulo_http::get("/take")]
    fn take(&self) -> Response {
        with_status(StatusCode::OK)
    }

    #[ulo_http::get("/take/switch")]
    fn take_switch(&self) -> Response {
        with_status(StatusCode::SWITCHING_PROTOCOLS)
    }
}

/// Over `/take/*` and `/take`: takes the request's upgrade future, then runs the route.
#[injectable]
struct Takes;

impl Middleware for Takes {
    async fn handle(&self, mut req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        keep(req.upgrade.take());
        next.run(req).await
    }
}

struct Socket;

impl UpgradeHandler for Socket {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/socket".into()]
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        keep(req.upgrade);
        Box::pin(async { with_status(StatusCode::SWITCHING_PROTOCOLS) })
    }
}

/// Replaces the answer `next` gives with a 400 when the request carries `x-refuse`.
#[injectable]
struct Refuse;

impl Middleware for Refuse {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        let refuse = req.headers().contains_key("x-refuse");
        let response = next.run(req).await?;
        Ok(if refuse { with_status(StatusCode::BAD_REQUEST) } else { response })
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Routes>();
    m.provide::<Refuse>();
    m.provide::<Takes>();
    m.meta::<PreDispatch>().apply_for::<Refuse>(["/*"]).apply_for::<Takes>(["/take", "/take/*"]);
    m.meta::<Upgrades>().register(Socket);
}

/// An upgrade future the backend fulfils when `handed` fires.
fn pending() -> (OnUpgrade, oneshot::Sender<()>) {
    let (handed, fired) = oneshot::channel();
    let pending = OnUpgrade::new(async move {
        fired.await.map_err(|_| BoxError::from("the backend dropped the connection"))?;
        Ok(Upgraded::from_futures(Cursor::new(Vec::new())))
    });
    (pending, handed)
}

#[tokio::test]
async fn the_upgrade_future_yields_the_connection_only_after_an_upgrade_handler_s_101() {
    let app = start(register).await;
    for (call, status, yields) in [
        (Call::get("/take").header("upgrade", "websocket"), StatusCode::OK, false),
        (Call::get("/take/switch").header("upgrade", "websocket"), StatusCode::INTERNAL_SERVER_ERROR, false),
        (Call::get("/socket").header("upgrade", "websocket"), StatusCode::SWITCHING_PROTOCOLS, true),
        (Call::get("/socket").header("upgrade", "websocket").header("x-refuse", "1"), StatusCode::BAD_REQUEST, false),
    ] {
        let (upgrade, handed) = pending();
        let mut req = call.into_request();
        let what = format!("{} {:?}", req.path(), req.headers().get("x-refuse"));
        req.upgrade = Some(upgrade);
        let response = app.service().call(req).await;
        assert_eq!(response.status(), status, "{what}");
        handed.send(()).expect("the future is still held");
        let taken = TAKEN.lock().unwrap_or_else(PoisonError::into_inner).pop().expect("the handler took the future");
        assert_eq!(taken.await.is_ok(), yields, "{what} answered {status}: whether its upgrade future yields the connection");
    }
    app.stop().await;
}
