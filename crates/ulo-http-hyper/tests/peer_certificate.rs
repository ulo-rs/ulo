//! The client certificate a guard reads comes from the TLS session, never from the request: a
//! server whose rustls configuration verifies clients puts the chain the verifier accepted in
//! `ConnInfo::tls`, and a header naming a certificate, from a client with none or over plain
//! HTTP, puts nothing there.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use ulo::{App, BoxError, Guard, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_http::{Http, HttpCx};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use ulo_net::rustls::server::WebPkiClientVerifier;
use ulo_net::rustls::{self, ClientConfig, ServerConfig};
use ulo_test_certs::{Ca, Certified};

const PATIENCE: Duration = Duration::from_secs(5);

/// The headers a proxy terminating TLS commonly writes the client's certificate into: nginx's,
/// Envoy's, AWS's load balancers' and a generic spelling or two.
const FORGED: [&str; 7] =
    ["x-client-cert", "x-ssl-client-cert", "ssl-client-cert", "x-forwarded-client-cert", "x-amzn-mtls-clientcert", "x-ssl-cert", "X-Client-Certificate"];

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn identity(cert: &Certified) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let chain = CertificateDer::pem_slice_iter(cert.cert_pem().as_bytes()).collect::<Result<Vec<_>, _>>().expect("the certificate parses");
    (chain, PrivateKeyDer::from_pem_slice(cert.key_pem().as_bytes()).expect("the key parses"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The leaf the TLS session verified, or `None`.
fn verified_leaf(cx: &HttpCx) -> Option<&[u8]> {
    cx.conn().tls.as_ref()?.peer_certificates.as_ref()?.leaf()
}

/// Admits a request whose connection presented a verified client certificate.
struct ClientCertificate;

impl Guard<Http> for ClientCertificate {
    async fn can_activate(&self, cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(verified_leaf(cx).is_some())
    }
}

#[controller]
struct Whoami;

#[handlers]
impl Whoami {
    /// The verified leaf, hex.
    #[ulo_http::get("/whoami")]
    #[guards(value = ClientCertificate)]
    fn whoami(&self, cx: HttpCx) -> String {
        verified_leaf(&cx).map(hex).unwrap_or_default()
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Whoami>();
    }
}

/// A server for `server`, verifying client certificates `ca` issued and admitting a client
/// that presents none, so the guard is what decides.
fn server_config(server: &Certified, ca: &Ca) -> Arc<ServerConfig> {
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(ca.roots()), provider())
        .allow_unauthenticated()
        .build()
        .expect("the verifier builds");
    let (chain, key) = identity(server);
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .expect("the pair is consistent");
    Arc::new(config)
}

struct Served {
    addr: SocketAddr,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn start(tls: Option<Arc<ServerConfig>>) -> Served {
    let mut server = ulo_http_hyper::Server::new("127.0.0.1:0");
    if let Some(config) = tls {
        server = server.tls(Tls::from_config(config));
    }
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("{error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .bind(server)
        .listen()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let [bound] = app.addresses().try_into().expect("one bound address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Served { addr: bound.addr, handle, serving }
}

impl Served {
    async fn stop(self) {
        let _ = self.handle.close(Signal::new("peer-certificate")).await;
        let _ = self.serving.await;
    }

    /// `GET /whoami` carrying every [`FORGED`] header set to `forged`, over plain HTTP.
    async fn plain(&self, forged: &str) -> String {
        let mut tcp = TcpStream::connect(self.addr).await.expect("the server accepts");
        tcp.write_all(request(forged).as_bytes()).await.expect("the request is written");
        let mut raw = Vec::new();
        tokio::time::timeout(PATIENCE, tcp.read_to_end(&mut raw)).await.expect("the read ends in time").expect("the read succeeds");
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// The same over TLS trusting `ca`, presenting `client` when given.
    async fn secure(&self, ca: &Ca, client: Option<&Certified>, forged: &str) -> std::io::Result<String> {
        let builder = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("the default versions")
            .with_root_certificates(ca.roots());
        let config = match client {
            Some(client) => {
                let (chain, key) = identity(client);
                builder.with_client_auth_cert(chain, key).expect("the client pair is consistent")
            }
            None => builder.with_no_client_auth(),
        };
        let tcp = TcpStream::connect(self.addr).await?;
        let name = ServerName::try_from("localhost").expect("a DNS name");
        let mut tls = tokio::time::timeout(PATIENCE, TlsConnector::from(Arc::new(config)).connect(name, tcp)).await.expect("the handshake ends in time")?;
        tls.write_all(request(forged).as_bytes()).await?;
        let mut raw = Vec::new();
        tokio::time::timeout(PATIENCE, tls.read_to_end(&mut raw)).await.expect("the read ends in time")?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }
}

fn request(forged: &str) -> String {
    let mut request = String::from("GET /whoami HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n");
    for name in FORGED {
        let _ = write!(request, "{name}: {forged}\r\n");
    }
    request + "\r\n"
}

#[tokio::test(flavor = "multi_thread")]
async fn the_guard_reads_the_certificate_the_tls_session_verified() {
    let ca = Ca::new();
    let server = start(Some(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca))).await;
    let client = ca.issue(&["client"]);

    let reply = server.secure(&ca, Some(&client), "").await.expect("the certified client is served");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with(&hex(client.cert_der())), "the leaf the session verified: {reply}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_header_naming_a_certificate_is_never_read_as_one() {
    let ca = Ca::new();
    let client = ca.issue(&["client"]);
    let forged = client.cert_pem().replace('\n', "");

    let secure = start(Some(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca))).await;
    let reply = secure.secure(&ca, None, &forged).await.expect("a client with no certificate is admitted to TLS");
    assert!(reply.starts_with("HTTP/1.1 403"), "a TLS client presenting no certificate: {reply}");
    secure.stop().await;

    let plain = start(None).await;
    let reply = plain.plain(&forged).await;
    assert!(reply.starts_with("HTTP/1.1 403"), "a plain HTTP client: {reply}");
    plain.stop().await;
}

/// The chain belongs to the connection that presented it: a client presenting none, served after
/// a certified client on the same server, is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_stays_with_the_connection_that_presented_it() {
    let ca = Ca::new();
    let server = start(Some(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca))).await;
    let client = ca.issue(&["client"]);

    let reply = server.secure(&ca, Some(&client), "").await.expect("the certified client is served");
    assert!(reply.starts_with("HTTP/1.1 200"), "the certified client: {reply}");
    let reply = server.secure(&ca, None, "").await.expect("a client with no certificate is admitted to TLS");
    assert!(reply.starts_with("HTTP/1.1 403"), "a client presenting no certificate, after a certified one: {reply}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_the_verifier_refuses_never_reaches_the_guard() {
    let ca = Ca::new();
    let server = start(Some(server_config(&ca.issue(&["localhost", "127.0.0.1"]), &ca))).await;
    let stranger = Ca::new().issue(&["client"]);

    let refused = server.secure(&ca, Some(&stranger), "").await;
    assert!(!matches!(&refused, Ok(reply) if reply.contains("200")), "a certificate from another authority got {refused:?}");
    server.stop().await;
}
