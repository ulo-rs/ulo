//! What a store module's identity and its refusals look like from outside: found by its own type
//! and key, named so in a failure report, and a lazy module that migrates refused at `wire()`.
//! SQLite on a file, so none of it needs a server.

#![cfg(feature = "sqlite")]

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use ulo::{App, ConnectError, Module, ModuleDef, ModuleIdentity, StartupError, WiringError};
use ulo_db::{LazyWithMigrations, StoreModule};
use ulo_db_sqlx::{Sqlite, SqliteOptions, SqlxModule, sqlx};
use ulo_runtime_tokio::Tokio;

struct Primary;
struct Replica;

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// A database file of its own, created by the URL's `mode=rwc`, removed on drop.
struct Db(PathBuf);

impl Db {
    fn new() -> Db {
        let dir = std::env::temp_dir().join(format!("ulo-wiring-sqlite-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&dir).expect("a directory for the database file");
        Db(dir)
    }

    fn options(&self) -> SqliteOptions {
        let url = format!("sqlite://{}?mode=rwc", self.0.join("db.sqlite").display());
        serde_json::from_value(json!({ "url": url, "max_connections": 2 })).expect("sqlite options")
    }

    fn module(&self) -> SqlxModule<Sqlite> {
        StoreModule::from_options(self.options())
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn migrator(sql: &'static str) -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![Migration::new(
            1,
            "wiring".into(),
            MigrationType::Simple,
            sqlx::SqlSafeStr::into_sql_str(sql),
            false,
        )]),
        ..Migrator::DEFAULT
    }
}

/// A root that imports whatever it is given, once.
type Import = Box<dyn FnOnce(&mut ModuleDef<'_>) + Send>;

struct Root(Mutex<Vec<Import>>);

impl Root {
    fn new() -> Root {
        Root(Mutex::new(Vec::new()))
    }

    fn import<M: Module>(self, module: M) -> Root {
        self.0.lock().unwrap().push(Box::new(move |m| {
            m.import(module);
        }));
        self
    }
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for import in std::mem::take(&mut *self.0.lock().unwrap()) {
            import(m);
        }
    }
}

fn app(root: Root) -> ulo::AppBuilder {
    App::builder(root).runtime(Tokio::current())
}

#[tokio::test]
async fn a_keyed_store_module_is_found_by_its_type_and_key() {
    let (primary, replica) = (Db::new(), Db::new());
    let wired = app(Root::new().import(primary.module().keyed::<Primary>()).import(replica.module().keyed::<Replica>()))
        .wire()
        .expect("two keyed store modules wire")
        .connect()
        .await
        .expect("two keyed store modules connect");
    wired.module_keyed::<SqlxModule<Sqlite>, Primary>().expect("the module keyed `Primary` is found by its type");
    wired.module_keyed::<SqlxModule<Sqlite>, Replica>().expect("the module keyed `Replica` is found by its type");
}

#[tokio::test]
async fn a_failed_hook_names_the_store_module_and_its_key() {
    let db = Db::new();
    let module = db.module().migrate(migrator("this is not sql")).keyed::<Primary>();
    let wired = app(Root::new().import(module)).wire().expect("the module wires");
    let Err(StartupError::Connect(error @ ConnectError::Hook { .. })) = wired.connect().await else {
        panic!("a migration that is not SQL did not fail the init hook");
    };
    let ConnectError::Hook { key, .. } = &error else { unreachable!() };
    assert_eq!(key.key().type_name().to_string(), "StoreModule<Sqlite>");
    assert!(error.to_string().starts_with("`OnModuleInit` hook of `StoreModule<Sqlite> @ Primary` failed: "), "{error}");
}

