//! A start whose wait for the SQLite migration lock reaches the bound, against a real file whose
//! sidecar lock another process holds: retryable, at the bound. The suite's replicas scenario never
//! reaches the bound, so without this nothing on a real engine reads the classification.
//! Planted break: the module wrapping every `MigrationFailed` in `Permanent`.
#![cfg(feature = "sqlite")]

use std::fs::File;
use std::time::{Duration, Instant};

use serde_json::json;
use ulo::{App, Module, ModuleDef, ModuleIdentity, is_permanent};
use ulo_db::StoreModule;
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_sqlx::sqlx::migrate::Migrator;
use ulo_db_sqlx::{Sqlite, SqliteOptions};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

const BOUND: Duration = Duration::from_millis(800);

struct Root(SqliteOptions);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0.url.expose().clone())
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(StoreModule::<Sqlite>::from_options(self.0.clone()).connect_within(BOUND).migrate(Migrator::DEFAULT));
    }
}

#[test]
fn a_held_sqlite_lock_fails_the_start_retryably_at_the_bound() {
    let dir = server::Scratch::new("lock-wait");
    let file = dir.0.join("app.db");
    File::create(&file).expect("the database file is created");
    let url = format!("sqlite://{}", file.display());
    // The other replica: the sidecar lock held for the whole start.
    let held = File::create(dir.0.join("app.db.ulo-migrate.lock")).expect("the sidecar is created");
    held.lock().expect("the sidecar is locked");

    let options: SqliteOptions = server::options(json!({ "url": url }));
    let (error, took) = <OnTokio as Harness>::block_on(async move {
        let app = App::builder(Root(options)).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
        let began = Instant::now();
        match app.connect().await {
            Err(error) => (error, began.elapsed()),
            Ok(_) => panic!("the start took a migration lock another holder has"),
        }
    });
    assert!(!is_permanent(&error), "a wait for another replica's lock is permanent: {error}");
    assert!(took >= BOUND && took < BOUND * 3, "failed after {took:?}, against the {BOUND:?} bound: {error}");
    drop(held);
}
