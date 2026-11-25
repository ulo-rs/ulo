//! An application's own `UpgradeHandler` answering 101 to an HTTP/1.x request carrying
//! `Transfer-Encoding`, whatever protocol it switches to, has the 101 replaced by the 400 problem
//! document with `Connection: close`, and the request's upgrade future does not yield the
//! connection (RFC 9112 §6.1, transports DESIGN decision 338, F702). The head check refuses only a
//! WebSocket handshake carrying it, so a request whose upgrade is never answered 101 keeps its
//! framing: `curl --http2` sends `Upgrade: h2c` on a chunked `POST` upload, and the route, or the
//! upgrade handler declining the switch, answers it as before. The wire is held by the HTTP
//! conformance scenario `handshake_with_transfer_encoding_refused` on every host.

mod o1_support;

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use futures::io::Cursor;
use http::{Method, StatusCode, Version};
use o1_support::{Call, Reply, Running, read, start};
use tokio::sync::oneshot;
use ulo::{BoxError, BoxFuture, ModuleDef, controller, handlers};
use ulo_http::{Bytes, HttpBody, OnUpgrade, Request, Response, Routing, UpgradeHandler, Upgrades, Upgraded};

/// The upgrade futures the handler took, each with its request's `x-id`: the tests run in
/// parallel.
static TAKEN: Mutex<Vec<(String, OnUpgrade)>> = Mutex::new(Vec::new());

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// The detail the replaced 101 carries.
const UPGRADE_TE: &str = "an upgrade request carries `Transfer-Encoding`";

#[controller]
struct Upload;

#[handlers]
impl Upload {
    #[ulo_http::post("/upload")]
    fn upload(&self, body: Bytes) -> String {
        String::from_utf8_lossy(&body).into_owned()
    }
}

/// Switches every request on `/h2c` to the protocol its `Upgrade` names, as an application's own
/// `h2c` handler would, and declines one carrying `x-decline` with a 200 that reads nothing.
struct Switches;

impl UpgradeHandler for Switches {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/h2c".into()]
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let decline = req.headers().contains_key("x-decline");
        let protocol = req.headers().get(http::header::UPGRADE).cloned();
        let id = req.headers().get("x-id").and_then(|id| id.to_str().ok()).unwrap_or_default().to_owned();
        if let Some(pending) = req.upgrade {
            TAKEN.lock().unwrap_or_else(PoisonError::into_inner).push((id, pending));
        }
        Box::pin(async move {
            let mut response = Response::new(HttpBody::empty());
            if decline {
                return response;
            }
            *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
            response.headers_mut().insert(http::header::CONNECTION, http::HeaderValue::from_static("upgrade"));
            if let Some(protocol) = protocol {
                response.headers_mut().insert(http::header::UPGRADE, protocol);
            }
            response
        })
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Upload>();
    m.meta::<Upgrades>().register(Switches);
}

/// Sends `call` over `version` with an upgrade future the backend fulfils once the service has
/// answered, as hyper does after the connection's last exchange: the answer, its version, and
/// whether the future yielded the connection, `None` when no upgrade handler took it.
async fn exchange(app: &Running, call: Call, version: Version) -> (Reply, Version, Option<bool>) {
    let (handed, fired) = oneshot::channel::<()>();
    let pending = OnUpgrade::new(async move {
        fired.await.map_err(|_| BoxError::from("the backend dropped the connection"))?;
        Ok(Upgraded::from_futures(Cursor::new(Vec::new())))
    });
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();
    let mut req = call.header("x-id", &id).into_request();
    req.head.version = version;
    req.upgrade = Some(pending);
    let response = app.service().call(req).await;
    let answered = response.version();
    let reply = read(response).await;
    let _ = handed.send(());
    let taken = {
        let mut taken = TAKEN.lock().unwrap_or_else(PoisonError::into_inner);
        taken.iter().position(|(taker, _)| *taker == id).map(|at| taken.remove(at).1)
    };
    let yielded = match taken {
        Some(taken) => Some(taken.await.is_ok()),
        None => None,
    };
    (reply, answered, yielded)
}

fn h2c(method: Method) -> Call {
    Call::new(method, "/h2c").header("host", "a.example").header("connection", "Upgrade, HTTP2-Settings").header("upgrade", "h2c")
}

