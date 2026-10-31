//! `Tls::from_config`: a rustls `ServerConfig` the caller built serves HTTPS, which is how a
//! server verifies its clients' certificates, and one whose ALPN would switch HTTP/2 off is
//! refused when the app listens.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use ulo::{Signal, StartupError};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use ulo_net::rustls::server::WebPkiClientVerifier;
use ulo_net::rustls::{self, ClientConfig, ServerConfig};
use ulo_test_certs::{Ca, Certified};

const PATIENCE: Duration = Duration::from_secs(5);

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn identity(cert: &Certified) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let chain = CertificateDer::pem_slice_iter(cert.cert_pem().as_bytes()).collect::<Result<Vec<_>, _>>().expect("the certificate parses");
    (chain, PrivateKeyDer::from_pem_slice(cert.key_pem().as_bytes()).expect("the key parses"))
}

/// A server configuration with `alpn`, requiring a client certificate issued by `ca` when
/// `verify_clients`.
fn server_config(server: &Certified, ca: &Ca, verify_clients: bool, alpn: &[&[u8]]) -> Arc<ServerConfig> {
    let builder = ServerConfig::builder_with_provider(provider()).with_safe_default_protocol_versions().expect("the default versions");
    let builder = if verify_clients {
        let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(ca.roots()), provider()).build().expect("the verifier builds");
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let (chain, key) = identity(server);
    let mut config = builder.with_single_cert(chain, key).expect("the pair is consistent");
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    Arc::new(config)
}

struct Served {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

async fn start(config: Arc<ServerConfig>) -> Result<Served, StartupError> {
    let server = ulo_http_hyper::Server::new("127.0.0.1:0").tls(Tls::from_config(config));
    let app = ulo_http_conformance::app(ulo_runtime_tokio::Tokio::current()).await.bind(server).listen().await?;
    let [bound] = app.addresses().try_into().expect("one bound address");
    assert!(bound.tls, "the bound address reports TLS");
    let (stop, stopped) = oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        let _ = app
            .serve(async move {
                let _ = stopped.await;
                Signal::new("tls-config")
            })
            .await;
    });
    Ok(Served { addr: bound.addr, stop, serving })
}

impl Served {
    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }

    /// The protocol a client offering `alpn` and presenting no certificate settles on.
    async fn negotiated(&self, ca: &Ca, alpn: &[&[u8]]) -> Option<Vec<u8>> {
        let mut config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("the default versions")
            .with_root_certificates(ca.roots())
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let tcp = TcpStream::connect(self.addr).await.expect("the server accepts");
        let name = ServerName::try_from("localhost").expect("a DNS name");
        let tls = tokio::time::timeout(PATIENCE, TlsConnector::from(Arc::new(config)).connect(name, tcp)).await.expect("in time").expect("the handshake succeeds");
        tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec)
    }

    /// One `GET /hit` over HTTP/1.1 trusting `ca`, presenting `client` when given: the reply, and
    /// the protocol ALPN settled on.
    async fn get(&self, ca: &Ca, client: Option<&Certified>, alpn: &[&[u8]]) -> std::io::Result<(String, Option<Vec<u8>>)> {
        let builder = ClientConfig::builder_with_provider(provider()).with_safe_default_protocol_versions().expect("the default versions").with_root_certificates(ca.roots());
        let mut config = match client {
            Some(client) => {
                let (chain, key) = identity(client);
                builder.with_client_auth_cert(chain, key).expect("the client pair is consistent")
            }
            None => builder.with_no_client_auth(),
        };
        config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
        let tcp = TcpStream::connect(self.addr).await?;
        let name = ServerName::try_from("localhost").expect("a DNS name");
        let mut tls = tokio::time::timeout(PATIENCE, TlsConnector::from(Arc::new(config)).connect(name, tcp)).await.expect("the handshake ends in time")?;
        let negotiated = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        tls.write_all(b"GET /hit HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").await?;
        let mut raw = Vec::new();
        tokio::time::timeout(PATIENCE, tls.read_to_end(&mut raw)).await.expect("the read ends in time")?;
        Ok((String::from_utf8_lossy(&raw).into_owned(), negotiated))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handed_configuration_serves_https_and_the_server_adds_the_alpn_it_needs() {
    let ca = Ca::new();
    let server = start(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca, false, &[])).await.expect("the app listens");

    assert_eq!(server.negotiated(&ca, &[b"h2", b"http/1.1"]).await.as_deref(), Some(b"h2".as_slice()), "the empty list became the server's");
    let (reply, alpn) = server.get(&ca, None, &[b"http/1.1"]).await.expect("an HTTP/1.1 client is served");
    assert!(reply.starts_with("HTTP/1.1 200") && reply.ends_with("hit"), "{reply}");
    assert_eq!(alpn.as_deref(), Some(b"http/1.1".as_slice()));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_further_protocol_in_the_handed_configuration_is_not_offered() {
    let ca = Ca::new();
    let server = start(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca, false, &[b"acme-tls/1", b"http/1.1", b"h2"])).await.expect("the app listens");

    assert_eq!(server.negotiated(&ca, &[b"acme-tls/1", b"h2", b"http/1.1"]).await.as_deref(), Some(b"h2".as_slice()), "h2 is preferred");
    assert_eq!(server.negotiated(&ca, &[b"acme-tls/1", b"http/1.1"]).await.as_deref(), Some(b"http/1.1".as_slice()));
    let only_further = server.get(&ca, None, &[b"acme-tls/1"]).await;
    assert!(only_further.is_err(), "a client offering only the further protocol gets an alert, got {only_further:?}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_a_certificate_is_refused_and_one_with_it_is_served() {
    let ca = Ca::new();
    let server = start(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca, true, &[])).await.expect("the app listens");

    // TLS 1.3 finishes the client's handshake before the server checks its certificate, so the
    // refusal arrives on the first read.
    let refused = server.get(&ca, None, &[b"http/1.1"]).await;
    assert!(!matches!(&refused, Ok((reply, _)) if reply.contains("200")), "a client with no certificate got {refused:?}");
    let stranger = Ca::new().issue(&["client"]);
    let forged = server.get(&ca, Some(&stranger), &[b"http/1.1"]).await;
    assert!(!matches!(&forged, Ok((reply, _)) if reply.contains("200")), "a certificate from another authority got {forged:?}");

    let client = ca.issue(&["client"]);
    let (reply, _) = server.get(&ca, Some(&client), &[b"http/1.1"]).await.expect("the certified client is served");
    assert!(reply.starts_with("HTTP/1.1 200") && reply.ends_with("hit"), "{reply}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configuration_that_would_switch_http_2_off_fails_listen_as_configure() {
    let ca = Ca::new();
    let Err(error) = start(server_config(&ca.issue(&["localhost"]), &ca, false, &[b"http/1.1"])).await else { panic!("listen accepted an ALPN list without h2") };
    let StartupError::Configure(report) = &error else { panic!("{error:?}") };
    assert_eq!(report.into_iter().count(), 1, "{error}");
    let text = error.to_string();
    assert!(text.contains("ALPN") && text.contains("h2"), "{text}");
}
