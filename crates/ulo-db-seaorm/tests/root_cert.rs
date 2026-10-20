//! A root certificate the URL names is read in `create`, as `ulo-db-sqlx` reads it: a path that
//! cannot be read and a file that is not PEM are `Permanent` there, with no server involved, and
//! only under the ssl modes that verify the server. Without it SQLx's bare `Io` of kind `NotFound`
//! is retried until `connect_within` runs out.
//! Planted break: the check left out of `create`.
#![cfg(all(feature = "tls", any(feature = "postgres", feature = "mysql")))]

use serde_json::json;
use ulo::Permanent;
use ulo_db::Store;
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_seaorm::{SeaOrm, SeaOrmOptions};
use ulo_test_certs::Ca;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

/// A URL naming a root certificate at `path`, in the spelling of each engine, under `mode`: the
/// verifying one and one that does not read the file.
struct Engine {
    verifying: fn(&str) -> String,
    plain: fn(&str) -> Vec<String>,
}

fn engines() -> Vec<(&'static str, Engine)> {
    let mut all = Vec::new();
    #[cfg(feature = "postgres")]
    all.push((
        "postgres",
        Engine {
            verifying: |path| {
                format!("postgres://u:pw@127.0.0.1:1/app?sslmode=verify-ca&sslrootcert={path}")
            },
            plain: |path| {
                ["require", "prefer", "disable"]
                    .map(|mode| format!("postgres://u:pw@127.0.0.1:1/app?sslmode={mode}&sslrootcert={path}"))
                    .to_vec()
            },
        },
    ));
    #[cfg(feature = "mysql")]
    all.push((
        "mysql",
        Engine {
            verifying: |path| format!("mysql://u:pw@127.0.0.1:1/app?ssl-mode=verify_ca&ssl-ca={path}"),
            plain: |path| {
                ["required", "preferred", "disabled"]
                    .map(|mode| format!("mysql://u:pw@127.0.0.1:1/app?ssl-mode={mode}&ssl-ca={path}"))
                    .to_vec()
            },
        },
    ));
    all
}

async fn created(url: &str) -> Result<(), ulo::BoxError> {
    let options: SeaOrmOptions = server::options(json!({ "url": url }));
    <SeaOrm as Store>::create(&options).await.map(drop)
}

#[test]
fn a_missing_root_certificate_is_permanent_in_create() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("sea-root-missing");
        let missing = dir.0.join("nope.pem");
        for (name, engine) in engines() {
            let error = created(&(engine.verifying)(&missing.display().to_string()))
                .await
                .expect_err("a root certificate that does not exist was accepted");
            assert!(error.is::<Permanent>(), "{name}: a missing root certificate is retryable: {error}");
        }
    });
}

#[test]
fn a_root_certificate_that_is_not_pem_is_permanent_in_create() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("sea-root-garbage");
        let path = dir.0.join("ca.pem");
        std::fs::write(&path, "-----BEGIN CERTIFICATE-----\n%%%not base64%%%\n-----END CERTIFICATE-----\n").unwrap();
        for (name, engine) in engines() {
            let error = created(&(engine.verifying)(&path.display().to_string()))
                .await
                .expect_err("a root certificate that is not PEM was accepted");
            assert!(error.is::<Permanent>(), "{name}: a malformed root certificate is retryable: {error}");
        }
    });
}

/// Planted break: the read made in every mode.
#[test]
fn a_root_certificate_outside_the_verifying_modes_is_not_read() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("sea-root-unread");
        let missing = dir.0.join("nope.pem");
        for (name, engine) in engines() {
            for url in (engine.plain)(&missing.display().to_string()) {
                created(&url).await.unwrap_or_else(|error| panic!("{name}: an unread root certificate was refused: {error}"));
            }
        }
    });
}

#[test]
fn a_readable_root_certificate_creates() {
    <OnTokio as Harness>::block_on(async {
        let dir = server::Scratch::new("sea-root-valid");
        let path = dir.0.join("ca.pem");
        std::fs::write(&path, Ca::new().cert_pem()).unwrap();
        for (name, engine) in engines() {
            created(&(engine.verifying)(&path.display().to_string()))
                .await
                .unwrap_or_else(|error| panic!("{name}: a valid root was refused: {error}"));
        }
    });
}
