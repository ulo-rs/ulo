//! A gRPC server with `.tls(..)`: a client trusting its certificate calls it, one speaking
//! plaintext does not reach a handler, the bound address is marked secure, and a certificate that
//! does not load fails `listen()` as one `Configure` entry naming the gRPC transport. Capability: gRPC TLS
//! serving (G19).

mod support;
mod wire;

use std::net::SocketAddr;
use std::time::Duration;

use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use ulo::{App, Signal, StartupError, TypeName};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{AddRequest, Sum};
use ulo_grpc::Grpc;
use ulo_net::{Tls, TlsError};
use ulo_test_certs::{Certified, localhost};

use support::{Running, within};
use wire::{Hold, Root};

fn serving(cert: &Certified) -> ulo_grpc::Server {
    ulo_grpc::Server::new("127.0.0.1:0").reflection(false).tls(Tls::from_pem(cert.cert_pem(), cert.key_pem()))
}

/// An `https` channel to `addr` verifying the server as `localhost` against `trusted`.
async fn secure(addr: SocketAddr, trusted: &Certified) -> Result<Channel, tonic::transport::Error> {
    let tls = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(trusted.cert_pem())).domain_name("localhost");
    let endpoint = Endpoint::from_shared(format!("https://{addr}")).unwrap_or_else(|error| panic!("bad endpoint: {error}"));
    let endpoint = endpoint.tls_config(tls).unwrap_or_else(|error| panic!("bad TLS config: {error}")).connect_timeout(Duration::from_secs(3));
    within("the TLS connection attempt", endpoint.connect()).await
}

#[tokio::test]
async fn a_client_trusting_the_certificate_calls_a_handler_over_tls() {
    let hold = Hold::new();
    let app = Running::start(Root(hold.clone()), serving(localhost())).await;
    let channel = secure(app.addr, localhost()).await.unwrap_or_else(|error| panic!("the trusting client did not connect: {error}"));
    let reply = CounterClient::new(channel).add(AddRequest { a: 2, b: 3 }).await.unwrap_or_else(|status| panic!("Add failed: {status:?}"));
    assert_eq!(reply.into_inner(), Sum { sum: 5 });
    app.stop().await;
}

