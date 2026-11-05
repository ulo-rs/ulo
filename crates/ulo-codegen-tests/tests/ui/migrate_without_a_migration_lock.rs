// `StoreModule::migrate` on a store that implements no `MigrationLock`.
// Refused at: the `.migrate(..)` call, the error naming the stores that take migrations.
use std::sync::Arc;

use ulo::{BoxError, BoxFuture};
use ulo_config::Problems;
use ulo_db::{Migrations, Store, StoreModule};

struct Lockless;

impl Store for Lockless {
    const NAME: &'static str = "lockless";
    type Options = String;
    type Client = String;

    fn create_connects(_: &String) -> bool {
        false
    }

    fn validate(_: &String) -> Result<(), Problems> {
        Ok(())
    }

    fn create(o: &String) -> BoxFuture<'static, Result<String, BoxError>> {
        let o = o.clone();
        Box::pin(async move { Ok(o) })
    }

    fn ping(_: &String) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(_: String) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

struct Migrator;

impl Migrations<Lockless> for Migrator {
    fn run(self: Arc<Self>, _: String) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
}

fn main() {
    let _ = StoreModule::<Lockless>::from_options(String::new()).migrate(Migrator);
}
