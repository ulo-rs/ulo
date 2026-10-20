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
/// A root certificate path that does not exist surfaces from SQLx as a bare `Io` of kind
/// `NotFound`, which is also what a Unix socket not yet created gives and so is not classified
/// here. `create` reads the named root certificate itself and marks an unreadable path `Permanent`
/// there.
///
/// A pool reports a refused connection as `PoolTimedOut` once its acquire timeout passes, so an
/// unreachable server and a wrong host both read as retryable here.
pub(crate) fn permanent_or_not(engine: Engine, error: sqlx::Error) -> BoxError {
    if is_permanent_error(engine, &error) { Permanent::new(error).into() } else { error.into() }
}

/// Whether `error` is one this crate's stores raise inside `ulo::Permanent` for `engine`: the table
/// the crate documentation's TLS and pool sections give. For a store built on SQLx's errors, such as
/// `ulo-db-seaorm`'s, to read the same table.
pub fn is_permanent_error(engine: Engine, error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Configuration(_) | sqlx::Error::Tls(_) => true,
        sqlx::Error::Database(database) => database.code().is_some_and(|code| permanent_code(engine, &code)),
        sqlx::Error::Io(io) => certificate_refused(io),
        _ => false,
    }
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
}
