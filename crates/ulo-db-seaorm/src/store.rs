use sea_orm::DatabaseConnection;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, Store};

use crate::options::SeaOrmOptions;

/// The SeaORM store; the client is `sea_orm::DatabaseConnection`.
///
/// Errors are classified through the SQLx error SeaORM carries, as `ulo-db-sqlx`'s table does.
pub struct SeaOrm;

impl Store for SeaOrm {
    const NAME: &'static str = "seaorm";
    type Options = SeaOrmOptions;
    type Client = DatabaseConnection;
    const CREATE_CONNECTS: bool = false;

    fn validate(o: &SeaOrmOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URL's password component, beside the whole URL the section read registered.
    fn secrets(o: &SeaOrmOptions) -> Vec<Secret<String>> {
        let _ = o;
        todo!()
    }

    /// `ConnectOptions::connect_lazy(true)`: connects nothing.
    fn create(o: &SeaOrmOptions) -> BoxFuture<'static, Result<DatabaseConnection, BoxError>> {
        let _ = o;
        todo!()
    }

    fn ping(c: &DatabaseConnection) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = c;
        todo!()
    }

    fn close(c: DatabaseConnection) -> BoxFuture<'static, ()> {
        let _ = c;
        todo!()
    }
}

impl LazyStore for SeaOrm {}