#[tokio::test]
async fn lazy_with_migrations_is_refused_at_wire_naming_both_calls() {
    let db = Db::new();
    let lazy_line = line!() + 1;
    let module = db.module().lazy();
    let migrate_line = line!() + 1;
    let module = module.migrate(migrator("create table t (id integer)"));
    let Err(StartupError::Wiring(errors)) = app(Root::new().import(module)).wire() else {
        panic!("a lazy module that migrates wired");
    };
    let failures: Vec<_> = errors
        .iter()
        .filter_map(|entry| match entry {
            WiringError::ValueFailed { error, key, module, .. } => error.downcast_ref::<LazyWithMigrations>().map(|e| (e, key, module)),
            _ => None,
        })
        .collect();
    assert_eq!(failures.len(), 1, "{errors}");
    assert_eq!(errors.iter().count(), 1, "the refusal is the only wiring error: {errors}");
    let (refusal, key, module) = failures[0];
    assert_eq!(refusal.store(), "sqlite");
    assert_eq!((refusal.lazy_at().file(), refusal.lazy_at().line()), (file!(), lazy_line));
    assert_eq!((refusal.migrate_at().file(), refusal.migrate_at().line()), (file!(), migrate_line));
    assert_eq!(key.key().type_name().to_string(), "LazyWithMigrations");
    assert_eq!(module.to_string(), "StoreModule<Sqlite>");
    let text = errors.to_string();
    assert!(text.contains(&format!("{}:{lazy_line}", file!())) && text.contains(&format!("{}:{migrate_line}", file!())), "{text}");
}

#[tokio::test]
async fn migrate_then_lazy_is_refused_the_same_way() {
    let db = Db::new();
    let migrate_line = line!() + 1;
    let module = db.module().migrate(migrator("create table t (id integer)"));
    let lazy_line = line!() + 1;
    let module = module.lazy();
    let Err(StartupError::Wiring(errors)) = app(Root::new().import(module)).wire() else {
        panic!("a module that migrates and then is made lazy wired");
    };
    let refusals: Vec<_> = errors
        .iter()
        .filter_map(|entry| match entry {
            WiringError::ValueFailed { error, .. } => error.downcast_ref::<LazyWithMigrations>(),
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 1, "{errors}");
    assert_eq!((refusals[0].lazy_at().line(), refusals[0].migrate_at().line()), (lazy_line, migrate_line));
}

/// A module whose database file is absent and may not be created, so its readiness ping fails.
fn unreachable_module(name: &str) -> SqlxModule<Sqlite> {
    let url = format!("sqlite://{}?mode=rw", std::env::temp_dir().join(format!("ulo-absent-{}-{name}", std::process::id())).join("db.sqlite").display());
    StoreModule::from_options(serde_json::from_value(json!({ "url": url, "max_connections": 1 })).expect("sqlite options"))
}

#[tokio::test]
async fn two_failing_readiness_checks_name_the_module_that_owns_each() {
    let wired = app(Root::new().import(unreachable_module("p").keyed::<Primary>())).wire().expect("the module wires");
    let Err(StartupError::Connect(primary)) = wired.connect().await else { panic!("an unreachable Primary connected") };
    let wired = app(Root::new().import(unreachable_module("r").keyed::<Replica>())).wire().expect("the module wires");
    let Err(StartupError::Connect(replica)) = wired.connect().await else { panic!("an unreachable Replica connected") };

    let mut seen = Vec::new();
    for (error, owner) in [(&primary, "Primary"), (&replica, "Replica")] {
        let ConnectError::Readiness { key, module, attempts, reason } = error else { panic!("expected a readiness failure: {error}") };
        assert_eq!(key.key().type_name().to_string(), "Pool<Sqlite>");
        assert_eq!(module.to_string(), format!("StoreModule<Sqlite> @ {owner}"));
        assert_eq!(*attempts, 1);
        assert!(matches!(reason, ulo::FailureReason::Errored(_)), "{reason}");
        let text = error.to_string();
        assert!(text.starts_with(&format!("readiness check of `Pool<Sqlite>` in StoreModule<Sqlite> @ {owner} failed after 1 attempt: ")), "{text}");
        seen.push(text);
    }
    assert_ne!(seen[0], seen[1]);
}