#[tokio::test]
async fn the_bound_address_is_marked_tls() {
    let app = App::builder(Root(Hold::new()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"))
        .bind(serving(localhost()))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("listen: {error}"));
    let bound = app.addresses();
    assert_eq!(bound.len(), 1);
    assert!(bound[0].tls, "the address of a TLS server is marked secure");
    let _ = app.handle().close(Signal::new("test")).await;
}

#[tokio::test]
async fn a_plaintext_client_does_not_reach_a_handler_of_a_tls_server() {
    let hold = Hold::new();
    let app = Running::start(Root(hold.clone()), serving(localhost())).await;
    let plain = Endpoint::from_shared(format!("http://{}", app.addr)).unwrap_or_else(|error| panic!("bad endpoint: {error}")).connect_timeout(Duration::from_secs(3));
    match within("the plaintext attempt", plain.connect()).await {
        Err(_) => {}
        Ok(channel) => {
            let call = within("the plaintext call", CounterClient::new(channel).add(AddRequest { a: 100, b: 0 })).await;
            assert!(call.is_err(), "a plaintext call was answered by a TLS server");
        }
    }
    assert_eq!(hold.entered.snapshot().len(), 0, "a handler ran for a call that never completed a handshake");
    app.stop().await;
}

#[tokio::test]
async fn a_certificate_that_does_not_load_fails_listen_as_one_configure_entry_naming_grpc() {
    let broken = ulo_grpc::Server::new("127.0.0.1:0").reflection(false).tls(Tls::from_pem("not a certificate", "not a key"));
    let bound = App::builder(Root(Hold::new()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"))
        .bind(broken);
    let Err(error) = within("listen", bound.listen()).await else {
        panic!("a server with an unloadable certificate listened");
    };
    let StartupError::Configure(errors) = error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    let entries: Vec<_> = errors.iter().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].transport, TypeName::of::<Grpc>());
}

/// What `listen()` reports for a server that does not configure; one entry, the gRPC transport's.
async fn configure_entry(server: ulo_grpc::Server) -> ulo::ConfigureErrors {
    let bound = App::builder(Root(Hold::new()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"))
        .bind(server);
    let Err(error) = within("listen", bound.listen()).await else {
        panic!("a server with a configuration failure listened");
    };
    let StartupError::Configure(errors) = error else {
        panic!("expected StartupError::Configure, got {error}");
    };
    assert_eq!(errors.len(), 1, "one entry: {errors}");
    assert_eq!(errors.iter().next().map(|entry| entry.transport), Some(TypeName::of::<Grpc>()));
    errors
}

/// The entry's source.
fn source(errors: &ulo::ConfigureErrors) -> &ulo::Redacted {
    &errors.iter().next().expect("one entry").source
}

fn gateway(tls: Tls) -> ulo_grpc::Server {
    ulo_grpc::Server::new("127.0.0.1:0").reflection(false).tls(tls)
}

/// Which `TlsError` the entry's source is, so a caller can tell the cause. Each case names the
/// variant and what it carries: the certificate and key text are each refused as their own, a
/// pair that does not match is rustls's, and an unreadable file names its path.
#[tokio::test]
async fn a_tls_failure_keeps_its_tls_error_for_the_entry_to_downcast() {
    let ours = localhost();
    let other = ulo_test_certs::Certified::self_signed(&["other.invalid"]);

    let entry = configure_entry(gateway(Tls::from_pem("not a certificate", ours.key_pem()))).await;
    assert!(matches!(source(&entry).downcast_ref::<TlsError>(), Some(TlsError::Certificate(_))), "a bad certificate: {entry}");

    let entry = configure_entry(gateway(Tls::from_pem(ours.cert_pem(), "not a key"))).await;
    assert!(matches!(source(&entry).downcast_ref::<TlsError>(), Some(TlsError::PrivateKey(_))), "a bad key: {entry}");

    let entry = configure_entry(gateway(Tls::from_pem(ours.cert_pem(), other.key_pem()))).await;
    assert!(matches!(source(&entry).downcast_ref::<TlsError>(), Some(TlsError::Rustls(_))), "a key that does not match: {entry}");

    let missing = std::env::temp_dir().join("ulo-grpc-tls-no-such-certificate.pem");
    let entry = configure_entry(gateway(Tls::from_pem_files(&missing, &missing))).await;
    match source(&entry).downcast_ref::<TlsError>() {
        Some(TlsError::Read { path, .. }) => assert_eq!(path, &missing, "the file the failure names"),
        other => panic!("an unreadable certificate file was {other:?}"),
    }
}

/// A configure failure that is not TLS does not downcast to `TlsError`: a server with a zero
/// bound and a good certificate fails on the bound alone, and a bad certificate beside another
/// problem is one line of that problem's list, not the `TlsError`.
#[tokio::test]
async fn a_failure_that_is_not_tls_does_not_downcast_to_tls_error() {
    let ours = localhost();
    let zero = ulo_grpc::Server::new("127.0.0.1:0").reflection(false).max_inflight(ulo_transport::Count::Max(0));
    let entry = configure_entry(zero.tls(Tls::from_pem(ours.cert_pem(), ours.key_pem()))).await;
    assert!(source(&entry).downcast_ref::<TlsError>().is_none(), "a zero `max_inflight` read as a TLS failure: {entry}");
    assert!(source(&entry).downcast_ref::<ulo::PrepareError>().is_some(), "the zero bound is a `PrepareError`: {entry}");

    let both = ulo_grpc::Server::new("127.0.0.1:0").reflection(false).max_inflight(ulo_transport::Count::Max(0));
    let entry = configure_entry(both.tls(Tls::from_pem("not a certificate", ours.key_pem()))).await;
    assert!(source(&entry).downcast_ref::<TlsError>().is_none(), "two problems are the report's list: {entry}");
    assert!(source(&entry).downcast_ref::<ulo::PrepareError>().is_some(), "the list is a `PrepareError`: {entry}");
}
