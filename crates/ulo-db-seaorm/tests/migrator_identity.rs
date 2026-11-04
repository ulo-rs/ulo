//! `SeaOrmMigrations<M>` compares by `M`'s type: two imports migrating with one migrator type are
//! one module, and two migrator types are refused at `wire()`. Nothing connects: `wire()` reads the
//! settings alone.

#![cfg(feature = "sqlite")]

use sea_orm_migration::prelude::*;
use serde_json::json;
use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, WiringError, WiringErrors};
use ulo_db_conformance::{Harness, OnTokio};
use ulo_db_seaorm::{SeaOrmMigrations, SeaOrmModule, SeaOrmOptions};

struct Accounts;

impl MigratorTrait for Accounts {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Vec::new()
    }
}

struct Billing;

impl MigratorTrait for Billing {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Vec::new()
    }
}

/// Imports each store module, in order.
struct Root(Vec<fn() -> SeaOrmModule>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Root>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for module in &self.0 {
            m.import(module());
        }
    }
}

fn store() -> SeaOrmModule {
    let options: SeaOrmOptions = serde_json::from_value(json!({ "url": "sqlite::memory:" })).expect("sqlite options");
    SeaOrmModule::from_options(options)
}

fn wired(root: Root) -> Result<(), WiringErrors> {
    let wired = <OnTokio as Harness>::block_on(async move { App::builder(root).runtime(<OnTokio as Harness>::runtime()).wire().map(drop) });
    match wired {
        Ok(()) => Ok(()),
        Err(StartupError::Wiring(errors)) => Err(errors),
        Err(other) => panic!("expected wiring errors, got: {other}"),
    }
}

/// Planted break: `SeaOrmMigrations::identity` left at the trait's default, which compares the
/// `.migrate(..)` calls and refuses the first pair.
#[test]
fn one_migrator_type_is_one_module_wherever_it_is_written() {
    wired(Root(vec![
        || store().migrate(SeaOrmMigrations::<Accounts>::new()),
        || store().migrate(SeaOrmMigrations::<Accounts>::new()),
    ]))
    .unwrap_or_else(|errors| panic!("one migrator type at two calls is one module:\n{errors}"));
}

/// Planted break: `SeaOrmMigrations::identity` answering one identity for every `M`.
#[test]
fn two_migrator_types_are_refused() {
    fn migrating<M: MigratorTrait + 'static>() -> SeaOrmModule {
        store().migrate(SeaOrmMigrations::<M>::new())
    }
    let errors = match wired(Root(vec![migrating::<Accounts>, migrating::<Billing>])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two migrator types under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let names: Vec<_> = settings.iter().map(|setting| setting.name).collect();
    assert_eq!((names, errors.len()), (vec!["migrate"], 1), "{errors}");
    let sides = [settings[0].first.as_deref().unwrap_or_default(), settings[0].second.as_deref().unwrap_or_default()];
    assert!(sides[0].starts_with(&format!("`{}` (", std::any::type_name::<Accounts>())), "{sides:?}");
    assert!(sides[1].starts_with(&format!("`{}` (", std::any::type_name::<Billing>())), "{sides:?}");
}
