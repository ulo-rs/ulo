use ulo::{BoxError, Permanent};

use crate::options::Engine;

/// Wraps an SQLx error in `ulo::Permanent` when retrying cannot help, and leaves the rest bare.
///
/// The test is who can fix it. Only the operator can mend a bad configuration, a refused
/// credential or a certificate the client will not accept, and the same attempt fails again on
/// every restart until they do. A server that is not up yet, a lost connection or a clock that is
/// early in a boot mends itself.
///
/// Permanent: `Error::Configuration`; `Error::Tls`, which SQLx raises for a TLS configuration it
/// cannot use (a root certificate that is not PEM or not X.509, a name that is no valid server
/// name, a server that offers no TLS where the URL requires it); a database error whose SQLSTATE
/// class is `28` or whose code is `3D000` on Postgres, `28000` or `42000` on MySQL, class `14` on
/// SQLite; and a server certificate rustls refused for anything but its validity period: an
/// unknown issuer, a name that is not the certificate's, a bad signature. Retryable: any other
/// `Io`, `PoolTimedOut`, `WorkerCrashed`, and an expired or not-yet-valid certificate. The engine
/// is a parameter because the same SQLSTATE means different things in each.
///
/// SQLx's pool hands a failed handshake back at once, as `Io` of kind `InvalidData` around the
/// `rustls::Error`. `Expired` and `NotValidYet` read the local clock, which can still be wrong
/// early in a boot, so they stay retryable.
///
/// A certificate file that does not exist or cannot be read surfaces from SQLx as a bare `Io` of
/// kind `NotFound` or `PermissionDenied`, which is also what a Unix socket not yet created gives.
/// Over a socket it is therefore not classified. Over a host and port nothing else in a connect
/// reads a file, so it is `Permanent` whichever way the path reached SQLx: the URL, which `create`
/// has already read in the verifying modes and refused there, or `PGSSLROOTCERT`, `PGSSLCERT` and
/// `PGSSLKEY`, which SQLx's option parser reads itself and this crate does not, so `create` cannot
/// know them. The stores pass how the pool reaches its server, as `pg_reaches_by_tcp` and
/// `mysql_reaches_by_tcp` read it; [`is_permanent_error`] assumes a socket.
///
/// A pool reports a refused connection as `PoolTimedOut` once its acquire timeout passes, so an
/// unreachable server and a wrong host both read as retryable here.
#[cfg(feature = "sqlite")]
pub(crate) fn permanent_or_not(engine: Engine, error: sqlx::Error) -> BoxError {
    permanent_or_not_over(engine, error, false)
}

/// [`permanent_or_not`] for a connection that `over_tcp` reaches by host and port.
pub(crate) fn permanent_or_not_over(engine: Engine, error: sqlx::Error, over_tcp: bool) -> BoxError {
    if permanent(engine, &error, over_tcp) { Permanent::new(error).into() } else { error.into() }
}

/// Whether `error` is one this crate's stores raise inside `ulo::Permanent` for `engine`: the table
/// the crate documentation's TLS and pool sections give. For a store built on SQLx's errors, such as
/// `ulo-db-seaorm`'s, to read the same table. A bare `Io` of kind `NotFound` stays retryable here,
/// since the connection may be a Unix socket; [`is_permanent_error_over_tcp`] reads it as a
/// certificate file, for a pool `pg_reaches_by_tcp` or `mysql_reaches_by_tcp` answers `true` for.
pub fn is_permanent_error(engine: Engine, error: &sqlx::Error) -> bool {
    permanent(engine, error, false)
}

/// [`is_permanent_error`] for a connection made by host and port, where a file that cannot be found
/// or read is a certificate file: `Permanent`, from whichever source the path came (the rule
/// stated on `permanent_or_not` above).
pub fn is_permanent_error_over_tcp(engine: Engine, error: &sqlx::Error) -> bool {
    permanent(engine, error, true)
}

fn permanent(engine: Engine, error: &sqlx::Error, over_tcp: bool) -> bool {
    match error {
        sqlx::Error::Configuration(_) | sqlx::Error::Tls(_) => true,
        sqlx::Error::Database(database) => database.code().is_some_and(|code| permanent_code(engine, &code)),
        sqlx::Error::Io(io) => certificate_refused(io) || (over_tcp && file_unreadable(io)),
        _ => false,
    }
}