type Case = (&'static str, fn() -> Call);

/// Planted break: the guard removed, which sends the 101 and hands the connection over.
#[tokio::test]
async fn an_upgrade_handler_s_101_to_a_request_carrying_transfer_encoding_is_replaced_by_400_and_close() {
    let app = start(register).await;
    let cases: [Case; 3] = [
        ("`GET` with `Upgrade: h2c` and `Transfer-Encoding: chunked`", || h2c(Method::GET).header("transfer-encoding", "chunked")),
        ("a chunked `POST` with `Upgrade: h2c`", || h2c(Method::POST).header("transfer-encoding", "chunked").chunks(["hello"])),
        ("`Upgrade: websocketx` and `Transfer-Encoding: gzip, chunked`", || {
            Call::get("/h2c").header("host", "a.example").header("upgrade", "websocketx").header("transfer-encoding", "gzip, chunked")
        }),
    ];
    for (what, call) in cases {
        for version in [Version::HTTP_10, Version::HTTP_11] {
            let (reply, answered, yielded) = exchange(&app, call(), version).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{what} over {version:?}: {}", reply.text());
            assert_eq!(reply.header("connection"), Some("close"), "{what} over {version:?}");
            assert_eq!(reply.header("upgrade"), None, "{what} over {version:?}: the 400 kept the 101's `Upgrade`");
            assert_eq!(answered, version, "{what}: the 400 is written in the request's version, which hyper reads to end the connection");
            assert_eq!(reply.json()["detail"], UPGRADE_TE, "{what} over {version:?}");
            assert!(matches!(reply.routing, Some(Routing::Unrouted)), "{what} over {version:?}: {:?}", reply.routing);
            assert_eq!(yielded, Some(false), "{what} over {version:?}: whether the upgrade future yielded the connection after the 400");
        }
    }
    app.stop().await;
}

/// The same handler switches a request framed by `Content-Length` or carrying no body, and a
/// request carrying `Transfer-Encoding` that is never answered 101 keeps its framing: a chunked
/// `POST` upload with `Upgrade: h2c` to a route is read and answered, and one the upgrade handler
/// declines is answered as the handler wrote it, with no `Connection: close` added. Planted
/// break: the guard replacing any answer to a request carrying `Transfer-Encoding`.
#[tokio::test]
async fn a_request_not_switched_or_not_framed_by_transfer_encoding_is_answered_as_before() {
    let app = start(register).await;
    let switched: [Case; 2] = [
        ("`Upgrade: h2c` with `Content-Length`", || h2c(Method::POST).header("content-length", "5").body("hello")),
        ("`Upgrade: h2c` with no body", || h2c(Method::GET)),
    ];
    for (what, call) in switched {
        let (reply, _, yielded) = exchange(&app, call(), Version::HTTP_11).await;
        assert_eq!(reply.status, StatusCode::SWITCHING_PROTOCOLS, "{what}: {}", reply.text());
        assert_eq!(reply.header("upgrade"), Some("h2c"), "{what}");
        assert_eq!(yielded, Some(true), "{what}: whether the switched request's upgrade future yielded the connection");
    }

    let upload = || Call::new(Method::POST, "/upload").header("host", "a.example").header("upgrade", "h2c").header("transfer-encoding", "chunked").chunks(["hel", "lo"]);
    let (reply, _, yielded) = exchange(&app, upload(), Version::HTTP_11).await;
    assert_eq!(reply.status, StatusCode::OK, "a chunked `POST` upload with `Upgrade: h2c` to a route: {}", reply.text());
    assert_eq!(reply.text(), "hello", "the upload's body is read by its `Transfer-Encoding`");
    assert_eq!(reply.header("connection"), None, "the upload's answer closes the connection");
    assert_eq!(yielded, None, "an upgrade handler took the upload to a route");

    let declined = h2c(Method::POST).header("transfer-encoding", "chunked").header("x-decline", "1").chunks(["hello"]);
    let (reply, _, yielded) = exchange(&app, declined, Version::HTTP_11).await;
    assert_eq!(reply.status, StatusCode::OK, "a chunked upload the upgrade handler declines: {}", reply.text());
    assert_eq!(reply.header("connection"), None, "the declined upload's answer closes the connection");
    assert_eq!(yielded, Some(false), "the declined upload's upgrade future yielded the connection");
    app.stop().await;
}
