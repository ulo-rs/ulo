//! A hand-written `Construct` that reads a key its `dependencies` does not declare (section 0.1,
//! 3.4): the first construction refuses it, in every build, as a `Permanent` failure naming the
//! key and the reading type.
//!
//! The reads that are not construction reads, a `ModuleRef` or an execution used after the
//! constructor returned, are never refused.

#[macro_use]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use ulo::scope::{PerExecution, Singleton, Transient};
use ulo::{
    App, ConnectError, Construct, ConstructError, Dependencies, Dep, ExecOptions, Ext, FailureReason, Key, LookupError, Many, ModuleRef,
    Permanent, Resolver, StartupError, UndeclaredRead,
};

struct A;
struct Cfg(u32);
struct Absent;
struct Primary;
struct Db(&'static str);

trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
}

struct Named(&'static str);

impl Plugin for Named {
    fn id(&self) -> &'static str {
        self.0
    }
}

/// Declares `Cfg` and reads `Cfg`.
struct Declared(Dep<Cfg>);

impl Construct for Declared {
    type Scope = Singleton;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Declared(r.dep::<Cfg>().await?))
    }
}

/// Declares nothing and reads `Cfg`.
struct Undeclared;

impl Construct for Undeclared {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep::<Cfg>().await?;
        Ok(Undeclared)
    }
}

/// Reads `Cfg` undeclared and discards the refusal.
struct Swallows;

impl Construct for Swallows {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let _ = r.dep::<Cfg>().await.ok();
        Ok(Swallows)
    }
}

/// Reads an unbound key through `Option`, undeclared.
struct UndeclaredOptional;

impl Construct for UndeclaredOptional {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        <Option<Dep<Absent>> as ulo::FromContainer>::from_container(r).await?;
        Ok(UndeclaredOptional)
    }
}

/// Reads a bound key through `Option`, undeclared.
struct UndeclaredOptionalBound;

impl Construct for UndeclaredOptionalBound {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        <Option<Dep<Cfg>> as ulo::FromContainer>::from_container(r).await?;
        Ok(UndeclaredOptionalBound)
    }
}

/// Declares an optional read, a collection and a qualified read, and makes all three.
struct AllShapes {
    absent: bool,
    plugins: Vec<&'static str>,
    primary: &'static str,
}

impl Construct for AllShapes {
    type Scope = Singleton;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Option<Dep<Absent>>>("absent");
        d.field::<Many<dyn Plugin>>("plugins");
        d.field::<Dep<Db, Primary>>("primary");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let absent = <Option<Dep<Absent>> as ulo::FromContainer>::from_container(r).await?.is_none();
        let plugins = r.many::<dyn Plugin>().await?.iter().map(|p| p.id()).collect();
        let primary = r.dep_qualified::<Db, Primary>().await?.0;
        Ok(AllShapes { absent, plugins, primary })
    }
}

/// Declares the unqualified `Db`, reads `Db @ Primary`.
struct WrongQualifier;

impl Construct for WrongQualifier {
    type Scope = Singleton;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Db>>("db");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep_qualified::<Db, Primary>().await?;
        Ok(WrongQualifier)
    }
}

/// Keeps the `ModuleRef` and reads through it after construction.
struct Lazy {
    module: ModuleRef,
}

impl Construct for Lazy {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Lazy { module: r.module() })
    }
}

static EXECUTION_BODY_RUNS: AtomicUsize = AtomicUsize::new(0);

/// Execution-scoped, reads `Cfg` undeclared.
struct PerExecUndeclared;

impl Construct for PerExecUndeclared {
    type Scope = PerExecution;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        EXECUTION_BODY_RUNS.fetch_add(1, Ordering::SeqCst);
        r.dep::<Cfg>().await?;
        Ok(PerExecUndeclared)
    }
}

static TRANSIENT_BODY_RUNS: AtomicUsize = AtomicUsize::new(0);

/// Transient, reads `Cfg` undeclared.
struct TransientUndeclared;

impl Construct for TransientUndeclared {
    type Scope = Transient;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        TRANSIENT_BODY_RUNS.fetch_add(1, Ordering::SeqCst);
        r.dep::<Cfg>().await?;
        Ok(TransientUndeclared)
    }
}

/// Reads an extension undeclared.
struct ExtUndeclared;

impl Construct for ExtUndeclared {
    type Scope = PerExecution;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.ext::<Cfg>()?;
        Ok(ExtUndeclared)
    }
}

