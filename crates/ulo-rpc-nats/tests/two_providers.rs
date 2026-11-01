//! F422: in a build linking both of rustls's crypto providers with neither installed, `ring`
//! through async-nats and `aws-lc-rs` through this crate's dev-dependency on rustls, a `tls://`
//! link is refused naming the fix, where async-nats's `ClientConfig::builder()` panicked its
//! connection. This binary installs no provider; the link's own tests, which install one, run in
//! another process.

use std::sync::Arc;

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError};
use ulo_rpc::{Link, RpcClient, Server};
use ulo_rpc_nats::Nats;
use ulo_runtime_tokio::Tokio;
use ulo_transport::ErrorKind;

const REFUSAL: &str =
    "this process links more than one rustls crypto provider and installs none; install one with `CryptoProvider::install_default` before the app starts";

/// The premise: no provider is installed, and rustls's own resolution panics.
fn two_providers_and_none_installed() {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none(), "a crypto provider is installed");
    assert!(
        std::panic::catch_unwind(|| drop(rustls::ClientConfig::builder())).is_err(),
        "rustls resolved a provider: the build does not link both"
    );
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

async fn app() -> App<ulo::app::Connected> {
    App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects")
}

/// A server on `tls://` is refused at `bind` as a `Configure` refusal carrying the fix. Nothing
/// listens at the url: the refusal comes from `prepare`, before any connection.
#[tokio::test]
async fn a_tls_server_is_refused_at_bind_naming_the_fix() {
    two_providers_and_none_installed();
    match app().await.bind(Server::new(Nats::url("tls://127.0.0.1:1"))).listen().await {
        Err(error @ StartupError::Configure(_)) => {
            let refused = error.to_string();
            assert!(refused.contains(REFUSAL), "the refusal does not name the fix: {refused}");
        }
        Err(other) => panic!("the bind failed other than as a `Configure` refusal: {other:?}"),
        Ok(_) => panic!("a server in a build with two providers and none installed listened"),
    }
}

/// Every call of a client on `tls://` fails `Internal` with `reason: "misconfigured_link"`,
/// carrying the fix.
#[tokio::test]
async fn every_call_of_a_tls_client_fails_misconfigured_naming_the_fix() {
    two_providers_and_none_installed();
    let client = RpcClient::new(Nats::url("tls://127.0.0.1:1"), Arc::new(Tokio::current())).expect("the link is usable");
    for _ in 0..2 {
        let failed = client.request::<_, u32>("nats.providers", &()).await.expect_err("a call over a link refused for its providers");
        assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Internal, Some("misconfigured_link")), "{}", failed.message());
        assert!(failed.message().contains(REFUSAL), "the call does not name the fix: {}", failed.message());
    }
}

/// A `nats://` link is not refused for its providers: its server's `prepare` passes, and its
/// client's calls fail as the unreachable url fails them, `Unavailable`.
#[tokio::test]
async fn a_plain_link_is_not_refused_for_its_providers() {
    two_providers_and_none_installed();
    let app = app().await;
    let mut server = Nats::url("nats://127.0.0.1:1");
    server.prepare(&app.handle()).await.expect("a plain server's link prepares");
    let client = RpcClient::new(Nats::url("nats://127.0.0.1:1"), Arc::new(Tokio::current())).expect("the link is usable");
    let failed = client.request::<_, u32>("nats.providers", &()).await.expect_err("a call to a server that is not there");
    assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, None), "{}", failed.message());
}
