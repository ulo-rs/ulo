//! `ulo_db::Migrations` for SQLx's `Migrator`, run inside `ulo-db`'s lock. SQLx's own lock nests
//! inside it, taken on another connection under its own key, and never waits on it.

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::sync::Arc;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use sqlx::migrate::Migrator;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::{BoxError, BoxFuture};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo_db::Migrations;

#[cfg(feature = "postgres")]
impl Migrations<crate::Postgres> for Migrator {
    fn run(self: Arc<Self>, client: sqlx::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (self, client);
        todo!()
    }
}

#[cfg(feature = "mysql")]
impl Migrations<crate::MySql> for Migrator {
    fn run(self: Arc<Self>, client: sqlx::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (self, client);
        todo!()
    }
}

#[cfg(feature = "sqlite")]
impl Migrations<crate::Sqlite> for Migrator {
    fn run(self: Arc<Self>, client: sqlx::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (self, client);
        todo!()
    }
}
