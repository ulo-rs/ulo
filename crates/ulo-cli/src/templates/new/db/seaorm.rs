use sea_orm::DatabaseConnection;
use ulo::{BoxError, Dep, injectable};

/// The project's queries go through the connection this service holds. Inject `Dep<Database>` in
/// the services that need it, or `Dep<DatabaseConnection>` directly.
#[injectable]
pub struct Database {
    connection: Dep<DatabaseConnection>,
}

impl Database {
    #[allow(dead_code, reason = "a starting point: nothing in the project queries the database yet")]
    pub async fn ping(&self) -> Result<(), BoxError> {
        self.connection.ping().await?;
        Ok(())
    }
}