/// Declares and reads an extension.
struct ExtDeclared(u32);

impl Construct for ExtDeclared {
    type Scope = PerExecution;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Ext<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(ExtDeclared(r.ext::<Cfg>()?.0))
    }
}

/// What the construction failure of a singleton reports: the reader's key, and the refusal found
/// under the redacted error.
fn refusal(error: StartupError) -> (Key, UndeclaredRead, String) {
    assert!(ulo::is_permanent(&error), "an undeclared read is a Permanent failure: {error}");
    let StartupError::Connect(ConnectError::Construct { key, reason: FailureReason::Errored(redacted), .. }) = error else {
        panic!("expected an errored construction failure, got: {error}");
    };
    (key.key(), found_in(&redacted), redacted.to_string())
}

fn found_in(redacted: &ulo::Redacted) -> UndeclaredRead {
    let permanent = redacted.downcast_ref::<Permanent>().expect("the refusal is wrapped in `Permanent`");
    let source = std::error::Error::source(permanent).expect("a `Permanent` has its source");
    source.downcast_ref::<UndeclaredRead>().expect("the source is an `UndeclaredRead`").clone()
}

fn cfg(m: &mut ulo::ModuleDef<'_>) {
    m.singleton(|| async { Cfg(7) });
}

#[tokio::test]
async fn an_undeclared_read_fails_the_first_construction_naming_the_key_and_the_reader() {
    let error = connect_err(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<Undeclared>();
    }))
    .await;

    let (key, found, text) = refusal(error);
    assert_eq!(key, Key::of::<Undeclared, ()>());
    assert_eq!(found.key().expect("a key read").key(), Key::of::<Cfg, ()>());
    assert_eq!(found.reader().to_string(), "Undeclared");
    assert!(text.contains("Undeclared") && text.contains("Cfg"), "names both: {text}");
    assert!(text.contains("dependencies"), "says where to declare it: {text}");
}

#[tokio::test]
async fn a_declared_read_constructs() {
    let app = connects(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<Declared>();
    }))
    .await;
    assert_eq!(app.get::<Declared>().await.unwrap().0.0, 7);
    close(app).await;
}

#[tokio::test]
async fn optional_collection_and_qualified_reads_declared_construct() {
    let app = connects(Mod::<A>::new(|m| {
        m.singleton(|| async { Db("primary") }).qualified::<Primary>();
        m.contribute::<dyn Plugin>().value(Arc::new(Named("one")));
        m.contribute::<dyn Plugin>().value(Arc::new(Named("two")));
        m.provide::<AllShapes>();
    }))
    .await;
    let shapes = app.get::<AllShapes>().await.unwrap();
    assert!(shapes.absent);
    assert_eq!(shapes.plugins, ["one", "two"]);
    assert_eq!(shapes.primary, "primary");
    close(app).await;
}

#[tokio::test]
async fn a_qualified_read_is_not_covered_by_declaring_the_unqualified_key() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.singleton(|| async { Db("plain") });
        m.singleton(|| async { Db("primary") }).qualified::<Primary>();
        m.provide::<WrongQualifier>();
    }))
    .await;
    let (_, found, _) = refusal(error);
    assert_eq!(found.key().unwrap().key(), Key::of::<Db, Primary>());
}

#[tokio::test]
async fn an_undeclared_optional_read_is_refused_whether_or_not_the_key_is_bound() {
    let unbound = connect_err(Mod::<A>::new(|m| {
        m.provide::<UndeclaredOptional>();
    }))
    .await;
    let (_, found, _) = refusal(unbound);
    assert_eq!(found.key().unwrap().key(), Key::of::<Absent, ()>());

    let bound = connect_err(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<UndeclaredOptionalBound>();
    }))
    .await;
    let (_, found, _) = refusal(bound);
    assert_eq!(found.key().unwrap().key(), Key::of::<Cfg, ()>());
}

#[tokio::test]
async fn a_body_that_discards_the_refusal_still_fails() {
    let error = connect_err(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<Swallows>();
    }))
    .await;
    let (key, found, _) = refusal(error);
    assert_eq!(key, Key::of::<Swallows, ()>());
    assert_eq!(found.reader().to_string(), "Swallows");
}

