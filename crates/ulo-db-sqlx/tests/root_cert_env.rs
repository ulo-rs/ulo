//! A root certificate named only by `PGSSLROOTCERT`, which SQLx's option parser reads itself and
//! this crate's `create` does not, fails the first ping as `Permanent` when the file is missing or
//! unreadable, and stays retryable over a Unix socket. The server here is a stub that answers
//! Postgres's `SSLRequest` with `S`, which is all the connect needs before it reads the root
//! certificate: no Postgres runs.
//!
//! The first connect loads the platform's roots before it reads the file, which takes seconds on
//! some hosts, so the pool's acquire timeout is long.
//!
//! The process environment is set only on a child process (the test binary re-run on `child`), so
//! no test mutates its own. Planted breaks: the classifier left bare for `Io`; the classifier
//! permanent over a socket too; the store reporting every pool as a socket or as a host and port;
//! the reach read from the socket option alone, which misses a socket directory `PGHOST` names; the
//! migration lock's reach fixed to either answer.
#![cfg(all(feature = "postgres", feature = "tls"))]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use ulo::Permanent;
use ulo_db::{MigrationLock, Store};
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_sqlx::{PgOptions, Postgres};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;
#[path = "../../ulo-db-conformance/support/stub.rs"]
mod stub;

const SCENARIO: &str = "ULO_ROOT_CERT_SCENARIO";

/// The scenario the parent chose through the environment it started this process with: the
/// outcome of the ping, or of the migration lock's acquire for a `-lock` scenario, as one word on
/// stdout.
#[test]
fn child() {
    let Ok(scenario) = std::env::var(SCENARIO) else { return };
    let (reach, lock) = match scenario.strip_suffix("-lock") {
        Some(reach) => (reach, true),
        None => (scenario.as_str(), false),
    };
    let url = match reach {
        "tcp" => format!("postgres://u:p@127.0.0.1:{}/app?sslmode=verify-ca", stub::postgres_ssl()),
        // The socket path is a directory that holds no socket: the connect fails with
        // `NotFound`, as a socket not yet created does.
        "socket" => format!("postgres://u:p@localhost/app?sslmode=verify-ca&host={}", std::env::temp_dir().display()),
        // No host in the URL: SQLx takes `PGHOST`, which the parent set to such a directory.
        "pghost" => "postgres:///app?sslmode=verify-ca".to_owned(),
        other => panic!("unknown scenario {other}"),
    };
    let options: PgOptions = server::options(json!({ "url": url, "acquire_timeout_secs": 20, "max_connections": 2 }));
    let outcome = <OnTokio as Harness>::block_on(async move {
        let pool = <Postgres as Store>::create(&options).await.expect("create reads no environment and refuses nothing");
        if lock {
            <Postgres as MigrationLock>::lock(&pool, Duration::from_secs(20)).await.map(drop)
        } else {
            <Postgres as Store>::ping(&pool).await
        }
    });
    // The class, then the kind of the I/O error the pool returned, so a timeout is told apart from
    // the file's `NotFound` and a wrapped error from a bare one.
    let word = match outcome {
        Ok(()) => "CONNECTED".to_owned(),
        Err(error) => {
            let (class, inner): (_, &(dyn std::error::Error + 'static)) = match error.downcast_ref::<Permanent>() {
                Some(permanent) => ("PERMANENT", std::error::Error::source(permanent).expect("a permanent error carries its cause")),
                None => ("RETRYABLE", &*error),
            };
            match inner.downcast_ref::<ulo_db_sqlx::sqlx::Error>() {
                Some(ulo_db_sqlx::sqlx::Error::Io(io)) => format!("{class}:{:?}", io.kind()),
                other => format!("{class}:{other:?}"),
            }
        }
    };
    println!("\nOUTCOME:{word}");
}

fn outcome(scenario: &str, root: &Path) -> String {
    outcome_with(scenario, root, &[])
}

fn outcome_with(scenario: &str, root: &Path, env: &[(&str, &Path)]) -> String {
    let mut cmd = Command::new(std::env::current_exe().expect("this binary"));
    cmd.args(["--exact", "child", "--nocapture", "--test-threads=1"]);
    for var in ["PGHOST", "PGPORT", "PGUSER", "PGPASSWORD", "PGDATABASE", "PGSSLMODE", "PGSSLROOTCERT", "PGSSLCERT", "PGSSLKEY"] {
        cmd.env_remove(var);
    }
    cmd.env(SCENARIO, scenario).env("PGSSLROOTCERT", root);
    for (var, value) in env {
        cmd.env(var, value);
    }
    let out = cmd.output().expect("the child ran");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "child failed: {stdout}{}", String::from_utf8_lossy(&out.stderr));
    stdout
        .lines()
        .find_map(|line| line.split_once("OUTCOME:").map(|(_, word)| word))
        .unwrap_or_else(|| panic!("the child printed no outcome: {stdout}"))
        .to_owned()
}

#[test]
fn a_root_certificate_missing_from_pgsslrootcert_is_permanent_over_tcp() {
    let dir = server::Scratch::new("env-root-missing");
    assert_eq!(outcome("tcp", &dir.0.join("nope.pem")), "PERMANENT:NotFound");
}

/// A file the process may not read: mode 000. Skipped where the process is root, which reads it.
#[cfg(unix)]
#[test]
fn a_root_certificate_unreadable_from_pgsslrootcert_is_permanent_over_tcp() {
    use std::os::unix::fs::PermissionsExt;

    let dir = server::Scratch::new("env-root-unreadable");
    let path = dir.0.join("locked.pem");
    std::fs::write(&path, "x").expect("a file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read(&path).is_ok() {
        return;
    }
    assert_eq!(outcome("tcp", &path), "PERMANENT:PermissionDenied");
}

/// Over a Unix socket the same `NotFound` is a socket not yet created, which a restart mends.
#[cfg(unix)]
#[test]
fn the_same_error_over_a_socket_stays_retryable() {
    let dir = server::Scratch::new("env-root-socket");
    assert_eq!(outcome("socket", &dir.0.join("nope.pem")), "RETRYABLE:NotFound");
}

/// The migration lock reads the reach its own way, so it is checked apart from the ping.
#[test]
fn the_migration_lock_reads_a_missing_root_certificate_as_permanent_over_tcp() {
    let dir = server::Scratch::new("env-root-lock");
    assert_eq!(outcome("tcp-lock", &dir.0.join("nope.pem")), "PERMANENT:NotFound");
}

#[cfg(unix)]
#[test]
fn the_migration_lock_keeps_the_error_retryable_over_a_socket() {
    let dir = server::Scratch::new("env-root-lock-socket");
    assert_eq!(outcome("socket-lock", &dir.0.join("nope.pem")), "RETRYABLE:NotFound");
}

/// A URL with no host takes `PGHOST`, and a `PGHOST` starting with `/` is a socket directory: SQLx
/// connects through `<dir>/.s.PGSQL.5432` though the options name no socket. The `NotFound` is the
/// socket's, not the certificate's.
#[cfg(unix)]
#[test]
fn a_socket_directory_named_by_pghost_keeps_the_error_retryable() {
    let dir = server::Scratch::new("env-root-pghost");
    let sockets = server::Scratch::new("env-root-pghost-dir");
    assert_eq!(outcome_with("pghost", &dir.0.join("nope.pem"), &[("PGHOST", &sockets.0)]), "RETRYABLE:NotFound");
    assert_eq!(outcome_with("pghost-lock", &dir.0.join("nope.pem"), &[("PGHOST", &sockets.0)]), "RETRYABLE:NotFound");
}
