//! A bare HTTPS client: one request over one TLS connection, the reply read to the end.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use ulo_test_certs::Certified;

fn connector(roots: RootCertStore, alpn: &[&[u8]]) -> TlsConnector {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    TlsConnector::from(Arc::new(config))
}

/// The status line and body `GET path` answers over TLS, trusting `trusted`, and the protocol
/// ALPN settled on.
pub async fn https_get(addr: SocketAddr, trusted: &Certified, path: &str) -> std::io::Result<(String, Option<Vec<u8>>)> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from("localhost").expect("a DNS name");
    let mut tls = connector(trusted.roots(), &[b"http/1.1"]).connect(name, tcp).await?;
    let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
    tls.write_all(format!("GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").as_bytes()).await?;
    let mut raw = Vec::new();
    let _ = tls.read_to_end(&mut raw).await;
    Ok((String::from_utf8_lossy(&raw).into_owned(), alpn))
}

/// Writes `cert` and `key` to `cert.pem` and `key.pem` in a directory of their own under the OS temp
/// directory, for `ex_20_http_tls::server` to read; `tag` keeps tests running in parallel apart.
pub fn pem_files(tag: &str, cert: &str, key: &str) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("ex-20-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the certificate directory");
    let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert_path, cert).expect("write the certificate");
    std::fs::write(&key_path, key).expect("write the key");
    (cert_path.display().to_string(), key_path.display().to_string())
}

/// Opens a TLS connection offering `alpn` and returns the protocol the server chose.
pub async fn negotiated(addr: SocketAddr, trusted: &Certified, alpn: &[&[u8]]) -> Option<Vec<u8>> {
    let tcp = TcpStream::connect(addr).await.expect("connect");
    let name = ServerName::try_from("localhost").expect("a DNS name");
    let tls = connector(trusted.roots(), alpn).connect(name, tcp).await.expect("the handshake succeeds");
    tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec)
}
