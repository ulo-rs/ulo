//! SeaORM reads the reach of its SQLx pool as `ulo-db-sqlx`'s stores do, so a certificate file that
//! cannot be found or read fails the first ping as `Permanent` over a host and port and stays
//! retryable over a Unix socket, whichever source named the file. Postgres takes the root
//! certificate from `PGSSLROOTCERT`, which SQLx's option parser reads and `create` does not, set only
//! on a child process (the test binary re-run on `child`); MySQL takes a client certificate from
//! the URL under `ssl-mode=required`, which `create` does not read either. The servers are stubs
//! that go no further than the TLS opening. Planted breaks: the reach fixed to either answer for
//! each engine; the Postgres reach read from the socket option alone, which misses a socket
//! directory `PGHOST` names.
#![cfg(all(feature = "tls", any(feature = "postgres", feature = "mysql")))]

use serde_json::json;
use ulo::{BoxError, Permanent};
use ulo_db::Store;
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_seaorm::{SeaOrm, SeaOrmOptions};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;
#[path = "../../ulo-db-conformance/support/stub.rs"]
mod stub;

/// The class, then the kind of the I/O error SQLx raised underneath SeaORM's.
fn word(outcome: Result<(), BoxError>) -> String {
    use sea_orm::{DbErr, RuntimeErr};

    let Err(error) = outcome else { return "CONNECTED".to_owned() };
    let (class, inner): (_, &(dyn std::error::Error + 'static)) = match error.downcast_ref::<Permanent>() {
        Some(permanent) => ("PERMANENT", std::error::Error::source(permanent).expect("a permanent error carries its cause")),
        None => ("RETRYABLE", &*error),
    };
    match inner.downcast_ref::<DbErr>() {
        Some(DbErr::Conn(RuntimeErr::SqlxError(sqlx))) => match &**sqlx {
            sea_orm::sqlx::Error::Io(io) => format!("{class}:{:?}", io.kind()),
            other => format!("{class}:{other:?}"),
        },
        other => format!("{class}:{other:?}"),
    }
}

fn pinged(url: &str) -> String {
    <OnTokio as Harness>::block_on(async {
        let options: SeaOrmOptions = server::options(json!({ "url": url, "acquire_timeout_secs": 20 }));
        let connection = <SeaOrm as Store>::create(&options).await.expect("create reads no environment and refuses nothing");
        word(<SeaOrm as Store>::ping(&connection).await)
    })
}

#[cfg(feature = "postgres")]
mod postgres {
    use std::path::Path;
    use std::process::Command;

    use super::{pinged, server, stub};

    const SCENARIO: &str = "ULO_SEAORM_ROOT_CERT_SCENARIO";

    /// The scenario the parent chose through the environment it started this process with: the
    /// ping's outcome as one word on stdout.
    #[test]
    fn child() {
        let Ok(scenario) = std::env::var(SCENARIO) else { return };
        let url = match scenario.as_str() {
            "tcp" => format!("postgres://u:p@127.0.0.1:{}/app?sslmode=verify-ca", stub::postgres_ssl()),
            // A directory that holds no socket: the connect fails with `NotFound`, as a socket not
            // yet created does.
            "socket" => format!("postgres://u:p@localhost/app?sslmode=verify-ca&host={}", std::env::temp_dir().display()),
            // No host in the URL: SQLx takes `PGHOST`, which the parent set to such a directory.
            "pghost" => "postgres:///app?sslmode=verify-ca".to_owned(),
            other => panic!("unknown scenario {other}"),
        };
        println!("\nOUTCOME:{}", pinged(&url));
    }

    fn outcome(scenario: &str, root: &Path, env: &[(&str, &Path)]) -> String {
        let mut cmd = Command::new(std::env::current_exe().expect("this binary"));
        cmd.args(["--exact", "postgres::child", "--nocapture", "--test-threads=1"]);
        for var in ["PGHOST", "PGHOSTADDR", "PGPORT", "PGUSER", "PGPASSWORD", "PGDATABASE", "PGSSLMODE", "PGSSLROOTCERT", "PGSSLCERT", "PGSSLKEY"] {
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
        let dir = server::Scratch::new("sea-env-root-missing");
        assert_eq!(outcome("tcp", &dir.0.join("nope.pem"), &[]), "PERMANENT:NotFound");
    }

    /// A file the process may not read: mode 000. Skipped where the process is root, which reads it.
    #[cfg(unix)]
    #[test]
    fn a_root_certificate_unreadable_from_pgsslrootcert_is_permanent_over_tcp() {
        use std::os::unix::fs::PermissionsExt;

        let dir = server::Scratch::new("sea-env-root-unreadable");
        let path = dir.0.join("locked.pem");
        std::fs::write(&path, "x").expect("a file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read(&path).is_ok() {
            return;
        }
        assert_eq!(outcome("tcp", &path, &[]), "PERMANENT:PermissionDenied");
    }

    #[cfg(unix)]
    #[test]
    fn the_same_error_over_a_socket_stays_retryable() {
        let dir = server::Scratch::new("sea-env-root-socket");
        assert_eq!(outcome("socket", &dir.0.join("nope.pem"), &[]), "RETRYABLE:NotFound");
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_directory_named_by_pghost_keeps_the_error_retryable() {
        let dir = server::Scratch::new("sea-env-root-pghost");
        let sockets = server::Scratch::new("sea-env-root-pghost-dir");
        assert_eq!(outcome("pghost", &dir.0.join("nope.pem"), &[("PGHOST", &sockets.0)]), "RETRYABLE:NotFound");
    }
}

#[cfg(feature = "mysql")]
mod mysql {
    use super::{pinged, server, stub};

    #[test]
    fn a_client_certificate_missing_over_tcp_is_permanent() {
        let dir = server::Scratch::new("sea-mysql-reach-tcp");
        let missing = dir.0.join("nope.pem").display().to_string();
        let url = format!("mysql://u:p@127.0.0.1:{}/app?ssl-mode=required&ssl-cert={missing}&ssl-key={missing}", stub::mysql_tls());
        assert_eq!(pinged(&url), "PERMANENT:NotFound");
    }

    #[cfg(unix)]
    #[test]
    fn the_same_error_over_a_socket_stays_retryable() {
        let dir = server::Scratch::new("sea-mysql-reach-socket");
        let missing = dir.0.join("nope.pem").display().to_string();
        let socket = dir.0.join("mysqld.sock").display().to_string();
        let url = format!("mysql://u:p@localhost/app?socket={socket}&ssl-mode=required&ssl-cert={missing}&ssl-key={missing}");
        assert_eq!(pinged(&url), "RETRYABLE:NotFound");
    }
}