/// An operating-system error reading a file: kind `NotFound` or `PermissionDenied` with no error
/// wrapped inside, which is how `std::fs::read` reports both and how nothing else on a TCP connect
/// does.
fn file_unreadable(io: &std::io::Error) -> bool {
    io.get_ref().is_none() && matches!(io.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied)
}

#[cfg(feature = "tls")]
fn certificate_refused(io: &std::io::Error) -> bool {
    use rustls::CertificateError as C;

    match io.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
        Some(rustls::Error::InvalidCertificate(
            C::Expired | C::ExpiredContext { .. } | C::NotValidYet | C::NotValidYetContext { .. },
        )) => false,
        Some(rustls::Error::InvalidCertificate(_)) => true,
        _ => false,
    }
}

#[cfg(not(feature = "tls"))]
fn certificate_refused(_: &std::io::Error) -> bool {
    false
}

fn permanent_code(engine: Engine, code: &str) -> bool {
    match engine {
        // Class 28 is invalid authorization (`28P01` wrong password, `28000`), `3D000` an unknown
        // database.
        Engine::Postgres => code.starts_with("28") || code == "3D000",
        Engine::MySql => matches!(code, "28000" | "42000"),
        // SQLite reports a result code, not a SQLSTATE. 14 is `SQLITE_CANTOPEN`; its extended
        // codes (a directory, a missing path) carry 14 in the low byte.
        Engine::Sqlite => code.parse::<i64>().is_ok_and(|n| n & 0xff == 14),
    }
}

#[cfg(all(test, feature = "tls"))]
mod tests {
    use std::io;

    use rustls::CertificateError as C;

    use super::*;

    fn refused(error: C) -> sqlx::Error {
        sqlx::Error::Io(io::Error::new(io::ErrorKind::InvalidData, rustls::Error::InvalidCertificate(error)))
    }

    #[test]
    fn a_certificate_nobody_here_can_mend_by_waiting_is_permanent() {
        for error in [
            C::UnknownIssuer,
            C::NotValidForName,
            C::NotValidForNameContext { expected: rustls::pki_types::ServerName::try_from("db").unwrap(), presented: vec![] },
            C::BadSignature,
            C::Revoked,
        ] {
            let label = format!("{error:?}");
            assert!(is_permanent_error(Engine::MySql, &refused(error)), "{label} is retryable");
        }
    }

    #[test]
    fn a_certificate_the_clock_refuses_is_retryable() {
        for error in [C::Expired, C::NotValidYet] {
            let label = format!("{error:?}");
            assert!(!is_permanent_error(Engine::MySql, &refused(error)), "{label} is permanent");
        }
    }

    #[test]
    fn a_tls_configuration_sqlx_cannot_use_is_permanent() {
        assert!(is_permanent_error(Engine::Postgres, &sqlx::Error::Tls("Invalid certificate file: ca.pem".into())));
    }

    #[test]
    fn the_rest_of_io_stays_retryable() {
        for kind in [io::ErrorKind::ConnectionReset, io::ErrorKind::UnexpectedEof, io::ErrorKind::NotFound] {
            assert!(!is_permanent_error(Engine::MySql, &sqlx::Error::Io(io::Error::from(kind))), "{kind:?} is permanent");
        }
    }

    /// Over a host and port the only file a connect reads is a certificate's, so a file that is
    /// missing or unreadable is the operator's to mend; the rest of `Io` is the network's.
    #[test]
    fn a_certificate_file_that_cannot_be_read_is_permanent_over_tcp_and_retryable_over_a_socket() {
        for engine in [Engine::Postgres, Engine::MySql] {
            for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
                let error = sqlx::Error::Io(io::Error::from(kind));
                assert!(is_permanent_error_over_tcp(engine, &error), "{engine:?} {kind:?} over tcp is retryable");
                assert!(!is_permanent_error(engine, &error), "{engine:?} {kind:?} over a socket is permanent");
            }
            for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::ConnectionReset, io::ErrorKind::TimedOut, io::ErrorKind::UnexpectedEof] {
                let error = sqlx::Error::Io(io::Error::from(kind));
                assert!(!is_permanent_error_over_tcp(engine, &error), "{engine:?} {kind:?} over tcp is permanent");
            }
        }
        // An error with a cause is not the operating system's answer to a file read.
        let wrapped = sqlx::Error::Io(io::Error::new(io::ErrorKind::NotFound, "a proxy said so"));
        assert!(!is_permanent_error_over_tcp(Engine::Postgres, &wrapped));
    }
}
