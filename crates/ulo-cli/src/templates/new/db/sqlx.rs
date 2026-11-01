use ulo::{BoxError, Dep, injectable};
use ulo_db_sqlx::sqlx::{self, @@pool@@};

/// The project's queries go through the pool this service holds. Inject `Dep<Database>` in the
/// services that need it, or `Dep<@@pool@@>` directly.
#[injectable]
pub struct Database {
    pool: Dep<@@pool@@>,
}

impl Database {
    #[allow(dead_code, reason = "a starting point: nothing in the project queries the database yet")]
    pub async fn ping(&self) -> Result<(), BoxError> {
        sqlx::query("select 1").execute(&*self.pool).await?;
        Ok(())
    }
}
