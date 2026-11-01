//! A start whose wait for the SQLite migration lock ends near the bound, against a real file whose
//! sidecar lock another holder frees: a lock freed in the last poll interval of the wait is taken,
//! and one never freed fails retryably at the bound.
//! Planted break: the wait ending at its deadline with no attempt at it.
#![cfg(feature = "sqlite")]

use std::fs::File;
use std::time::{Duration, Instant};

use diesel_migrations::FileBasedMigrations;
use serde_json::json;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, is_permanent};
use ulo_db::StoreModule;
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_diesel::{DieselMigrations, DieselOptions, Sqlite};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

const BOUND: Duration = Duration::from_millis(800);

/// 0.95 of `BOUND`, inside the last poll interval of the lock's wait.
const FREED_AT: Duration = Duration::from_millis(760);

struct Root {
    options: DieselOptions,
    migrations: std::path::PathBuf,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.options.url.expose().clone())
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let source = FileBasedMigrations::from_path(&self.migrations).expect("the migrations directory reads");
        m.import(StoreModule::<Sqlite>::from_options(self.options.clone()).connect_within(BOUND).migrate(DieselMigrations::new(source)));
    }
}

fn migrations_in(dir: &std::path::Path) -> std::path::PathBuf {
    let migrations = dir.join("migrations");
    let one = migrations.join("2026-01-01-000001_lock_wait");
    std::fs::create_dir_all(&one).expect("a migration directory");
    std::fs::write(one.join("up.sql"), "create table lock_wait_t (id integer)").expect("up.sql");
    std::fs::write(one.join("down.sql"), "drop table lock_wait_t").expect("down.sql");
    migrations
}

fn root(dir: &server::Scratch) -> Root {
    let file = dir.0.join("app.db");
    File::create(&file).expect("the database file is created");
    let options: DieselOptions = server::options(json!({ "url": file.display().to_string(), "max_size": 5 }));
    Root { options, migrations: migrations_in(&dir.0) }
}

fn hold(dir: &server::Scratch) -> File {
    let held = File::create(dir.0.join("app.db.ulo-migrate.lock")).expect("the sidecar is created");
    held.lock().expect("the sidecar is locked");
    held
}

#[test]
fn a_held_sqlite_lock_fails_the_start_retryably_at_the_bound() {
    let dir = server::Scratch::new("diesel-lock-wait");
    let root = root(&dir);
    let held = hold(&dir);
    let (error, took) = <OnTokio as Harness>::block_on(async move {
        let app = App::builder(root).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
        let began = Instant::now();
        match app.connect().await {
            Err(error) => (error, began.elapsed()),
            Ok(_) => panic!("the start took a migration lock another holder has"),
        }
    });
    assert!(!is_permanent(&error), "a wait for another replica's lock is permanent: {error}");
    assert!(took >= BOUND - Duration::from_millis(50) && took < BOUND * 3, "failed after {took:?}, against the {BOUND:?} bound: {error}");
    drop(held);
}

#[test]
fn a_sqlite_lock_freed_near_the_bound_still_migrates() {
    // Where the last poll falls against the release differs from start to start, so one start
    // meets the break only some of the time; three starts meet it in most runs.
    for round in 0..3 {
        let dir = server::Scratch::new("diesel-lock-late");
        let root = root(&dir);
        let held = hold(&dir);
        let release = std::thread::spawn(move || {
            std::thread::sleep(FREED_AT);
            drop(held);
        });
        let began = Instant::now();
        <OnTokio as Harness>::block_on(async move {
            let app = App::builder(root).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
            let app = app
                .connect()
                .await
                .unwrap_or_else(|error| panic!("round {round}: a lock freed before the bound failed the start: {error}"));
            let _ = app.close(Signal::new("test")).await;
        });
        release.join().expect("the releasing thread ends");
        assert!(began.elapsed() >= FREED_AT - Duration::from_millis(50), "the start did not wait for the lock");
    }
}
