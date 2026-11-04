//! `ulo_db::Migrations` for SQLx's `Migrator`, owned and `&'static`, run inside `ulo-db`'s lock.
//! SQLx's own lock nests inside it, taken on another connection under its own key, and never
//! waits on it. A `&'static Migrator` answers its address as its identity; an owned one answers
//! none, so `.migrate(..)` compares its call.
//!
//! `Migrator::run` is named by path throughout: on an `Arc<Migrator>` the method call
//! `migrator.run(&pool)` resolves to the trait's `run(self: Arc<Self>, ..)` first.

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::sync::Arc;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use sqlx::migrate::Migrator;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::{BoxError, BoxFuture};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo_db::{Migrations, MigratorIdentity};

#[cfg(feature = "postgres")]
impl Migrations<crate::Postgres> for Migrator {
    fn run(self: Arc<Self>, client: sqlx::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move { Migrator::run(&self, &client).await.map_err(BoxError::from) })
    }
}

#[cfg(feature = "mysql")]
impl Migrations<crate::MySql> for Migrator {
    fn run(self: Arc<Self>, client: sqlx::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move { Migrator::run(&self, &client).await.map_err(BoxError::from) })
    }
}

#[cfg(feature = "sqlite")]
impl Migrations<crate::Sqlite> for Migrator {
    fn run(self: Arc<Self>, client: sqlx::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move { Migrator::run(&self, &client).await.map_err(BoxError::from) })
    }
}

#[cfg(feature = "postgres")]
impl Migrations<crate::Postgres> for &'static Migrator {
    fn run(self: Arc<Self>, client: sqlx::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let migrator: &'static Migrator = *self;
        Box::pin(async move { Migrator::run(migrator, &client).await.map_err(BoxError::from) })
    }

    fn identity(&self) -> Option<MigratorIdentity> {
        Some(MigratorIdentity::of_static(*self))
    }
}

#[cfg(feature = "mysql")]
impl Migrations<crate::MySql> for &'static Migrator {
    fn run(self: Arc<Self>, client: sqlx::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let migrator: &'static Migrator = *self;
        Box::pin(async move { Migrator::run(migrator, &client).await.map_err(BoxError::from) })
    }

    fn identity(&self) -> Option<MigratorIdentity> {
        Some(MigratorIdentity::of_static(*self))
    }
}

#[cfg(feature = "sqlite")]
impl Migrations<crate::Sqlite> for &'static Migrator {
    fn run(self: Arc<Self>, client: sqlx::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        let migrator: &'static Migrator = *self;
        Box::pin(async move { Migrator::run(migrator, &client).await.map_err(BoxError::from) })
    }

    fn identity(&self) -> Option<MigratorIdentity> {
        Some(MigratorIdentity::of_static(*self))
    }
}
