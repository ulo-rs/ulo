#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::error::Error;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel::result::{ConnectionError, Error as QueryError};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_async::pooled_connection::PoolError as SetupError;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_async::pooled_connection::deadpool::PoolError;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::{BoxError, Permanent};

/// Message fragments, lowercased, of the failures retrying cannot mend, for the engines whose
/// server or driver states them in text alone:
///
/// - Postgres: a failed password or other authentication method, no `pg_hba.conf` entry, a role
///   that may not log in, no `CONNECT` privilege; tokio-postgres's own refusals of its
///   configuration, a password the server asked for and the URL lacks among them.
/// - MySQL: `Access denied for user` (1044, 1045) and `Unknown database` (1049).
/// - SQLite: a file that cannot be opened or is no database.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
const PERMANENT: &[&str] = &[
    "authentication failed",
    "no pg_hba.conf entry",
    "is not permitted to log in",
    "permission denied for database",
    "password missing",
    "invalid connection string",
    "invalid configuration",
    "access denied for user",
    "unknown database",
    "unable to open database file",
    "file is not a database",
];

/// Wraps in `ulo::Permanent` the errors retrying cannot help and leaves the rest bare.
///
/// diesel-async does not tell them apart by variant: wrong credentials and an unknown database both
/// arrive as `DatabaseError(Unknown, message)` with the SQLSTATE lost. The classification reads the
/// error's type down to that point and then matches the message against `PERMANENT`, together
/// with Postgres's `database "…" does not exist` and `role "…" does not exist`. A server whose
/// `lc_messages` is not English writes those messages translated, and a failure it reports is
/// then retried to the connect bound instead of ending the start at once. A URL the driver cannot
/// read, or one holding a NUL byte, is permanent by variant. `UnableToSendCommand` and
/// `BadConnection` are retryable, refused connections and failed lookups among them.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(crate) fn permanent_or_not(error: BoxError) -> BoxError {
    if permanent(&*error) { Permanent::new(error).into() } else { error }
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn permanent(error: &(dyn Error + 'static)) -> bool {
    if let Some(e) = error.downcast_ref::<PoolError>() {
        return match e {
            PoolError::Backend(setup) => setup_permanent(setup),
            PoolError::NoRuntimeSpecified => true,
            _ => false,
        };
    }
    if let Some(e) = error.downcast_ref::<SetupError>() {
        return setup_permanent(e);
    }
    if let Some(e) = error.downcast_ref::<ConnectionError>() {
        return connection_permanent(e);
    }
    if let Some(e) = error.downcast_ref::<QueryError>() {
        return query_permanent(e);
    }
    false
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn setup_permanent(error: &SetupError) -> bool {
    match error {
        SetupError::ConnectionError(e) => connection_permanent(e),
        SetupError::QueryError(e) => query_permanent(e),
    }
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn connection_permanent(error: &ConnectionError) -> bool {
    match error {
        ConnectionError::InvalidCString(_) | ConnectionError::InvalidConnectionUrl(_) => true,
        ConnectionError::BadConnection(message) => message_permanent(message),
        ConnectionError::CouldntSetupConfiguration(e) => query_permanent(e),
        _ => false,
    }
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn query_permanent(error: &QueryError) -> bool {
    match error {
        QueryError::DatabaseError(_, info) => message_permanent(info.message()),
        QueryError::InvalidCString(_) => true,
        _ => false,
    }
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn message_permanent(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    if PERMANENT.iter().any(|fragment| message.contains(fragment)) {
        return true;
    }
    // Postgres's 3D000 and the 28000 for an unknown role; "does not exist" alone would also take
    // in a missing relation.
    (message.starts_with("database \"") || message.starts_with("role \"")) && message.contains("\" does not exist")
}
