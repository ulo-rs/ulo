//! The migration lock's three primitives over diesel-async's connections.
//!
//! Postgres: `pg_advisory_lock(K)` on a connection held for the migration. MySQL: `GET_LOCK` on
//! `'ulo-db.migrate.'` followed by the database's name, hashed past MySQL's 64 characters. SQLite: an
//! OS lock on `<file>.ulo-migrate.lock` through `std::fs::File::lock`, taken off the worker, and none
//! for an in-memory database.

use std::any::Any;

/// The held lock: a pooled connection, or the locked sidecar file.
pub struct DieselLock {
    pub(crate) held: Box<dyn Any + Send>,
}