#[tokio::test]
async fn a_module_ref_kept_past_construction_reads_without_a_declaration() {
    let app = connects(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<Lazy>();
    }))
    .await;
    let lazy = app.get::<Lazy>().await.unwrap();
    assert_eq!(lazy.module.get::<Cfg>().await.unwrap().0, 7);
    let from_execution = app.execute(ExecOptions::new(), async |exec| exec.get::<Cfg>().await.unwrap().0).await.unwrap();
    assert_eq!(from_execution, 7, "a read through an execution handle is not a construction read");
    close(app).await;
}

fn construct_refusal(error: LookupError) -> (Key, UndeclaredRead) {
    let LookupError::Construct { key, reason: FailureReason::Errored(redacted) } = error else {
        panic!("expected a construction failure, got: {error}");
    };
    (key.key(), found_in(&redacted))
}

#[tokio::test]
async fn an_execution_scoped_provider_is_refused_at_its_first_construction_in_each_execution() {
    let app = connects(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<PerExecUndeclared>();
    }))
    .await;

    for round in 1..=2 {
        let error = app
            .execute(ExecOptions::new(), async |exec| exec.get::<PerExecUndeclared>().await.err().expect("refused"))
            .await
            .unwrap();
        let (key, found) = construct_refusal(error);
        assert_eq!(key, Key::of::<PerExecUndeclared, ()>());
        assert_eq!(found.key().unwrap().key(), Key::of::<Cfg, ()>());
        assert_eq!(EXECUTION_BODY_RUNS.load(Ordering::SeqCst), round, "one construction attempt per execution");
    }
    close(app).await;
}

#[tokio::test]
async fn a_transient_is_refused_at_every_construction() {
    let app = connects(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<TransientUndeclared>();
    }))
    .await;
    for round in 1..=2 {
        let error = app
            .execute(ExecOptions::new(), async |exec| exec.get::<TransientUndeclared>().await.err().expect("refused"))
            .await
            .unwrap();
        let (key, found) = construct_refusal(error);
        assert_eq!(key, Key::of::<TransientUndeclared, ()>());
        assert_eq!(found.reader().to_string(), "TransientUndeclared");
        assert_eq!(TRANSIENT_BODY_RUNS.load(Ordering::SeqCst), round);
    }
    close(app).await;
}

#[tokio::test]
async fn an_extension_read_must_be_declared_too() {
    let app = connects(Mod::<A>::new(|m| {
        m.provide::<ExtUndeclared>();
        m.provide::<ExtDeclared>();
    }))
    .await;
    let (refused, declared) = app
        .execute(ExecOptions::new(), async |exec| {
            exec.extensions().insert(Cfg(3));
            (exec.get::<ExtUndeclared>().await.err().expect("refused"), exec.get::<ExtDeclared>().await.unwrap().0)
        })
        .await
        .unwrap();
    let (_, found) = construct_refusal(refused);
    assert!(found.key().is_none(), "an extension read names no key");
    assert!(found.to_string().contains("extension"), "{found}");
    assert_eq!(found.extension().map(|e| e.to_string()).as_deref(), Some("Cfg"));
    assert_eq!(declared, 3);
    close(app).await;
}

struct Other;

/// Declares `Ext<Cfg>`, reads `Ext<Other>`.
struct ExtWrongType;

impl Construct for ExtWrongType {
    type Scope = PerExecution;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Ext<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.ext::<Other>()?;
        Ok(ExtWrongType)
    }
}

/// Declares `Option<Ext<Cfg>>`, reads `Ext<Cfg>`.
struct ExtOptionalDeclared(u32);

impl Construct for ExtOptionalDeclared {
    type Scope = PerExecution;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Option<Ext<Cfg>>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(ExtOptionalDeclared(r.ext::<Cfg>()?.0))
    }
}

#[tokio::test]
async fn declaring_one_extension_does_not_cover_another() {
    let app = connects(Mod::<A>::new(|m| {
        m.provide::<ExtWrongType>();
        m.provide::<ExtOptionalDeclared>();
    }))
    .await;
    let (refused, optional) = app
        .execute(ExecOptions::new(), async |exec| {
            exec.extensions().insert(Cfg(3));
            exec.extensions().insert(Other);
            (exec.get::<ExtWrongType>().await.err().expect("refused"), exec.get::<ExtOptionalDeclared>().await.unwrap().0)
        })
        .await
        .unwrap();
    let (_, found) = construct_refusal(refused);
    assert_eq!(found.reader().to_string(), "ExtWrongType");
    assert_eq!(found.extension().map(|e| e.to_string()).as_deref(), Some("Other"), "{found}");
    assert!(found.key().is_none());
    assert!(found.to_string().contains("Ext<Other>"), "{found}");
    assert_eq!(optional, 3);
    close(app).await;
}

