//! A TLS connect to MySQL 8 through `ulo-db-sqlx`, against a container serving a certificate a
//! test CA issued (`support/server.rs`, `mysql_tls`). The crate's `tls` feature is rustls over ring
//! with the platform's roots; the CA is supplied through the URL's `ssl-ca`, so the chain is
//! verified through a root the test chose, and the name through `verify_identity`. The server
//! requires secure transport, so a connect that succeeds was encrypted. MySQL 8's
//! `caching_sha2_password` then authenticates over TLS with no RSA key exchange, the crate
//! enabling no `mysql-rsa`.

#![cfg(all(feature = "integration", feature = "mysql", feature = "tls"))]

use std::time::{Duration, Instant};

use serde_json::json;
use ulo::{App, Module, ModuleDef, ModuleIdentity, is_permanent};
use ulo_db::{Store, StoreModule};
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_sqlx::sqlx::{self, Connection};
use ulo_db_sqlx::{MySql, MySqlOptions};
use ulo_test_certs::Ca;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

fn options(port: u16, mode: &str, ca: Option<&std::path::Path>) -> MySqlOptions {
    let mut url = format!("mysql://root:{}@127.0.0.1:{port}/app?ssl-mode={mode}", server::REAL_PASSWORD);
    if let Some(ca) = ca {
        url.push_str(&format!("&ssl-ca={}", ca.display()));
    }
    server::options(json!({ "url": url, "max_connections": 2, "acquire_timeout_secs": 3 }))
}

/// Writes `ca`'s certificate where the URL can name it, in a directory removed on drop.
fn root_file(ca: &Ca) -> (server::Scratch, std::path::PathBuf) {
    let dir = server::Scratch::new("tls-root");
    let path = dir.0.join("ca.pem");
    std::fs::write(&path, ca.cert_pem()).expect("the root file is written");
    (dir, path)
}

/// Pings `options` through the store, answering the cipher the session negotiated.
async fn connect(options: &MySqlOptions) -> Result<String, String> {
    let pool = <MySql as Store>::create(options).await.map_err(|e| e.to_string())?;
    <MySql as Store>::ping(&pool).await.map_err(|e| e.to_string())?;
    let (_, cipher): (String, String) =
        sqlx::query_as("show session status like 'Ssl_cipher'").fetch_one(&pool).await.map_err(|e| e.to_string())?;
    Ok(cipher)
}

/// The store's connect until the server answers: the image restarts its server once after
/// initialising, and the first moments refuse connections.
async fn connect_when_up(options: &MySqlOptions) -> String {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match connect(options).await {
            Ok(cipher) => return cipher,
            Err(e) if std::time::Instant::now() >= until => {
                panic!("a connect that verifies the server through its own CA failed: {e}")
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }
}

/// A single connection, without the pool that turns every failure into a timeout.
async fn direct(options: &MySqlOptions) -> Result<(), sqlx::Error> {
    sqlx::MySqlConnection::connect(options.url.expose()).await.map(|_| ())
}

/// A certificate no supplied root issued is refused at the handshake, and a start over it fails at
/// its first attempt as `Permanent` rather than retrying to `connect_within`. Planted breaks: the
/// wrong-root case run with `ssl-mode=required`, which does not verify, and connects; the same case
/// with `ssl-mode=disabled`; a classifier that reads the refused certificate as retryable.
#[test]
fn a_tls_connect_verifies_the_server_through_the_supplied_root() {
    <OnTokio as Harness>::block_on(async {
        let ca = Ca::new();
        let server = server::Server::start_with(&server::MYSQL, &server::mysql_tls(&ca)).await;
        let port = server.relay.addr().port();
        let (_dir, root) = root_file(&ca);

        let cipher = connect_when_up(&options(port, "verify_identity", Some(&root))).await;
        assert!(!cipher.is_empty(), "the session negotiated no cipher: it was not encrypted");

        let (_other_dir, wrong_root) = root_file(&Ca::new());
        match direct(&options(port, "verify_identity", Some(&wrong_root))).await {
            // rustls reports a chain it cannot verify as `InvalidCertificate`, which SQLx surfaces
            // as an I/O error of kind `InvalidData`.
            Err(sqlx::Error::Tls(_)) => {}
            Err(sqlx::Error::Io(e)) if format!("{e:?}").contains("InvalidCertificate") => {}
            other => panic!("verifying through a root that did not issue the certificate was not a TLS failure: {other:?}"),
        }

        let (error, took) = start(options(port, "verify_identity", Some(&wrong_root))).await;
        assert!(is_permanent(&error), "a certificate no supplied root issued is retryable, failing after {took:?}: {error}");
        assert!(took < Duration::from_secs(5), "a certificate no supplied root issued took {took:?} to refuse the start");

        if direct(&options(port, "disabled", None)).await.is_ok() {
            panic!("a connect without TLS succeeded against a server that requires it");
        }
    });
}

struct Root(MySqlOptions);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0.url.expose().clone())
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(StoreModule::<MySql>::from_options(self.0.clone()).connect_within(Duration::from_secs(15)));
    }
}

/// The start of an app whose only module is the store over `options`, refused, and how long it took.
async fn start(options: MySqlOptions) -> (ulo::StartupError, Duration) {
    let app = App::builder(Root(options)).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
    let began = Instant::now();
    match app.connect().await {
        Err(error) => (error, began.elapsed()),
        Ok(_) => panic!("a start verifying the server through a root that did not issue its certificate connected"),
    }
}
