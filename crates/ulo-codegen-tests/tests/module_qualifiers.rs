//! The `@ Qualifier` slot of `#[module]`: `Type @ Q` and `expr @ Q` in `providers`, `Type @ Q` and
//! `reexport Type @ Q` in `exports`. Each test wires and connects the module the attribute wrote
//! and reads the bindings back by key, so a qualifier lowered to the wrong call fails by the key
//! it bound. Capability 5 (qualified bindings of one type are told apart), through the attribute.

use std::sync::Arc;

use ulo::{App, Connected, Dep, ExecOptions, Key, LookupError, LookupKind, StartupError, WiringError, injectable, module};

struct Primary;
struct Replica;

#[derive(Debug, PartialEq)]
struct Db(&'static str);

trait Repo: Send + Sync {
    fn name(&self) -> &'static str;
}

trait Other: Send + Sync {}

impl Other for PgRepo {}

#[injectable]
struct PgRepo;

impl Repo for PgRepo {
    fn name(&self) -> &'static str {
        "pg"
    }
}

fn from_env(ok: bool) -> Result<Db, std::io::Error> {
    if ok { Ok(Db("env")) } else { Err(std::io::Error::other("no DB_URL")) }
}

async fn connected(root: impl ulo::Module) -> App<Connected> {
    App::builder(root).wire().expect("wires").connect().await.expect("connects")
}

async fn read<T: ?Sized + Send + Sync + 'static, Q: 'static>(app: &App<Connected>) -> Result<Dep<T, Q>, LookupError> {
    app.execute(ExecOptions::new(), async |exec| exec.resolver().dep_qualified::<T, Q>().await)
        .await
        .expect("a connected app opens executions")
}

async fn read_many<T: ?Sized + Send + Sync + 'static>(app: &App<Connected>) -> ulo::Many<T> {
    app.execute(ExecOptions::new(), async |exec| exec.resolver().many::<T>().await)
        .await
        .expect("a connected app opens executions")
        .expect("the collection resolves")
}

#[module(providers = [Db("primary") @ Primary, Db("replica") @ Replica])]
struct Values;

#[tokio::test]
async fn a_value_is_bound_under_its_qualifier_and_not_unqualified() {
    let app = connected(Values).await;

    // Nothing is exported, so the root reads its own bindings through the app.
    assert_eq!(*read::<Db, Primary>(&app).await.unwrap(), Db("primary"));
    assert_eq!(*read::<Db, Replica>(&app).await.unwrap(), Db("replica"));
    assert!(matches!(app.get::<Db>().await, Err(LookupError::NotFound { kind: LookupKind::Binding, .. })));
}

#[module(providers = [PgRepo @ Primary])]
struct TypeEntry;

#[tokio::test]
async fn a_type_is_bound_under_its_qualifier() {
    let app = connected(TypeEntry).await;

    assert!(read::<PgRepo, Primary>(&app).await.is_ok());
    assert!(matches!(app.get::<PgRepo>().await, Err(LookupError::NotFound { .. })));
}

#[injectable]
struct Reader {
    db: Dep<Db, Replica>,
}

#[module(providers = [Db("replica") @ Replica, Reader @ Primary])]
struct TypeWithDependency;

/// `Type @ Q` is a type entry, constructed with its dependencies, and not a value expression.
#[tokio::test]
async fn a_type_with_dependencies_is_constructed_under_its_qualifier() {
    let app = connected(TypeWithDependency).await;
    assert_eq!(read::<Reader, Primary>(&app).await.unwrap().db.0, "replica");
}

#[module(providers = [PgRepo as dyn Repo @ Primary])]
struct TypeAsTrait;

#[tokio::test]
async fn as_dyn_with_a_qualifier_qualifies_the_concrete_and_the_trait_key() {
    let app = connected(TypeAsTrait).await;

    let by_trait = read::<dyn Repo, Primary>(&app).await.unwrap();
    let concrete = read::<PgRepo, Primary>(&app).await.unwrap();
    assert_eq!(by_trait.name(), "pg");
    assert!(std::ptr::addr_eq(Arc::as_ptr(by_trait.arc()), Arc::as_ptr(concrete.arc())), "one object under both keys");
    assert!(matches!(app.get::<dyn Repo>().await, Err(LookupError::NotFound { .. })), "the unqualified trait key is unbound");
}

#[module(providers = [from_env(true)? @ Replica])]
struct TryValue;

#[tokio::test]
async fn a_try_value_is_bound_under_its_qualifier() {
    let app = connected(TryValue).await;
    assert_eq!(*read::<Db, Replica>(&app).await.unwrap(), Db("env"));
}