#[tokio::test]
async fn wiring_cannot_see_a_hand_written_body_so_the_graph_wires() {
    let wired = App::builder(Mod::<A>::new(|m| {
        cfg(m);
        m.provide::<Undeclared>();
    }))
    .wire();
    assert!(wired.is_ok());
}

struct X;
struct Y;

/// Declares `Dep<Cfg>`; reads the collection `Many<dyn Plugin>`, then its entries, then the
/// execution, then the input `X`, then `X` by key: each undeclared, each its own type below.
macro_rules! reads_undeclared {
    ($name:ident, $scope:ty, |$r:ident| $read:expr) => {
        struct $name;

        impl Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut Dependencies) {
                d.field::<Dep<Cfg>>("cfg");
            }

            async fn construct($r: &Resolver<'_>) -> Result<Self, ConstructError> {
                let _ = $read;
                Ok($name)
            }
        }
    };
}

reads_undeclared!(ReadsMany, PerExecution, |r| r.many::<dyn Plugin>().await);
reads_undeclared!(ReadsEntries, PerExecution, |r| r.entries::<dyn Plugin>().map(|e| e.count()));
reads_undeclared!(ReadsExecution, PerExecution, |r| r.execution());
reads_undeclared!(ReadsInput, PerExecution, |r| r.input::<X>());
reads_undeclared!(ReadsByKey, PerExecution, |r| r.by_key::<X>(Key::of::<X, ()>()).await);
reads_undeclared!(SingleForMany, PerExecution, |r| r.dep::<Cfg>().await.and(r.many::<Cfg>().await));

/// Declares an extension and reads the execution.
struct ExtForExecution;

impl Construct for ExtForExecution {
    type Scope = PerExecution;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Ext<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.execution()?;
        Ok(ExtForExecution)
    }
}

/// A single and a collection under one key are two reads: declaring `Dep<Cfg>` covers `Cfg` and
/// not `Many<Cfg>`. Every resolver read a body can make is checked.
#[tokio::test]
async fn every_resolver_read_is_checked() {
    let app = connects(Mod::<A>::new(|m| {
        cfg(m);
        m.singleton(|| async { X });
        m.contribute::<dyn Plugin>().value(Arc::new(Named("one")));
        m.provide::<ReadsMany>();
        m.provide::<ReadsEntries>();
        m.provide::<ReadsExecution>();
        m.provide::<ReadsInput>();
        m.provide::<ReadsByKey>();
        m.provide::<SingleForMany>();
        m.provide::<ExtForExecution>();
    }))
    .await;
    let refused = app
        .execute(ExecOptions::new(), async |exec| {
            vec![
                ("ReadsMany", exec.get::<ReadsMany>().await.err().map(construct_refusal)),
                ("ReadsEntries", exec.get::<ReadsEntries>().await.err().map(construct_refusal)),
                ("ReadsExecution", exec.get::<ReadsExecution>().await.err().map(construct_refusal)),
                ("ReadsInput", exec.get::<ReadsInput>().await.err().map(construct_refusal)),
                ("ReadsByKey", exec.get::<ReadsByKey>().await.err().map(construct_refusal)),
                ("SingleForMany", exec.get::<SingleForMany>().await.err().map(construct_refusal)),
                ("ExtForExecution", exec.get::<ExtForExecution>().await.err().map(construct_refusal)),
            ]
        })
        .await
        .unwrap();
    let plugins = Key::of::<dyn Plugin, ()>();
    let expected = [
        ("ReadsMany", Some(plugins)),
        ("ReadsEntries", Some(plugins)),
        ("ReadsExecution", None),
        ("ReadsInput", Some(Key::of::<X, ()>())),
        ("ReadsByKey", Some(Key::of::<X, ()>())),
        ("SingleForMany", Some(Key::of::<Cfg, ()>())),
        ("ExtForExecution", None),
    ];
    for ((name, found), (_, key)) in refused.into_iter().zip(expected) {
        let (reader, found) = found.unwrap_or_else(|| panic!("`{name}` built through an undeclared read"));
        assert_eq!(reader.type_name().to_string(), name);
        assert_eq!(found.reader().to_string(), name);
        assert_eq!(found.key().map(|k| k.key()), key, "{name}");
        if name == "ReadsMany" || name == "ReadsEntries" {
            assert_eq!(found.key().unwrap().kind(), ulo::BindingKind::Collection, "{name} names the collection");
        }
    }
    close(app).await;
}

