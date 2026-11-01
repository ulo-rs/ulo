//! A MySQL pool reaches its server by host and port unless its URL names a `socket`, and over a host
//! and port a file that cannot be found or read is a certificate file: `Permanent` at the first ping
//! and at the migration lock's acquire. Over a socket the same `NotFound` is a socket not yet
//! created and stays retryable. The file here is a client certificate the URL names under
//! `ssl-mode=required`, which SQLx's TLS setup reads and `create` does not; the server is a stub that
//! offers TLS and goes no further, so no MySQL runs. Planted breaks: the reach fixed to either
//! answer, for the ping and for the lock.
#![cfg(all(feature = "mysql", feature = "tls"))]

use std::time::Duration;

use serde_json::json;
use ulo::{BoxError, Permanent};
use ulo_db::{MigrationLock, Store};
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_sqlx::{MySql, MySqlOptions};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;
#[path = "../../ulo-db-conformance/support/stub.rs"]
mod stub;

/// The class, then the kind of the I/O error underneath.
fn word(outcome: Result<(), BoxError>) -> String {
    let Err(error) = outcome else { return "CONNECTED".to_owned() };
    let (class, inner): (_, &(dyn std::error::Error + 'static)) = match error.downcast_ref::<Permanent>() {
        Some(permanent) => ("PERMANENT", std::error::Error::source(permanent).expect("a permanent error carries its cause")),
        None => ("RETRYABLE", &*error),
    };
    match inner.downcast_ref::<ulo_db_sqlx::sqlx::Error>() {
        Some(ulo_db_sqlx::sqlx::Error::Io(io)) => format!("{class}:{:?}", io.kind()),
        other => format!("{class}:{other:?}"),
    }
}

/// The ping's word and the lock's, each on a pool of its own.
fn outcomes(url: &str) -> (String, String) {
    <OnTokio as Harness>::block_on(async {
        let options: MySqlOptions = server::options(json!({ "url": url, "acquire_timeout_secs": 20, "max_connections": 2 }));
        let pool = <MySql as Store>::create(&options).await.expect("create reads no client certificate");
        let pinged = word(<MySql as Store>::ping(&pool).await);
        let pool = <MySql as Store>::create(&options).await.expect("create reads no client certificate");
        let locked = word(<MySql as MigrationLock>::lock(&pool, Duration::from_secs(20)).await.map(drop));
        (pinged, locked)
    })
}

#[test]
fn a_client_certificate_missing_over_tcp_is_permanent() {
    let dir = server::Scratch::new("mysql-reach-tcp");
    let missing = dir.0.join("nope.pem").display().to_string();
    let url = format!("mysql://u:p@127.0.0.1:{}/app?ssl-mode=required&ssl-cert={missing}&ssl-key={missing}", stub::mysql_tls());
    assert_eq!(outcomes(&url), ("PERMANENT:NotFound".to_owned(), "PERMANENT:NotFound".to_owned()));
}

#[cfg(unix)]
#[test]
fn the_same_error_over_a_socket_stays_retryable() {
    let dir = server::Scratch::new("mysql-reach-socket");
    let missing = dir.0.join("nope.pem").display().to_string();
    let socket = dir.0.join("mysqld.sock").display().to_string();
    let url = format!("mysql://u:p@localhost/app?socket={socket}&ssl-mode=required&ssl-cert={missing}&ssl-key={missing}");
    assert_eq!(outcomes(&url), ("RETRYABLE:NotFound".to_owned(), "RETRYABLE:NotFound".to_owned()));
}
