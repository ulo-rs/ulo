//! `HttpCx::client_ip` behind the server's `trusted_proxies`, on a backend and on an embedding
//! (transports DESIGN §3.6, §3.8): nothing is trusted unset, the walk answers the first untrusted
//! hop from the right behind a trusted peer, and a text that is not a CIDR range is refused at
//! `listen()` naming it. The walk's cases are the unit table in `src/client_ip.rs`; these pin the
//! setting reaching the handler's context on both servers.

mod common;

use std::net::SocketAddr;

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, controller, handlers};
use ulo_http::embed::{Embed, EmbedLimits, Embedded, Service};
use ulo_http::{AppService, Cidr, ConnInfo, HttpBody, HttpCx, Request, Response};
use ulo_runtime_tokio::Tokio;

use common::{Keeper, written};

const PROXY: &str = "10.0.0.1:443";
const STRANGER: &str = "203.0.113.9:443";
const SPOOFED: &str = "6.6.6.6, 198.51.100.7";

#[controller]
struct Who;

#[handlers]
impl Who {
    #[ulo_http::get("/who")]
    fn who(&self, cx: HttpCx) -> String {
        let ip = cx.client_ip().map_or_else(|| "none".to_owned(), |ip| ip.to_string());
        let peer = cx.client_addr().map_or_else(|| "none".to_owned(), |addr| addr.ip().to_string());
        format!("{ip} via {peer}")
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Who>();
    }
}

struct Host;

impl Embed for Host {
    const NAME: &'static str = "test-host";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
    }
}

fn request(peer: Option<&str>, forwarded_for: Option<&str>) -> Request {
    let mut builder = http::Request::get("/who");
    if let Some(chain) = forwarded_for {
        builder = builder.header("x-forwarded-for", chain);
    }
    let (head, ()) = builder.body(()).expect("the request builds").into_parts();
    let mut conn = ConnInfo::new(http::Version::HTTP_11);
    if let Some(peer) = peer {
        conn = conn.peer(peer.parse::<SocketAddr>().expect("a socket address"));
    }
    Request { head, body: HttpBody::empty(), conn, upgrade: None }
}

async fn text(response: Response) -> String {
    let (status, body) = written(response).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    String::from_utf8(body).expect("UTF-8")
}

async fn served(server: ulo_http::Server<Keeper>, keeper: &Keeper) -> (App<ulo::app::Bound>, AppService) {
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the app listens");
    (app, keeper.service())
}

async fn embedded(server: Embedded<Host>) -> (App<ulo::app::Bound>, Service<Host>) {
    let handle = server.handle();
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server)
        .listen()
        .await
        .expect("the app listens");
    (app, handle.service())
}

async fn via_embedding(service: &Service<Host>, req: Request) -> String {
    let head = req.head.clone();
    text(service.respond(&head, req).await).await
}

#[tokio::test]
async fn unset_nothing_is_trusted_and_the_peer_is_the_client() {
    let keeper = Keeper::default();
    let (_app, svc) = served(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()), &keeper).await;
    assert_eq!(text(svc.call(request(Some(PROXY), Some(SPOOFED))).await).await, "10.0.0.1 via 10.0.0.1");
}

#[tokio::test]
async fn a_backend_server_walks_the_chain_behind_a_trusted_peer() {
    let keeper = Keeper::default();
    let server = ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()).trusted_proxies(["10.0.0.0/8"]);
    let (_app, svc) = served(server, &keeper).await;
    assert_eq!(text(svc.call(request(Some(PROXY), Some(SPOOFED))).await).await, "198.51.100.7 via 10.0.0.1", "behind the trusted proxy");
    assert_eq!(text(svc.call(request(Some(STRANGER), Some(SPOOFED))).await).await, "203.0.113.9 via 203.0.113.9", "an untrusted peer");
    assert_eq!(text(svc.call(request(None, Some(SPOOFED))).await).await, "none via none", "no peer address");
}

#[tokio::test]
async fn a_range_given_as_a_cidr_value_is_trusted_as_its_text_is() {
    let keeper = Keeper::default();
    let range: Cidr = "10.0.0.0/8".parse().expect("a range");
    let server = ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()).trusted_proxies([range]);
    let (_app, svc) = served(server, &keeper).await;
    assert_eq!(text(svc.call(request(Some(PROXY), Some(SPOOFED))).await).await, "198.51.100.7 via 10.0.0.1");
}

#[tokio::test]
async fn an_embedding_takes_the_same_setting() {
    let (_app, service) = embedded(Embedded::<Host>::new().trusted_proxies(["10.0.0.0/8"])).await;
    assert_eq!(via_embedding(&service, request(Some(PROXY), Some(SPOOFED))).await, "198.51.100.7 via 10.0.0.1");
    assert_eq!(via_embedding(&service, request(Some(STRANGER), Some(SPOOFED))).await, "203.0.113.9 via 203.0.113.9");
    assert_eq!(via_embedding(&service, request(None, Some(SPOOFED))).await, "none via none", "a host that supplies no peer");

    let (_unset, service) = embedded(Embedded::<Host>::new()).await;
    assert_eq!(via_embedding(&service, request(Some(PROXY), Some(SPOOFED))).await, "10.0.0.1 via 10.0.0.1", "unset");
}

/// The `Configure` entries of a `listen()` that failed, as text.
async fn refused(server: impl ulo::Server<Transport = ulo_http::Http>) -> Vec<String> {
    let bound = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects").bind(server);
    let Err(error) = bound.listen().await else { panic!("a server trusting a text that is no range listened") };
    let StartupError::Configure(errors) = error else { panic!("expected `Configure`, got {error}") };
    errors.iter().map(|entry| entry.to_string()).collect()
}

#[tokio::test]
async fn a_text_that_is_no_range_is_refused_at_listen_naming_it() {
    let keeper = Keeper::default();
    let backend = refused(ulo_http::Server::with_backend("127.0.0.1:0", keeper).trusted_proxies(["10.0.0.0/33", "fd00::/8", "proxy.local"])).await;
    let embedding = refused(Embedded::<Host>::new().trusted_proxies(["10.0.0.0/33", "fd00::/8", "proxy.local"])).await;
    for entries in [backend, embedding] {
        let text = entries.join("\n");
        assert_eq!(entries.len(), 1, "one entry for the HTTP server: {text}");
        assert!(text.contains("`10.0.0.0/33` is not a CIDR range: the prefix length 33 is over 32"), "{text}");
        assert!(text.contains("`proxy.local` is not a CIDR range"), "{text}");
        assert!(!text.contains("fd00::/8"), "a valid range was refused: {text}");
    }
}
