use ulo::{BoxError, Permanent};

use crate::options::Engine;

/// Wraps an SQLx error in `ulo::Permanent` when retrying cannot help, and leaves the rest bare.
///
/// Permanent: `Error::Configuration`, and a database error whose SQLSTATE class is `28` or whose
/// code is `3D000` on Postgres, `28000` or `42000` on MySQL, class `14` on SQLite. Retryable:
/// `Io`, `PoolTimedOut`, `WorkerCrashed`, `Tls`. The engine is a parameter because the same SQLSTATE
/// means different things in each.
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
        _ => false,
    }
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
