mod common;

use common::{https_get, negotiated, pem_files};
use ex_20_http_tls::{builder, server};
use ex_support::{Served, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use ulo::StartupError;
use ulo_runtime_tokio::Tokio;
use ulo_test_certs::{Certified, localhost};

async fn app(tag: &str) -> (Served, bool) {
    let (cert, key) = pem_files(tag, localhost().cert_pem(), localhost().key_pem());
    let bound = builder().runtime(Tokio::current()).wire().unwrap().connect().await.unwrap().bind(server("127.0.0.1:0", &cert, &key)).listen().await.unwrap();
    let tls = bound.addresses()[0].tls;
    (serve(bound), tls)
}

#[tokio::test]
async fn a_client_that_trusts_the_certificate_gets_the_answer_over_tls() {
    let (server, reported_tls) = app("trusted").await;
    assert!(reported_tls, "the bound address says it speaks TLS");

    let (reply, alpn) = https_get(server.addr, localhost(), "/").await.expect("the handshake succeeds");
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("Hello over TLS"), "{reply}");
    assert_eq!(alpn.as_deref(), Some(b"http/1.1".as_slice()));
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn alpn_settles_on_h2_when_the_client_prefers_it_and_on_http_1_1_otherwise() {
    let (server, _) = app("alpn").await;

    assert_eq!(negotiated(server.addr, localhost(), &[b"h2", b"http/1.1"]).await.as_deref(), Some(b"h2".as_slice()));
    assert_eq!(negotiated(server.addr, localhost(), &[b"http/1.1"]).await.as_deref(), Some(b"http/1.1".as_slice()));
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn plaintext_http_to_the_tls_port_is_never_answered_with_the_page() {
    let (server, _) = app("plaintext").await;

    let mut plain = TcpStream::connect(server.addr).await.unwrap();
    plain.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").await.unwrap();
    let mut raw = Vec::new();
    let _ = plain.read_to_end(&mut raw).await;
    let text = String::from_utf8_lossy(&raw);
    assert!(!text.contains("HTTP/1.1 200") && !text.contains("Hello over TLS"), "{text}");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_client_that_does_not_trust_the_certificate_fails_the_handshake() {
    let (server, _) = app("untrusted").await;
    let stranger = Certified::self_signed(&["localhost"]);

    let refused = https_get(server.addr, &stranger, "/").await;
    assert!(refused.is_err(), "the handshake against an untrusted certificate must fail, got {refused:?}");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_certificate_file_that_holds_no_certificate_fails_listen_naming_the_file() {
    let (cert, key) = pem_files("bad", "not a certificate", localhost().key_pem());
    let bound = builder()
        .runtime(Tokio::current())
        .wire()
        .unwrap()
        .connect()
        .await
        .unwrap()
        .bind(server("127.0.0.1:0", &cert, &key))
        .listen()
        .await;
    let Err(error) = bound else { panic!("listen accepted a bad certificate") };
    assert!(matches!(error, StartupError::Configure(_)), "{error:?}");
    let text = error.to_string();
    assert!(text.contains(&format!("bad TLS certificate: `{cert}`")), "{text}");
}
