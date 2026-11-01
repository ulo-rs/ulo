//! Two imports of one `StoreModule` whose options are equal are one module, so they must carry
//! the same builder settings: `wire()` refuses a second import with other settings as
//! `WiringError::DifferingSettings`, naming both import sites and each setting that differs. The
//! store is a fake that connects to nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ulo::{App, Bound, BoxError, BoxFuture, LookupError, Module, ModuleDef, ModuleIdentity, ModuleSite, Signal, WiringError, WiringErrors};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio};
use ulo_db::{MigrationLock, Migrations, Store, StoreModule};

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

impl MigrationLock for Fake {
    type Guard = ();

    fn lock(_: &String, _: Duration) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn unlock(_: ()) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

struct Migrator;

impl Migrations<Fake> for Migrator {
    fn run(self: Arc<Self>, _: String) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
}

struct Primary;
struct Replica;

type Import = Box<dyn Fn(&mut ModuleDef<'_>) + Send + Sync>;

/// An application module that runs each of its imports; `K` tells two of them apart.
struct Host<K: 'static>(Vec<Import>, std::marker::PhantomData<fn() -> K>);

struct Root;
struct Inner;
struct Other;

fn module<K: 'static>(imports: Vec<Import>) -> Host<K> {
    Host(imports, std::marker::PhantomData)
}

impl<K: 'static> Module for Host<K> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<K>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for import in &self.0 {
            import(m);
        }
    }
}

fn store() -> StoreModule<Fake> {
    StoreModule::from_options("db".to_owned())
}

/// `wire()` on an app with the harness's runtime, which a store's readiness bounds need. The
/// runtime value is taken inside the harness's executor.
fn wire(root: impl Module + 'static) -> Result<App<ulo::Wired>, ulo::StartupError> {
    <OnTokio as Harness>::block_on(async move { App::builder(root).runtime(<OnTokio as Harness>::runtime()).wire() })
}

