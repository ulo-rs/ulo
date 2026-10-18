use ulo::BoxError;

use crate::options::Engine;

/// Wraps an SQLx error in `ulo::Permanent` when retrying cannot help, and leaves the rest bare.
///
/// Permanent: `Error::Configuration`, and a database error whose SQLSTATE class is `28` or whose
/// code is `3D000` on Postgres, `28000` or `42000` on MySQL, class `14` on SQLite. Retryable:
/// `Io`, `PoolTimedOut`, `WorkerCrashed`, `Tls`. The engine is a parameter because the same SQLSTATE
/// means different things in each.
pub(crate) fn permanent_or_not(engine: Engine, error: sqlx::Error) -> BoxError {
    let _ = (engine, error);
    todo!()
}
