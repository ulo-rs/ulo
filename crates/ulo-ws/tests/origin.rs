//! The handshake's `Origin` check on the HTTP server's port, where a gateway is reached through
//! the hand-off `WsModule` registers: a same-origin page and a client sending no `Origin` are
//! admitted unset, a cross-origin page is refused 403 before the connection phase runs, a `Cors`
//! entry allowing that origin does not admit it, `WsModule::origins` replaces the default, and a
//! listed entry that is not an origin fails `listen()`.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, controller, handlers};
use ulo_http::{Cors, PreDispatch};
use ulo_transport::ErrorKind;
use ulo_ws::{ConnectCx, ConnectRefused, OnConnect, Origins, WsModule};

use support::{Running, hang_up, upgrade};

/// The upgrade header naming the visitor; `OnConnect` refuses an upgrade without it 401.
const USER: &str = "x-user";

const EVIL: &str = "https://evil.example";

/// How many times `OnConnect` ran.
#[derive(Clone, Default)]
struct Connects(Arc<AtomicUsize>);

#[controller]
struct Door {
    connects: ulo::Dep<Connects>,
}

#[handlers]
#[ulo_ws::gateway(path = "/door", refuse = handshake)]
impl Door {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: ulo_ws::Payload<String>) -> String {
        text.0
    }
}

impl OnConnect for Door {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        self.connects.0.fetch_add(1, Ordering::SeqCst);
        match cx.head().headers().get(USER) {
            Some(_) => Ok(()),
            None => Err(ConnectRefused::kind(ErrorKind::Unauthorized, "who are you?")),
        }
    }
}

struct Root {
    origins: Option<Origins>,
    cors: bool,
    connects: Connects,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let module = match &self.origins {
            Some(origins) => WsModule::for_root().origins(origins.clone()),
            None => WsModule::for_root(),
        };
        m.import(module);
        m.value(self.connects.clone());
        if self.cors {
            m.meta::<PreDispatch>().apply_value(Cors::new().allow_origin(EVIL));
        }
        m.controller::<Door>();
    }
}

async fn start(origins: Option<Origins>, cors: bool) -> (Running, Connects) {
    let connects = Connects::default();
    let app = Running::start(Root { origins, cors, connects: connects.clone() }, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    (app, connects)
}

/// The status `origin` is answered with on `/door`, the visitor named, hanging up on a 101.
async fn status(app: &Running, origin: Option<&str>) -> (u16, String) {
    let mut headers = vec![(USER, "ada")];
    headers.extend(origin.map(|origin| ("Origin", origin)));
    let answer = upgrade(app.addr, "/door", &headers).await;
    if let Some(socket) = answer.socket {
        hang_up(socket).await;
    }
    (answer.status, answer.body)
}

#[tokio::test]
async fn unset_a_same_origin_page_and_a_client_without_origin_are_admitted_and_a_cross_origin_page_is_refused_403() {
    let (app, _) = start(None, false).await;
    let own = format!("http://{}", app.addr);

    assert_eq!(status(&app, None).await.0, 101, "no `Origin`: a client outside a browser");
    assert_eq!(status(&app, Some(&own)).await.0, 101, "the server's own origin");
    for origin in [EVIL, "null", &format!("http://{}", app.addr).replace("127.0.0.1", "localhost")] {
        let (code, body) = status(&app, Some(origin)).await;
        assert_eq!(code, 403, "{origin}: {body}");
        assert!(body.starts_with("forbidden: ") && body.contains("`origins([..])`"), "{origin}: {body}");
    }
    app.stop().await;
}

/// The check runs before the connection phase: an upgrade `OnConnect` would refuse 401 is
/// refused 403 by its `Origin`, and `OnConnect` never runs.
#[tokio::test]
async fn a_refused_origin_is_answered_before_the_connection_phase_runs() {
    let (app, connects) = start(None, false).await;

    let refused = upgrade(app.addr, "/door", &[("Origin", EVIL)]).await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(connects.0.load(Ordering::SeqCst), 0, "`OnConnect` ran for a refused origin");
    let unnamed = upgrade(app.addr, "/door", &[]).await;
    assert_eq!(unnamed.status, 401, "the connection phase still runs for an admitted origin: {}", unnamed.body);
    assert_eq!(connects.0.load(Ordering::SeqCst), 1);
    app.stop().await;
}

/// A `Cors` entry allowing an origin decorates HTTP responses and admits no handshake.
#[tokio::test]
async fn a_cors_entry_allowing_an_origin_does_not_admit_its_handshake() {
    let (app, _) = start(None, true).await;
    assert_eq!(status(&app, Some(EVIL)).await.0, 403);
    app.stop().await;
}

#[tokio::test]
async fn ws_module_origins_replaces_the_default_with_a_list_or_any() {
    let (listed, _) = start(Some(Origins::from(["https://app.example"])), false).await;
    let own = format!("http://{}", listed.addr);
    assert_eq!(status(&listed, Some("https://app.example")).await.0, 101);
    assert_eq!(status(&listed, Some("https://app.example:443")).await.0, 101, "the default port written out");
    assert_eq!(status(&listed, Some("http://app.example")).await.0, 403, "the scheme is compared");
    assert_eq!(status(&listed, Some(&own)).await.0, 403, "a same-origin page is admitted only when listed");
    assert_eq!(status(&listed, None).await.0, 101);
    listed.stop().await;

    let (any, _) = start(Some(Origins::Any), false).await;
    assert_eq!(status(&any, Some(EVIL)).await.0, 101);
    assert_eq!(status(&any, Some("null")).await.0, 101);
    any.stop().await;
}

#[tokio::test]
async fn a_listed_entry_that_is_not_an_origin_fails_listen_naming_it() {
    let refused: StartupError = async {
        App::builder(Root { origins: Some(Origins::from(["null", "https://ok.example"])), cors: false, connects: Connects::default() })
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
            .listen()
            .await
            .err()
            .expect("`listen()` refuses an entry that is not an origin")
    }
    .await;
    let text = refused.to_string();
    assert!(text.contains("`origins([..])` lists `null`"), "{text}");
    assert!(!text.contains("ok.example"), "{text}");
}