/// Reads `X`, then `Y`, both undeclared, swallowing both: the refusal names the first.
struct TwoUndeclared;

impl Construct for TwoUndeclared {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let _ = r.dep::<X>().await;
        let _ = r.dep::<Y>().await;
        Ok(TwoUndeclared)
    }
}

#[tokio::test]
async fn the_first_undeclared_read_is_the_one_reported() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.singleton(|| async { X });
        m.singleton(|| async { Y });
        m.provide::<TwoUndeclared>();
    }))
    .await;
    let (_, found, _) = refusal(error);
    assert_eq!(found.key().unwrap().key(), Key::of::<X, ()>());
}

/// Declares and reads `Dep<Cfg>` and `Dep<X>`, yielding between them.
struct Good;

impl Construct for Good {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Cfg>>("cfg").field::<Dep<X>>("x");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep::<Cfg>().await?;
        tokio::task::yield_now().await;
        r.dep::<X>().await?;
        Ok(Good)
    }
}

/// Declares `Dep<Cfg>`, yields, reads `X` undeclared and swallows the refusal.
struct BadX;

impl Construct for BadX {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Cfg>>("cfg");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep::<Cfg>().await?;
        tokio::task::yield_now().await;
        let _ = r.dep::<X>().await;
        Ok(BadX)
    }
}

/// Declares nothing, reads `Y` after yielding.
struct BadY;

impl Construct for BadY {
    type Scope = Transient;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        tokio::task::yield_now().await;
        r.dep::<Y>().await?;
        Ok(BadY)
    }
}

/// Declares `Dep<BadY>` and reads it: the nested refusal names `BadY`, the type that read `Y`.
struct OverBadY;

impl Construct for OverBadY {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<BadY>>("inner");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep::<BadY>().await?;
        Ok(OverBadY)
    }
}

/// Declares `Dep<Good>` and builds it, whose reads are its own, then reads `Y` undeclared.
struct OverGood;

impl Construct for OverGood {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Good>>("good");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        r.dep::<Good>().await?;
        let _ = r.dep::<Y>().await;
        Ok(OverGood)
    }
}

fn mixed() -> Mod<A> {
    Mod::<A>::new(|m| {
        cfg(m);
        m.singleton(|| async { X });
        m.singleton(|| async { Y });
        m.provide::<Good>();
        m.provide::<BadX>();
        m.provide::<BadY>();
        m.provide::<OverBadY>();
        m.provide::<OverGood>();
    })
}

/// Builds the `i`th of the mix and checks the outcome: `None` when it is as expected.
async fn built(exec: &ulo::Execution, i: usize) -> Option<String> {
    fn expect(name: &str, result: Result<(), LookupError>, reader: Key, read: Key) -> Option<String> {
        let Err(error) = result else { return Some(format!("{name} built")) };
        let (key, found) = construct_refusal(error);
        let read_found = found.key().map(|k| k.key());
        (key != reader || found.reader() != reader.type_name() || read_found != Some(read))
            .then(|| format!("{name}: built {key}, reader {}, read {read_found:?}", found.reader()))
    }
    let (x, y) = (Key::of::<X, ()>(), Key::of::<Y, ()>());
    match i % 10 {
        0..=4 => exec.get::<Good>().await.err().map(|e| format!("Good failed: {e}")),
        5 | 6 => expect("BadX", exec.get::<BadX>().await.map(drop), Key::of::<BadX, ()>(), x),
        7 => expect("BadY", exec.get::<BadY>().await.map(drop), Key::of::<BadY, ()>(), y),
        8 => expect("OverBadY", exec.get::<OverBadY>().await.map(drop), Key::of::<BadY, ()>(), y),
        _ => expect("OverGood", exec.get::<OverGood>().await.map(drop), Key::of::<OverGood, ()>(), y),
    }
}

