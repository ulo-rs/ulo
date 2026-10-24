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
//! permanent over a socket too; the store reporting every pool as a socket or as a host and port.
#![cfg(all(feature = "postgres", feature = "tls"))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;

use serde_json::json;
use ulo::Permanent;
use ulo_db::Store;
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_sqlx::{PgOptions, Postgres};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

const SCENARIO: &str = "ULO_ROOT_CERT_SCENARIO";

/// Answers each connection's `SSLRequest` with `S` and holds the connection open.
fn stub() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    let port = listener.local_addr().expect("an address").port();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 8];
            if stream.read_exact(&mut request).is_ok() {
                let _ = stream.write_all(b"S");
            }
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
            let mut rest = [0u8; 64];
            let _ = stream.read(&mut rest);
        }
    });
    (port, handle)
}

/// The scenario the parent chose through the environment it started this process with: the ping's
/// outcome as one word on stdout.
#[test]
fn child() {
    let Ok(scenario) = std::env::var(SCENARIO) else { return };
    let (url, stub) = match scenario.as_str() {
        "tcp" => {
            let (port, stub) = stub();
            (format!("postgres://u:p@127.0.0.1:{port}/app?sslmode=verify-ca"), Some(stub))
        }
        // The socket path is a directory that holds no socket: the connect fails with
        // `NotFound`, as a socket not yet created does.
        "socket" => (format!("postgres://u:p@localhost/app?sslmode=verify-ca&host={}", std::env::temp_dir().display()), None),
        other => panic!("unknown scenario {other}"),
    };
    let options: PgOptions = server::options(json!({ "url": url, "acquire_timeout_secs": 20 }));
    let outcome = <OnTokio as Harness>::block_on(async move {
        let pool = <Postgres as Store>::create(&options).await.expect("create reads no environment and refuses nothing");
        <Postgres as Store>::ping(&pool).await
    });
    let word = match outcome {
        Ok(()) => "CONNECTED".to_owned(),
        Err(error) if error.is::<Permanent>() => "PERMANENT".to_owned(),
        // The kind of the error the pool returned, so a retryable answer that is a timeout
        // instead of the file's `NotFound` is told apart.
        Err(error) => match error.downcast_ref::<ulo_db_sqlx::sqlx::Error>() {
            Some(ulo_db_sqlx::sqlx::Error::Io(io)) => format!("RETRYABLE:{:?}", io.kind()),
            other => format!("RETRYABLE:{other:?}"),
        },
    };
    println!("\nOUTCOME:{word}");
    if let Some(stub) = stub {
        drop(stub);
    }
}

fn outcome(scenario: &str, root: &Path) -> String {
    let mut cmd = Command::new(std::env::current_exe().expect("this binary"));
    cmd.args(["--exact", "child", "--nocapture", "--test-threads=1"]);
    for var in ["PGHOST", "PGPORT", "PGUSER", "PGPASSWORD", "PGDATABASE", "PGSSLMODE", "PGSSLROOTCERT", "PGSSLCERT", "PGSSLKEY"] {
        cmd.env_remove(var);
    }
    cmd.env(SCENARIO, scenario).env("PGSSLROOTCERT", root);
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
    assert_eq!(outcome("tcp", &dir.0.join("nope.pem")), "PERMANENT");
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
    assert_eq!(outcome("tcp", &path), "PERMANENT");
}

/// Over a Unix socket the same `NotFound` is a socket not yet created, which a restart mends.
#[cfg(unix)]
#[test]
fn the_same_error_over_a_socket_stays_retryable() {
    let dir = server::Scratch::new("env-root-socket");
    assert_eq!(outcome("socket", &dir.0.join("nope.pem")), "RETRYABLE:NotFound");
}
