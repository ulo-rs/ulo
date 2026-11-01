//! `Tls::from_config` on the TCP link, which needs no ALPN protocol: the configuration serves as
//! given, its protocols untouched, and its client-certificate verifier refuses a client with no
//! certificate or one another authority issued.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_net::Tls;
use ulo_net::rustls::pki_types::pem::PemObject;
use ulo_net::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use ulo_net::rustls::server::WebPkiClientVerifier;
use ulo_net::rustls::{self, ClientConfig, ServerConfig};
use ulo_rpc::Payload;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_test_certs::{Ca, Certified};

#[injectable]
struct Echo;

#[routes]
impl Echo {
    #[ulo_rpc::message("echo")]
    async fn echo(&self, n: Payload<u32>) -> u32 {
        n.0
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Echo>();
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn identity(cert: &Certified) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let chain = CertificateDer::pem_slice_iter(cert.cert_pem().as_bytes()).collect::<Result<Vec<_>, _>>().unwrap();
    (chain, PrivateKeyDer::from_pem_slice(cert.key_pem().as_bytes()).unwrap())
}

fn server_config(server: &Certified, ca: &Ca, alpn: &[&[u8]]) -> Arc<ServerConfig> {
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(ca.roots()), provider()).build().unwrap();
    let (chain, key) = identity(server);
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .unwrap();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(config)
}

/// The handshake's error, or the protocol ALPN settled on and what a read within 300 ms saw: a
/// verified client's connection is held open waiting for a frame.
async fn dial(addr: std::net::SocketAddr, ca: &Ca, client: Option<&Certified>, alpn: &[&[u8]]) -> Result<(Option<Vec<u8>>, String), String> {
    let builder = ClientConfig::builder_with_provider(provider()).with_safe_default_protocol_versions().unwrap().with_root_certificates(ca.roots());
    let mut config = match client {
        Some(c) => {
            let (chain, key) = identity(c);
            builder.with_client_auth_cert(chain, key).unwrap()
        }
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let mut tls = TlsConnector::from(Arc::new(config)).connect(ServerName::try_from("localhost").unwrap(), tcp).await.map_err(|e| e.to_string())?;
    let negotiated = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
    let _ = tls.write_all(b"").await;
    let mut buf = [0u8; 64];
    let read = match tokio::time::timeout(Duration::from_millis(300), tls.read(&mut buf)).await {
        Err(_) => "held open".to_owned(),
        Ok(Ok(0)) => "eof".to_owned(),
        Ok(Ok(n)) => format!("{n} bytes"),
        Ok(Err(e)) => format!("read error: {e}"),
    };
    Ok((negotiated, read))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handed_configuration_serves_as_given_and_verifies_client_certificates() {
    let ca = Ca::new();
    let server = ca.issue(&["localhost", "127.0.0.1"]);
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .unwrap()
        .connect()
        .await
        .unwrap()
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0").tls(Tls::from_config(server_config(&server, &ca, &[b"x-ulo"])))))
        .listen()
        .await
        .expect("listens");
    let [bound] = app.addresses().try_into().expect("one");
    assert!(bound.tls);
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending()));

    let client = ca.issue(&["client"]);
    let certified = dial(bound.addr, &ca, Some(&client), &[b"x-ulo"]).await;
    assert_eq!(certified.as_ref().expect("the certified client completes its handshake").0.as_deref(), Some(b"x-ulo".as_slice()), "the link kept the given ALPN");
    assert_eq!(certified.as_ref().unwrap().1, "held open");

    let anonymous = dial(bound.addr, &ca, None, &[b"x-ulo"]).await;
    assert!(anonymous.is_err() || anonymous.as_ref().unwrap().1.starts_with("read error"), "{anonymous:?}");

    let stranger = Ca::new().issue(&["client"]);
    let forged = dial(bound.addr, &ca, Some(&stranger), &[b"x-ulo"]).await;
    assert!(forged.is_err() || forged.as_ref().unwrap().1.starts_with("read error"), "{forged:?}");

    let h2_only = dial(bound.addr, &ca, Some(&client), &[b"h2"]).await;
    assert!(h2_only.is_err(), "the link added no protocol: {h2_only:?}");

    let _ = handle.close(Signal::new("tls-config")).await;
    let _ = serving.await;
}