fn refused(root: impl Module + 'static) -> WiringErrors {
    match wire(root) {
        Err(ulo::StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("wire() failed, but not with wiring errors: {other}"),
        Ok(_) => panic!("wire() accepted two imports of one store with other settings"),
    }
}

/// The names of the settings the one `DifferingSettings` entry of `errors` names.
fn differing(errors: &WiringErrors) -> Vec<&'static str> {
    let found: Vec<Vec<&'static str>> = errors
        .iter()
        .filter_map(|error| match error {
            WiringError::DifferingSettings { settings, .. } => Some(settings.iter().map(|setting| setting.name).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "expected one `DifferingSettings` entry:\n{errors}");
    assert_eq!(errors.len(), 1, "and nothing else:\n{errors}");
    found.into_iter().next().unwrap_or_default()
}

/// Connects `root` and runs `check` on the connected app.
fn connected(root: impl Module, check: impl FnOnce(&App<ulo::Connected>)) {
    <OnTokio as Harness>::block_on(async move {
        let app = App::builder(root).runtime(<OnTokio as Harness>::runtime()).wire();
        let app = app.unwrap_or_else(|error| panic!("wire() refused the graph:\n{error}"));
        let app = app.connect().await.unwrap_or_else(|error| panic!("the app did not connect: {error}"));
        check(&app);
        let _ = app.close(Signal::new("test")).await;
    });
}

static FIRST: AtomicU32 = AtomicU32::new(0);
static AGAIN: AtomicU32 = AtomicU32::new(0);

/// On HEAD the second import is dropped and the module connects under the first's 5 s. Planted
/// break: `StoreModule::settings` leaving out `connect_within`.
#[test]
fn a_second_import_with_another_connect_within_is_refused_naming_both_sites() {
    #[rustfmt::skip]
    let root = module::<Root>(vec![
        Box::new(|m| { m.import(store().connect_within(Duration::from_secs(5))); FIRST.store(line!(), Ordering::SeqCst); }),
        Box::new(|m| m.import(module::<Inner>(vec![
            Box::new(|m| { m.import(store().connect_within(Duration::from_secs(10))); AGAIN.store(line!(), Ordering::SeqCst); }),
        ]))),
    ]);
    let errors = refused(root);
    assert_eq!(differing(&errors), ["connect_within"]);

    let Some(WiringError::DifferingSettings { module, first, second, settings }) = errors.iter().next() else {
        panic!("{errors}");
    };
    assert_eq!(module.to_string(), "StoreModule<Fake>");
    let site = |site: &ModuleSite| match site {
        ModuleSite::Import { importer, at } => (importer.to_string(), at.line()),
        other => panic!("expected an import site, got {other:?}"),
    };
    assert_eq!(site(first), ("Root".to_owned(), FIRST.load(Ordering::SeqCst)), "the import that claimed the identity");
    assert_eq!(site(second), ("Inner".to_owned(), AGAIN.load(Ordering::SeqCst)), "the later import");
    assert_eq!((settings[0].first.as_deref(), settings[0].second.as_deref()), (Some("5s"), Some("10s")));

    let expected = format!(
        "error: wiring failed with 1 error\n\
         \n  × two registrations of StoreModule<Fake> differ in `connect_within`\n\
         \x20   ├─ imported by Root at {file}:{first} with `connect_within` = 5s\n\
         \x20   ├─ imported again by Inner at {file}:{again} with `connect_within` = 10s\n\
         \x20   └─ help: an equal identity is one module, registered once; write the same settings at both, or key one with `.keyed::<Q>()` to make it a second instance",
        file = file!(),
        first = FIRST.load(Ordering::SeqCst),
        again = AGAIN.load(Ordering::SeqCst),
    );
    assert_eq!(errors.to_string(), expected);
}

/// Each builder setting is compared, and an import that differs in one names that one alone.
/// Planted break: any one of them left out of `StoreModule::settings`.
#[test]
fn each_builder_setting_is_compared() {
    type Set = fn(StoreModule<Fake>) -> StoreModule<Fake>;
    let cases: [(&str, Set); 6] = [
        ("lazy", |s| s.lazy()),
        ("without_health_check", |s| s.without_health_check()),
        ("retries", |s| s.retries(3)),
        ("backoff", |s| s.backoff(Duration::from_millis(50))),
        ("migrate_within", |s| s.migrate_within(Bound::After(Duration::from_secs(7)))),
        ("connect_within", |s| s.connect_within(Duration::from_secs(9))),
    ];
    for (name, set) in cases {
        let errors = refused(module::<Root>(vec![Box::new(|m| m.import(store())), Box::new(move |m| m.import(set(store())))]));
        assert_eq!(differing(&errors), [name], "a second import differing in `{name}` alone");
    }
}

/// A migrator has no value to compare, so two `.migrate(..)` calls are two settings even when
/// they name one migrator, and a store built in one function that both importers call is one.
#[test]
fn migrations_are_compared_by_the_call_that_set_them() {
    let errors = refused(module::<Root>(vec![Box::new(|m| m.import(store())), Box::new(|m| m.import(store().migrate(Migrator)))]));
    assert_eq!(differing(&errors), ["migrate"]);
    let text = errors.to_string();
    assert!(text.contains("`migrate` = off"), "{text}");
    assert!(text.contains(&format!("`migrate` = `.migrate(..)` at {}:", file!())), "{text}");

    let errors = refused(module::<Root>(vec![
        Box::new(|m| m.import(store().migrate(Migrator))),
        Box::new(|m| m.import(store().migrate(Migrator))),
    ]));
    assert_eq!(differing(&errors), ["migrate"]);

    fn migrating() -> StoreModule<Fake> {
        store().migrate(Migrator)
    }
    wire(module::<Root>(vec![Box::new(|m| m.import(migrating())), Box::new(|m| m.import(migrating()))]))
        .unwrap_or_else(|error| panic!("one `.migrate(..)` call is one setting:\n{error}"));
}

#[test]
fn two_imports_with_equal_settings_are_one_module() {
    let root = module::<Root>(vec![
        Box::new(|m| m.import(store().connect_within(Duration::from_secs(5)).retries(2))),
        Box::new(|m| m.import(module::<Inner>(vec![Box::new(|m| m.import(store().connect_within(Duration::from_secs(5)).retries(2)))]))),
    ]);
    connected(root, |app| {
        app.module::<StoreModule<Fake>>().unwrap_or_else(|error| panic!("one module under the identity: {error}"));
    });
}

/// Other options are another identity, so other settings beside them are two pools, no error.
#[test]
fn other_options_with_other_settings_are_two_modules() {
    // Each store in a module of its own, so no module sees two clients.
    let root = module::<Root>(vec![
        Box::new(|m| m.import(module::<Inner>(vec![Box::new(|m| m.import(StoreModule::<Fake>::from_options("one".to_owned())))]))),
        Box::new(|m| {
            m.import(module::<Other>(vec![Box::new(|m| {
                m.import(StoreModule::<Fake>::from_options("two".to_owned()).connect_within(Duration::from_secs(9)))
            })]))
        }),
    ]);
    connected(root, |app| {
        let lookup = app.module::<StoreModule<Fake>>().map(drop);
        assert!(matches!(lookup, Err(LookupError::AmbiguousModule { .. })), "two modules: {lookup:?}");
    });
}

/// A keyed pair over equal options is two identities, so its settings may differ.
#[test]
fn a_keyed_pair_with_other_settings_is_two_modules() {
    let root = module::<Root>(vec![
        Box::new(|m| m.import(store().keyed::<Primary>())),
        Box::new(|m| m.import(store().connect_within(Duration::from_secs(9)).without_health_check().keyed::<Replica>())),
    ]);
    connected(root, |app| {
        app.module_keyed::<StoreModule<Fake>, Primary>().unwrap_or_else(|error| panic!("the primary: {error}"));
        app.module_keyed::<StoreModule<Fake>, Replica>().unwrap_or_else(|error| panic!("the replica: {error}"));
    });
}
