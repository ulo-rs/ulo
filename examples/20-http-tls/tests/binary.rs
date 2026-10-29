mod common;

use std::net::SocketAddr;

use common::https_get;
use ex_support::Bin;
use ulo_test_certs::localhost;

#[tokio::test]
async fn the_binary_serves_https_with_the_files_it_is_given_and_exits_clean_on_sigterm() {
    let dir = std::env::temp_dir().join(format!("ex-20-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, localhost().cert_pem()).unwrap();
    std::fs::write(&key, localhost().key_pem()).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();

    let mut bin = Bin::spawn(
        env!("CARGO_BIN_EXE_ex-20-http-tls"),
        &[("PORT", &port), ("TLS_CERT", cert.to_str().unwrap()), ("TLS_KEY", key.to_str().unwrap())],
    );
    let line = bin.next_line();
    let addr: SocketAddr = line.strip_prefix("listening on https://").unwrap_or_else(|| panic!("{line}")).parse().unwrap();
    assert_eq!(addr.port().to_string(), port);

    let (reply, _) = https_get(addr, localhost(), "/").await.unwrap();
    assert!(reply.ends_with("Hello over TLS"), "{reply}");
    let exit = bin.terminate();
    std::fs::remove_dir_all(&dir).ok();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}

#[test]
fn without_the_certificate_files_it_says_which_variable_is_missing_and_exits_with_an_error() {
    let bin = Bin::spawn(env!("CARGO_BIN_EXE_ex-20-http-tls"), &[]);
    let exit = bin.finish();
    assert!(!exit.status.success());
    assert!(exit.stderr.contains("TLS_CERT"), "{}", exit.stderr);
}
