use redis::aio::ConnectionManager;
use ulo::{BoxError, Dep, injectable};

/// The project's commands go through the connection this service holds. Inject `Dep<Database>` in
/// the services that need it, or `Dep<ConnectionManager>` directly.
#[injectable]
pub struct Database {
    connection: Dep<ConnectionManager>,
}

impl Database {
    #[allow(dead_code, reason = "a starting point: nothing in the project uses the database yet")]
    pub async fn ping(&self) -> Result<(), BoxError> {
        let mut connection = ConnectionManager::clone(&self.connection);
        redis::cmd("PING").query_async::<String>(&mut connection).await?;
        Ok(())
    }
}
