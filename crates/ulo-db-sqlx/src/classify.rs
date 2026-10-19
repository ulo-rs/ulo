use ulo::{BoxError, Permanent};

use crate::options::Engine;

/// Wraps an SQLx error in `ulo::Permanent` when retrying cannot help, and leaves the rest bare.
///
/// Permanent: `Error::Configuration`, a database error whose SQLSTATE class is `28` or whose
/// code is `3D000` on Postgres, `28000` or `42000` on MySQL, class `14` on SQLite, and a server
/// certificate rustls refused for anything but its validity period. Retryable: any other `Io`,
/// `PoolTimedOut`, `WorkerCrashed`, `Tls`. The engine is a parameter because the same SQLSTATE
/// means different things in each.
///
/// SQLx's pool hands a failed handshake back at once, as `Io` of kind `InvalidData` around the
/// `rustls::Error`. An unknown issuer, a name mismatch or a bad signature is met again on every
/// attempt and every restart; `Expired` and `NotValidYet` read the local clock, which can still be
/// wrong early in a boot, so they stay retryable.
///
/// A pool reports a refused connection as `PoolTimedOut` once its acquire timeout passes, so an
/// unreachable server and a wrong host both read as retryable here.
pub(crate) fn permanent_or_not(engine: Engine, error: sqlx::Error) -> BoxError {
    if is_permanent(engine, &error) { Permanent::new(error).into() } else { error.into() }
}

fn is_permanent(engine: Engine, error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Configuration(_) => true,
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
