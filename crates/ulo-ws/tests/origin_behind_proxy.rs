//! The same-origin `Origin` check behind a reverse proxy, on the HTTP server's port (transports
//! DESIGN §4.1): the proxy rewrites `Host` to the backend's address and reports the public host,
//! which the check compares only under `behind_proxy`, only from a trusted peer, and only from the
//! header the setting names. The test client connects from 127.0.0.1, so `127.0.0.1/32` makes it
//! the trusted proxy and `10.0.0.0/8` leaves it untrusted.

mod support;

use ulo::{Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_http::ProxyHeader;
use ulo_ws::{Payload, WsModule};

use support::{Running, hang_up, upgrade};

/// The page's origin, which the proxy reports as the host the browser addressed.
const PUBLIC: &str = "https://app.example";

#[controller]
struct Door;

#[handlers]
#[ulo_ws::gateway(path = "/door")]
impl Door {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Door>();
    }
}

/// The status a handshake from the page carrying `forwarded` gets from a server built by
/// `server`. The client's `Host` is the backend's own address, as a proxy rewriting it sends.
async fn status(server: ulo_http_hyper::Server, forwarded: &[(&str, &str)]) -> u16 {
    let app = Running::start_configured(Root, server, ulo_runtime_tokio::Tokio::current(), ulo::AppBuilder::unserved::<ulo_ws::Ws>).await;
    let mut headers = vec![("Origin", PUBLIC)];
    headers.extend_from_slice(forwarded);
    let answer = upgrade(app.addr, "/door", &headers).await;
    let status = answer.status;
    if let Some(socket) = answer.socket {
        hang_up(socket).await;
    }
    app.stop().await;
    status
}

fn server() -> ulo_http_hyper::Server {
    ulo_http_hyper::Server::new("127.0.0.1:0")
}

#[tokio::test]
async fn a_proxied_handshake_is_admitted_by_the_host_its_trusted_proxy_reports() {
    let xff = [("X-Forwarded-Host", "app.example")];
    assert_eq!(status(server().behind_proxy(ProxyHeader::XForwardedFor, ["127.0.0.1/32"]), &xff).await, 101, "behind a trusted proxy");
    assert_eq!(status(server(), &xff).await, 403, "unset, the backend's `Host` is compared");

    let forwarded = [("Forwarded", "for=203.0.113.5;host=app.example;proto=https")];
    assert_eq!(status(server().behind_proxy(ProxyHeader::Forwarded, ["127.0.0.1/32"]), &forwarded).await, 101, "`host=` in `Forwarded`");
}

#[tokio::test]
async fn a_forwarded_host_from_an_untrusted_peer_or_in_the_header_not_named_is_not_believed() {
    let xff = [("X-Forwarded-Host", "app.example")];
    assert_eq!(status(server().behind_proxy(ProxyHeader::XForwardedFor, ["10.0.0.0/8"]), &xff).await, 403, "an untrusted peer");
    assert_eq!(status(server().behind_proxy(ProxyHeader::Forwarded, ["127.0.0.1/32"]), &xff).await, 403, "`X-Forwarded-Host` under `Forwarded`");
    let forwarded = [("Forwarded", "host=app.example")];
    assert_eq!(status(server().behind_proxy(ProxyHeader::XForwardedFor, ["127.0.0.1/32"]), &forwarded).await, 403, "`host=` under `XForwardedFor`");
}