#[module(providers = [from_env(false)? @ Replica])]
struct FailingTryValue;

#[test]
fn a_failing_try_value_with_a_qualifier_is_reported_under_the_qualified_key() {
    let Err(StartupError::Wiring(errors)) = App::builder(FailingTryValue).wire() else { panic!("wire() should refuse the module") };
    let [WiringError::ValueFailed { key, .. }] = errors.iter().collect::<Vec<_>>()[..] else { panic!("expected one ValueFailed, got {errors}") };
    assert_eq!(key.key(), Key::of::<Db, Replica>());
}

#[module(providers = [Db("one") @ Primary, Db("two") @ Primary])]
struct Twice;

#[test]
fn two_values_under_one_qualifier_are_a_duplicate_binding_of_the_qualified_key() {
    let Err(StartupError::Wiring(errors)) = App::builder(Twice).wire() else { panic!("wire() should refuse the module") };
    let [WiringError::DuplicateBinding { key, .. }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one DuplicateBinding, got {errors}")
    };
    assert_eq!(key.key(), Key::of::<Db, Primary>());
}

#[module(providers = [Db("plain"), Db("primary") @ Primary])]
struct BesideUnqualified;

#[tokio::test]
async fn a_qualified_binding_sits_beside_the_unqualified_one() {
    let app = connected(BesideUnqualified).await;

    assert_eq!(*app.get::<Db>().await.unwrap(), Db("plain"));
    assert_eq!(*read::<Db, Primary>(&app).await.unwrap(), Db("primary"));
}

#[module(providers = [Db("primary") @ Primary, Db("replica") @ Replica], exports = [Db @ Primary])]
struct Exporter;

#[injectable]
struct Consumer {
    primary: Dep<Db, Primary>,
}

#[module(imports = [Exporter], providers = [Consumer])]
struct Importer;

#[tokio::test]
async fn an_export_with_a_qualifier_leaves_only_that_key() {
    let app = connected(Importer).await;
    assert_eq!(app.get::<Consumer>().await.unwrap().primary.0, "primary");
}

#[injectable]
struct ReplicaConsumer {
    _replica: Dep<Db, Replica>,
}

#[module(imports = [Exporter], providers = [ReplicaConsumer])]
struct ReplicaImporter;

#[test]
fn a_qualified_key_not_exported_is_missing_to_the_importer() {
    let Err(StartupError::Wiring(errors)) = App::builder(ReplicaImporter).wire() else { panic!("wire() should refuse the module") };
    let [WiringError::Missing { key, .. }] = errors.iter().collect::<Vec<_>>()[..] else { panic!("expected one Missing, got {errors}") };
    assert_eq!(key.key(), Key::of::<Db, Replica>());
}

#[module(imports = [Exporter], exports = [reexport Db @ Primary])]
struct Relay;

#[module(imports = [Relay], providers = [Consumer])]
struct Far;

#[tokio::test]
async fn reexport_with_a_qualifier_passes_the_qualified_key_on() {
    let app = connected(Far).await;
    assert_eq!(app.get::<Consumer>().await.unwrap().primary.0, "primary");
}

#[module(providers = [Db("primary") @ Primary], exports = [Db])]
struct ExportsTheUnqualifiedKey;

#[test]
fn exporting_the_unqualified_key_of_a_qualified_binding_is_export_not_bound() {
    let Err(StartupError::Wiring(errors)) = App::builder(ExportsTheUnqualifiedKey).wire() else { panic!("wire() should refuse the module") };
    let [WiringError::ExportNotBound { key, .. }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one ExportNotBound, got {errors}")
    };
    assert_eq!(key.key(), Key::of::<Db, ()>());
}

/// Every form beside a qualifier in one list, for the parser: a qualifier does not swallow the
/// comma that follows it, and the entry after it is read as its own.
#[module(providers = [Db("a") @ Primary, PgRepo as dyn Repo, 7_u8, with = || 9_u16, into dyn Other: [PgRepo]])]
struct Beside;

#[tokio::test]
async fn a_qualified_entry_leaves_its_neighbours_as_written() {
    let app = connected(Beside).await;

    assert_eq!(*read::<Db, Primary>(&app).await.unwrap(), Db("a"));
    assert_eq!(app.get::<dyn Repo>().await.unwrap().name(), "pg");
    assert_eq!(*app.get::<u8>().await.unwrap(), 7);
    assert_eq!(*app.get::<u16>().await.unwrap(), 9);
    assert_eq!(read_many::<dyn Other>(&app).await.len(), 1);
}
