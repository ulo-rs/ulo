use mongodb::Client;
use mongodb::bson::doc;
use ulo::{BoxError, Dep, injectable};

/// The project's queries go through the client this service holds. Inject `Dep<Database>` in the
/// services that need it, or `Dep<Client>` directly.
#[injectable]
pub struct Database {
    client: Dep<Client>,
}

impl Database {
    #[allow(dead_code, reason = "a starting point: nothing in the project queries the database yet")]
    pub async fn ping(&self) -> Result<(), BoxError> {
        self.client.database("admin").run_command(doc! { "ping": 1 }).await?;
        Ok(())
    }
}
