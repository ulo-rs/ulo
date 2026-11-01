//! `Option<Dep<dyn Trait>>` as an `#[injectable]` field and as a constructor parameter, with
//! `dyn Trait` and `dyn Trait + Send + Sync` alike (core DESIGN section 3.2: `Option<S>` is an
//! injection point for any `FromContainer` type `S`).

use ulo::{App, Connected, Dep, Module, ModuleDef, ModuleIdentity, Signal, injectable};

trait Audit: Send + Sync {
    fn name(&self) -> &'static str;
}

struct Disk;

impl Audit for Disk {
    fn name(&self) -> &'static str {
        "disk"
    }
}

#[injectable]
struct FieldPlain {
    audit: Option<Dep<dyn Audit>>,
}

#[injectable]
struct FieldMarked {
    audit: Option<Dep<dyn Audit + Send + Sync>>,
}

struct CtorPlain {
    audit: Option<Dep<dyn Audit>>,
}

#[injectable]
impl CtorPlain {
    fn new(audit: Option<Dep<dyn Audit>>) -> Self {
        CtorPlain { audit }
    }
}

struct CtorMarked {
    audit: Option<Dep<dyn Audit + Send + Sync>>,
}

#[injectable]
impl CtorMarked {
    fn new(audit: Option<Dep<dyn Audit + Send + Sync>>) -> Self {
        CtorMarked { audit }
    }
}

/// Binds `Disk` under both spellings of the trait object, or neither.
struct Bound(bool);

impl Module for Bound {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if self.0 {
            m.value(Disk).also_as::<dyn Audit>(|d| d).also_as::<dyn Audit + Send + Sync>(|d| d);
        }
        m.provide::<FieldPlain>();
        m.provide::<FieldMarked>();
        m.provide::<CtorPlain>();
        m.provide::<CtorMarked>();
    }
}

async fn connected(bound: bool) -> App<Connected> {
    App::builder(Bound(bound))
        .timer(ulo_runtime_tokio::Timer::current())
        .wire()
        .unwrap_or_else(|error| panic!("did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("did not connect: {error}"))
}

/// What each of the four positions read, by trait object name or `none`.
async fn read(app: &App<Connected>) -> [&'static str; 4] {
    let name = |audit: &Option<Dep<dyn Audit>>| audit.as_ref().map_or("none", |audit| audit.name());
    let marked = |audit: &Option<Dep<dyn Audit + Send + Sync>>| audit.as_ref().map_or("none", |audit| audit.name());
    [
        name(&app.get::<FieldPlain>().await.expect("FieldPlain").audit),
        marked(&app.get::<FieldMarked>().await.expect("FieldMarked").audit),
        name(&app.get::<CtorPlain>().await.expect("CtorPlain").audit),
        marked(&app.get::<CtorMarked>().await.expect("CtorMarked").audit),
    ]
}

#[tokio::test]
async fn an_optional_trait_object_is_read_in_a_field_and_a_constructor_parameter_under_both_spellings() {
    let app = connected(true).await;
    assert_eq!(read(&app).await, ["disk"; 4]);
    let _ = app.close(Signal::new("test")).await;

    let app = connected(false).await;
    assert_eq!(read(&app).await, ["none"; 4]);
    let _ = app.close(Signal::new("test")).await;
}
