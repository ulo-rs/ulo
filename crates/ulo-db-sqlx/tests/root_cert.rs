//! A root certificate the URL names is read in `create`: a path that cannot be read and a file that
//! is not PEM are `Permanent` there, with no server involved. SQLx reads the file at the first
//! handshake and reports a missing path as a bare `Io` of kind `NotFound`, which a Unix socket not
//! yet created also gives; only `create` knows the path is the root certificate's.
//! Planted break: the read left out of `create`.
#![cfg(all(feature = "mysql", feature = "tls"))]

use serde_json::json;
use ulo::Permanent;
use ulo_db::Store;
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_sqlx::{MySql, MySqlOptions};
use ulo_test_certs::Ca;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

fn options(query: &str) -> MySqlOptions {
    server::options(json!({ "url": format!("mysql://root:pw@127.0.0.1:1/app?{query}") }))
}

async fn created(options: &MySqlOptions) -> Result<(), ulo::BoxError> {
    <MySql as Store>::create(options).await.map(drop)
}

fn permanent(error: &ulo::BoxError) -> bool {
    error.is::<Permanent>()
}

#[test]
fn a_missing_root_certificate_is_permanent_in_create() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("root-missing");
        let missing = dir.0.join("nope.pem");
        let error = created(&options(&format!("ssl-mode=verify_ca&ssl-ca={}", missing.display())))
            .await
            .expect_err("a root certificate that does not exist was accepted");
        assert!(permanent(&error), "a missing root certificate is retryable: {error}");
    });
}

#[test]
fn a_root_certificate_that_is_not_pem_is_permanent_in_create() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("root-garbage");
        let path = dir.0.join("ca.pem");
        std::fs::write(&path, "-----BEGIN CERTIFICATE-----\n%%%not base64%%%\n-----END CERTIFICATE-----\n").unwrap();
        let error = created(&options(&format!("ssl-mode=verify_ca&ssl-ca={}", path.display())))
            .await
            .expect_err("a root certificate that is not PEM was accepted");
        assert!(permanent(&error), "a malformed root certificate is retryable: {error}");
    });
}

#[test]
fn a_readable_root_certificate_and_no_root_certificate_both_create() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("root-valid");
        let path = dir.0.join("ca.pem");
        std::fs::write(&path, Ca::new().cert_pem()).unwrap();
        created(&options(&format!("ssl-mode=verify_ca&ssl-ca={}", path.display()))).await.expect("a valid root was refused");
        created(&options("ssl-mode=preferred")).await.expect("a URL naming no root was refused");
    });
}