/// A refusal is the construction's own: 1000 transients, half declaring what they read and half
/// not, nested and not, each in its own execution on its own task over eight workers, every one
/// answering for itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_constructions_are_refused_each_for_its_own_reads() {
    let app = Arc::new(connects(mixed()).await);
    let tasks: Vec<_> = (0..1000)
        .map(|i| {
            let app = Arc::clone(&app);
            tokio::spawn(async move { app.execute(ExecOptions::new(), async |exec| built(exec, i).await).await.unwrap() })
        })
        .collect();
    let mut wrong = Vec::new();
    for task in tasks {
        wrong.extend(task.await.unwrap());
    }
    assert!(wrong.is_empty(), "{} of 1000 misattributed, first: {:?}", wrong.len(), wrong.first());
    let Ok(app) = Arc::try_unwrap(app) else { panic!("a task kept the app") };
    close(app).await;
}

/// The same 1000 inside one execution, polled together, so their reads interleave on one task.
#[tokio::test]
async fn interleaved_constructions_in_one_execution_are_refused_each_for_its_own_reads() {
    let app = connects(mixed()).await;
    let wrong = app
        .execute(ExecOptions::new(), async |exec| {
            let mut all: Vec<_> = (0..1000).map(|i| Box::pin(built(exec, i))).collect();
            let mut out: Vec<Option<Option<String>>> = (0..all.len()).map(|_| None).collect();
            std::future::poll_fn(|cx| {
                let mut pending = false;
                for (slot, fut) in out.iter_mut().zip(all.iter_mut()) {
                    if slot.is_none() {
                        match fut.as_mut().poll(cx) {
                            std::task::Poll::Ready(v) => *slot = Some(v),
                            std::task::Poll::Pending => pending = true,
                        }
                    }
                }
                if pending { std::task::Poll::Pending } else { std::task::Poll::Ready(()) }
            })
            .await;
            out.into_iter().flatten().flatten().collect::<Vec<_>>()
        })
        .await
        .unwrap();
    assert!(wrong.is_empty(), "{} of 1000 misattributed, first: {:?}", wrong.len(), wrong.first());
    close(app).await;
}

struct Made(u32);

/// Declares and reads `Dep<Made>`, a transient factory whose own parameter, `Dep<Cfg>`, this
/// type does not declare: the factory's reads are the factory's.
struct OverFactory(u32);

impl Construct for OverFactory {
    type Scope = Transient;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<Made>>("made");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(OverFactory(r.dep::<Made>().await?.0))
    }
}

#[tokio::test]
async fn a_dependency_built_inside_a_read_is_checked_against_its_own_declaration() {
    let app = connects(Mod::<A>::new(|m| {
        cfg(m);
        m.transient(|cfg: Dep<Cfg>| async move { Made(cfg.0) });
        m.provide::<OverFactory>();
        // `Undeclared::construct` reads `Cfg` undeclared; built by a factory, it never runs.
        m.provide_with::<Undeclared, _>(|| async { Undeclared });
    }))
    .await;
    let built = app.execute(ExecOptions::new(), async |exec| exec.get::<OverFactory>().await.map(|v| v.0)).await.unwrap();
    assert_eq!(built.unwrap(), 7);
    assert!(app.get::<Undeclared>().await.is_ok(), "a `Construct` built by `provide_with` runs the factory, not `construct`");
    close(app).await;
}

static SEEN_BY_THE_BODY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Reads `Absent` through `Option` undeclared, records what the read answered, and fails with its
/// own error in place of the refusal.
struct ReplacesTheRefusal;

impl Construct for ReplacesTheRefusal {
    type Scope = Singleton;

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let read = <Option<Dep<Absent>> as ulo::FromContainer>::from_container(r).await;
        *SEEN_BY_THE_BODY.lock().unwrap() = Some(match &read {
            Ok(None) => "None".into(),
            Ok(Some(_)) => "Some".into(),
            Err(LookupError::Construct { .. }) => "Construct".into(),
            Err(other) => format!("{other}"),
        });
        Err(ConstructError::failed(std::io::Error::other("the body's own error")))
    }
}

/// The body sees the refusal as a construction failure, which `Option` does not turn into
/// `None`, and the refusal is reported in place of whatever the body answers with.
#[tokio::test]
async fn the_refusal_replaces_the_body_s_own_error() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.provide::<ReplacesTheRefusal>();
    }))
    .await;
    let (key, found, _) = refusal(error);
    assert_eq!(key, Key::of::<ReplacesTheRefusal, ()>());
    assert_eq!(found.key().unwrap().key(), Key::of::<Absent, ()>());
    assert_eq!(SEEN_BY_THE_BODY.lock().unwrap().as_deref(), Some("Construct"));
}
