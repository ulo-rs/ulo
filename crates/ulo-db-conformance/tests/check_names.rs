//! The name a keyed store's health check prints: `S::NAME @ Key`, the key read from the nearest
//! keyed module on the path from the store upward, written in full when two keys print alike.
//! The store is a fake that connects to nothing.

use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio};
use ulo_db::{Store, StoreModule};
use ulo_health::{HealthModule, Readiness};

struct Fake;

impl Store for Fake {
    const NAME: &'static str = "fake";
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

mod a {
    pub struct Primary;
}

mod b {
    pub struct Primary;
}

struct First;
struct Second;

/// An application module that imports one store, as most do; `own` keys the store itself.
struct Inner {
    options: &'static str,
    own: bool,
}

impl Module for Inner {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&(self.options, self.own))
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if self.own {
            m.import(store(self.options).keyed::<First>());
        } else {
            m.import(store(self.options));
        }
    }
}

type Import = Box<dyn Fn(&mut ModuleDef<'_>) + Send + Sync>;

struct Root(Vec<Import>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for import in &self.0 {
            import(m);
        }
        m.import(HealthModule::new());
    }
}

fn store(options: &str) -> StoreModule<Fake> {
    StoreModule::from_options(options.to_owned())
}

/// The check names the store contributed, sorted.
fn names(root: Root) -> Vec<String> {
    <OnTokio as Harness>::block_on(async move {
        let app = App::builder(root).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
        let app = app.connect().await.unwrap_or_else(|error| panic!("the app did not connect: {error}"));
        let readiness: Readiness = (*app.get::<Readiness>().await.expect("a readiness")).clone();
        let report = readiness.readiness().await;
        let mut names: Vec<String> = report.results().iter().map(|r| r.name().to_owned()).collect();
        names.sort();
        let _ = app.close(Signal::new("test")).await;
        names
    })
}

/// A store inside a keyed application module reads that module's key. Planted break:
/// `ModuleDef::qualifier` answering only the module's own key.
#[test]
fn a_store_inside_a_keyed_application_module_reads_its_key() {
    let root = Root(vec![Box::new(|m: &mut ModuleDef<'_>| {
        m.import(Inner { options: "one", own: false }.keyed::<First>());
    })]);
    assert_eq!(names(root), vec!["fake @ First".to_owned()]);
}

/// Two stores built with `from_options`, each inside a differently keyed application module, print
/// apart: the case the qualifier exists for.
#[test]
fn two_stores_in_two_keyed_application_modules_print_apart() {
    let root = Root(vec![
        Box::new(|m: &mut ModuleDef<'_>| m.import(Inner { options: "one", own: false }.keyed::<First>())),
        Box::new(|m: &mut ModuleDef<'_>| m.import(Inner { options: "two", own: false }.keyed::<Second>())),
    ]);
    assert_eq!(names(root), vec!["fake @ First".to_owned(), "fake @ Second".to_owned()]);
}

/// The nearest key wins: a store keyed on its own inside a keyed application module reads its own.
#[test]
fn the_nearest_key_wins() {
    let root = Root(vec![Box::new(|m: &mut ModuleDef<'_>| {
        m.import(Inner { options: "one", own: true }.keyed::<Second>());
    })]);
    assert_eq!(names(root), vec!["fake @ First".to_owned()]);
}

#[test]
fn an_unkeyed_store_prints_its_name_alone() {
    let root = Root(vec![Box::new(|m: &mut ModuleDef<'_>| m.import(store("one")))]);
    assert_eq!(names(root), vec!["fake".to_owned()]);
}

/// Two keys with equal last segments print in full and apart, the wiring report's rule
/// (`TypeName::colliding`); keys that print apart stay short beside them. Planted break: the
/// collision set ignored.
#[test]
fn keys_that_print_alike_print_in_full() {
    let root = Root(vec![
        Box::new(|m: &mut ModuleDef<'_>| m.import(store("one").keyed::<a::Primary>())),
        Box::new(|m: &mut ModuleDef<'_>| m.import(store("two").keyed::<b::Primary>())),
        Box::new(|m: &mut ModuleDef<'_>| m.import(store("three").keyed::<First>())),
    ]);
    let names = names(root);
    let in_full: Vec<&String> = names.iter().filter(|n| n.contains("::")).collect();
    assert_eq!(in_full.len(), 2, "{names:?}");
    assert!(in_full[0].ends_with("a::Primary") && in_full[1].ends_with("b::Primary"), "{names:?}");
    assert!(names.contains(&"fake @ First".to_owned()), "a key that prints apart was written in full: {names:?}");
}
